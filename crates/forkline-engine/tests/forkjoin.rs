//! Phase 2: forks, branches, joins, merge rules, cancellation, panics, detached paths.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use forkline_core::{
    Branch, Build, BuildError, Ctx, Decode, Empty, Encode, ErrorKind, Join, JoinStep, Module, MultiBranch, NoConfig,
    NodeError, PlanError, Registry, Reply, Status, Terminal,
};
use forkline_engine::{Engine, Request};
use forkline_macros::interface;
use forkline_plan::PlansFile;
use serde::Deserialize;
use tokio::time::Instant;

interface! {
    #[forkline(crate = forkline_core)]
    pub struct KeysOut {
        out a: i64, out e: i64, out f: i64, out g: i64,
        out k: i64, out m: i64, out n: i64, out p: i64,
    }
}
interface! {
    #[forkline(crate = forkline_core)]
    pub struct KeysIn {
        opt a: i64, opt e: i64, opt f: i64, opt g: i64,
        opt k: i64, opt m: i64, opt n: i64, opt p: i64,
    }
}
interface! {
    #[forkline(crate = forkline_core)]
    pub struct SumIo { inout a: i64, inout joined: i64 }
}

#[derive(Deserialize, Default, Clone)]
#[serde(deny_unknown_fields, default)]
struct WriteConfig {
    set: Vec<String>,
    value: i64,
    delay_ms: u64,
    panic: bool,
    fail: bool,
    counter: Option<String>,
}

static COUNTERS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, u32>>> =
    std::sync::LazyLock::new(Default::default);

fn bump(name: &str) {
    *COUNTERS.lock().unwrap().entry(name.to_owned()).or_default() += 1;
}

fn count(name: &str) -> u32 {
    COUNTERS.lock().unwrap().get(name).copied().unwrap_or(0)
}

fn write_keys(c: &WriteConfig, io: &mut KeysOut<'_>) {
    for key in &c.set {
        let v = c.value;
        match key.as_str() {
            "a" => io.set_a(v),
            "e" => io.set_e(v),
            "f" => io.set_f(v),
            "g" => io.set_g(v),
            "k" => io.set_k(v),
            "m" => io.set_m(v),
            "n" => io.set_n(v),
            "p" => io.set_p(v),
            other => panic!("unknown key {other}"),
        }
    }
}

/// Writes the configured keys, optionally after a delay, failing or panicking.
struct Write(WriteConfig);
impl Build for Write {
    type Config = WriteConfig;
    fn build(c: WriteConfig) -> Result<Self, BuildError> {
        Ok(Self(c))
    }
}
impl Module for Write {
    type Io<'a> = KeysOut<'a>;
    async fn call(&self, _: &Ctx, mut io: KeysOut<'_>) -> Result<(), NodeError> {
        if self.0.delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.0.delay_ms)).await;
        }
        assert!(!self.0.panic, "boom");
        if self.0.fail {
            return Err(NodeError::msg("failed on purpose"));
        }
        write_keys(&self.0, &mut io);
        if let Some(name) = &self.0.counter {
            bump(name);
        }
        Ok(())
    }
}

struct Seed(WriteConfig);
impl Build for Seed {
    type Config = WriteConfig;
    fn build(c: WriteConfig) -> Result<Self, BuildError> {
        Ok(Self(c))
    }
}
impl Decode for Seed {
    type Io<'a> = KeysOut<'a>;
    fn decode(&self, _: &Ctx, _: &Bytes, mut io: KeysOut<'_>) -> Result<(), NodeError> {
        assert!(!self.0.panic, "decode boom");
        write_keys(&self.0, &mut io);
        Ok(())
    }
}

/// Prints the present keys, e.g. `a=5 f=2 g=1`.
struct Dump;
impl Build for Dump {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Encode for Dump {
    type Io<'a> = KeysIn<'a>;
    fn encode(&self, _: &Ctx, io: KeysIn<'_>) -> Result<Reply, NodeError> {
        let pairs = [
            ("a", io.a()),
            ("e", io.e()),
            ("f", io.f()),
            ("g", io.g()),
            ("k", io.k()),
            ("m", io.m()),
            ("n", io.n()),
            ("p", io.p()),
        ];
        let text: Vec<String> = pairs.iter().filter_map(|(name, v)| v.map(|v| format!("{name}={v}"))).collect();
        Ok(Reply::ok(text.join(" ")))
    }
}

/// Sums `a` over the arriving paths (the guide's join).
struct SumA;
impl Build for SumA {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Join for SumA {
    type Io<'a> = SumIo<'a>;
    fn on_path(&self, _: &Ctx, path: SumIo<'_>, mut merged: SumIo<'_>) -> Result<JoinStep, NodeError> {
        let seen = merged.joined().copied().unwrap_or(0);
        let a = path.a().copied().unwrap_or(0);
        if seen == 0 {
            merged.set_a(a);
        } else if let Some(total) = merged.a_mut() {
            *total += a;
        }
        merged.set_joined(seen + 1);
        Ok(JoinStep::Wait)
    }
}

struct Pass;
impl Build for Pass {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Join for Pass {
    type Io<'a> = Empty<'a>;
    fn on_path(&self, _: &Ctx, _: Empty<'_>, _: Empty<'_>) -> Result<JoinStep, NodeError> {
        Ok(JoinStep::Wait)
    }
}

/// Continues on the first path and cancels the rest.
struct First;
impl Build for First {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Join for First {
    type Io<'a> = Empty<'a>;
    fn on_path(&self, _: &Ctx, _: Empty<'_>, _: Empty<'_>) -> Result<JoinStep, NodeError> {
        Ok(JoinStep::Done)
    }
}

#[derive(Deserialize)]
struct PickConfig {
    labels: Vec<String>,
}
fn leak(labels: Vec<String>) -> Vec<&'static str> {
    labels.into_iter().map(|l| &*Box::leak(l.into_boxed_str())).collect()
}

struct Pick(&'static str);
impl Build for Pick {
    type Config = PickConfig;
    fn build(c: PickConfig) -> Result<Self, BuildError> {
        leak(c.labels).first().copied().map(Self).ok_or_else(|| BuildError::msg("need one label"))
    }
}
impl Branch for Pick {
    type Io<'a> = Empty<'a>;
    fn choose(&self, _: &Ctx, _: Empty<'_>) -> Result<&'static str, NodeError> {
        Ok(self.0)
    }
}

struct MultiPick(Vec<&'static str>);
impl Build for MultiPick {
    type Config = PickConfig;
    fn build(c: PickConfig) -> Result<Self, BuildError> {
        Ok(Self(leak(c.labels)))
    }
}
impl MultiBranch for MultiPick {
    type Io<'a> = Empty<'a>;
    fn choose(&self, _: &Ctx, _: Empty<'_>) -> Result<Vec<&'static str>, NodeError> {
        Ok(self.0.clone())
    }
}

struct Show;
impl Build for Show {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Terminal for Show {
    type Io<'a> = Empty<'a>;
    fn respond(&self, _: &Ctx, _: Empty<'_>, e: &PlanError) -> Result<Reply, NodeError> {
        Ok(Reply::error(e.kind, format!("{}: {}", e.node, e.message)))
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct FaultConfig {
    fail: bool,
    panic: bool,
}

/// A join that fails or panics on the first arrival.
struct FaultyJoin(FaultConfig);
impl Build for FaultyJoin {
    type Config = FaultConfig;
    fn build(c: FaultConfig) -> Result<Self, BuildError> {
        Ok(Self(c))
    }
}
impl Join for FaultyJoin {
    type Io<'a> = Empty<'a>;
    fn on_path(&self, _: &Ctx, _: Empty<'_>, _: Empty<'_>) -> Result<JoinStep, NodeError> {
        assert!(!self.0.panic, "join boom");
        if self.0.fail {
            return Err(NodeError::msg("join refused"));
        }
        Ok(JoinStep::Wait)
    }
}

/// Panics in every node kind that runs synchronously.
struct Boom;
impl Build for Boom {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Branch for Boom {
    type Io<'a> = Empty<'a>;
    fn choose(&self, _: &Ctx, _: Empty<'_>) -> Result<&'static str, NodeError> {
        panic!("branch boom")
    }
}
impl MultiBranch for Boom {
    type Io<'a> = Empty<'a>;
    fn choose(&self, _: &Ctx, _: Empty<'_>) -> Result<Vec<&'static str>, NodeError> {
        panic!("multibranch boom")
    }
}
impl Encode for Boom {
    type Io<'a> = Empty<'a>;
    fn encode(&self, _: &Ctx, _: Empty<'_>) -> Result<Reply, NodeError> {
        panic!("encode boom")
    }
}
impl Terminal for Boom {
    type Io<'a> = Empty<'a>;
    fn respond(&self, _: &Ctx, _: Empty<'_>, _: &PlanError) -> Result<Reply, NodeError> {
        panic!("terminal boom")
    }
}

/// Gives up on the whole request by cancelling it, then returns Ok.
struct CancelRequest;
impl Build for CancelRequest {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Module for CancelRequest {
    type Io<'a> = Empty<'a>;
    async fn call(&self, cx: &Ctx, _: Empty<'_>) -> Result<(), NodeError> {
        cx.cancellation().cancel();
        Ok(())
    }
}

/// Computes `k` on the blocking pool, or panics there.
struct Blocking(FaultConfig);
impl Build for Blocking {
    type Config = FaultConfig;
    fn build(c: FaultConfig) -> Result<Self, BuildError> {
        Ok(Self(c))
    }
}
impl Module for Blocking {
    type Io<'a> = KeysOut<'a>;
    async fn call(&self, cx: &Ctx, mut io: KeysOut<'_>) -> Result<(), NodeError> {
        let panic = self.0.panic;
        let value = cx
            .blocking(move || {
                assert!(!panic, "blocking boom");
                (1..=10).sum::<i64>()
            })
            .await?;
        io.set_k(value);
        Ok(())
    }
}

/// Like a service client: waits on a slow backend bounded by the request
/// deadline, and turns the cut-off into its own (non-deadline) error.
struct DeadlineBound;
impl Build for DeadlineBound {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Module for DeadlineBound {
    type Io<'a> = Empty<'a>;
    async fn call(&self, cx: &Ctx, _: Empty<'_>) -> Result<(), NodeError> {
        let slow = tokio::time::sleep(Duration::from_secs(60));
        tokio::time::timeout_at(cx.deadline(), slow).await.map_err(|_| NodeError::msg("backend call timed out"))
    }
}

fn registry() -> Arc<Registry> {
    Arc::new(
        Registry::builder()
            .decode::<Seed>("Seed")
            .module::<Write>("Write")
            .module::<CancelRequest>("CancelRequest")
            .module::<Blocking>("Blocking")
            .module::<DeadlineBound>("DeadlineBound")
            .encode::<Dump>("Dump")
            .encode::<Boom>("BoomEncode")
            .join::<SumA>("SumA")
            .join::<Pass>("Pass")
            .join::<First>("First")
            .join::<FaultyJoin>("FaultyJoin")
            .branch::<Pick>("Pick")
            .branch::<Boom>("BoomBranch")
            .multi_branch::<MultiPick>("MultiPick")
            .multi_branch::<Boom>("BoomMulti")
            .terminal::<Show>("Show")
            .terminal::<Boom>("BoomTerminal")
            .build()
            .unwrap(),
    )
}

/// A plan named `p` from links and node lines; `error` and `encode` are
/// added unless `nodes` defines them.
fn engine(header: &str, links: &[&str], nodes: &str) -> Engine {
    let links: Vec<String> = links.iter().map(|l| format!("\"{l}\"")).collect();
    let mut defaults = String::new();
    if !nodes.contains("encode ") {
        defaults.push_str("encode = { module = \"Dump\" }\n");
    }
    if !nodes.contains("error ") {
        defaults.push_str("error = { module = \"Show\" }\n");
    }
    let text = format!(
        "[plans.p]\nroot = \"decode\"\nterminal = \"error\"\n{header}\nlinks = [{}]\n[plans.p.nodes]\n{defaults}{nodes}",
        links.join(", ")
    );
    Engine::new(registry(), &PlansFile::from_toml_str(&text).unwrap()).unwrap_or_else(|e| panic!("{e}\n{text}"))
}

async fn call(e: &Engine) -> Reply {
    e.call("p", Request::in_process("")).await.unwrap()
}

fn body(r: &Reply) -> String {
    String::from_utf8(r.body.to_vec()).unwrap()
}

const GUIDE_LINKS: &[&str] = &["decode -> p1", "decode -> p2", "p1 -> join", "p2 -> join", "join -> encode"];
const GUIDE_NODES: &str = r#"
    decode = { module = "Seed",  config = { set = ["a", "e", "f"], value = 1 } }
    p1     = { module = "Write", config = { set = ["a", "e", "f"], value = 2 } }
    p2     = { module = "Write", config = { set = ["a", "e", "g"], value = 3, delay_ms = 1 } }
    join   = { module = "SumA" }
"#;

#[tokio::test]
async fn the_guide_merge_example_with_drop() {
    // base a=1 e=1 f=1; path 1 sets a=2 e=2 f=2; path 2 sets a=3 e=3 g=3.
    // a: the join wrote it (2+3). e: changed on both paths, dropped.
    // f: changed on path 1 only. g: added on path 2 only.
    let e = engine("on_conflict = \"drop\"", GUIDE_LINKS, GUIDE_NODES);
    let reply = call(&e).await;
    assert_eq!(reply.status, Status::Ok, "{}", body(&reply));
    assert_eq!(body(&reply), "a=5 f=2 g=3");
}

#[tokio::test]
async fn the_guide_merge_example_with_error() {
    let e = engine("on_conflict = \"error\"", GUIDE_LINKS, GUIDE_NODES);
    let reply = call(&e).await;
    assert_eq!(reply.status, Status::Error(ErrorKind::MergeConflict));
    assert_eq!(body(&reply), "join: keys changed on more than one path: e");
}

#[tokio::test(start_paused = true)]
async fn fork_paths_run_concurrently() {
    let e = engine(
        "",
        &["decode -> x", "decode -> y", "x -> join", "y -> join", "join -> encode"],
        r#"
        decode = { module = "Seed" }
        x      = { module = "Write", config = { set = ["m"], value = 1, delay_ms = 50 } }
        y      = { module = "Write", config = { set = ["n"], value = 2, delay_ms = 50 } }
        join   = { module = "Pass" }
        "#,
    );
    let started = Instant::now();
    let reply = call(&e).await;
    assert_eq!(body(&reply), "m=1 n=2");
    assert_eq!(started.elapsed(), Duration::from_millis(50), "not 100 ms: the paths overlapped");
}

/// The guide's stage 3 shape: a fork whose second path branches, all meeting at one join.
fn stage3(label: &str) -> Engine {
    engine(
        "",
        &[
            "decode -> time",
            "decode -> branch",
            "branch -> horoscope : horoscope",
            "branch -> stock : stock",
            "time -> join",
            "horoscope -> join",
            "stock -> join",
            "join -> encode",
        ],
        &format!(
            r#"
            decode    = {{ module = "Seed" }}
            time      = {{ module = "Write", config = {{ set = ["k"], value = 1, delay_ms = 5 }} }}
            branch    = {{ module = "Pick", config = {{ labels = ["{label}"] }} }}
            horoscope = {{ module = "Write", config = {{ set = ["m"], value = 2 }} }}
            stock     = {{ module = "Write", config = {{ set = ["n"], value = 3 }} }}
            join      = {{ module = "Pass" }}
            "#
        ),
    )
}

#[tokio::test(start_paused = true)]
async fn a_branch_runs_only_the_chosen_path_and_the_join_waits_for_the_live_ones() {
    assert_eq!(body(&call(&stage3("horoscope")).await), "k=1 m=2");
    assert_eq!(body(&call(&stage3("stock")).await), "k=1 n=3");
}

#[tokio::test]
async fn an_unknown_branch_label_fails_the_request() {
    let reply = call(&stage3("weather")).await;
    assert_eq!(reply.status, Status::Error(ErrorKind::Module));
    assert!(body(&reply).starts_with("branch: returned unknown label `weather`"), "{}", body(&reply));
}

#[tokio::test]
async fn a_multibranch_runs_the_chosen_set() {
    let e = engine(
        "",
        &[
            "decode -> mb",
            "mb -> x : x",
            "mb -> y : y",
            "mb -> z : z",
            "x -> join",
            "y -> join",
            "z -> join",
            "join -> encode",
        ],
        r#"
        decode = { module = "Seed" }
        mb     = { module = "MultiPick", config = { labels = ["x", "z", "x"] } }
        x      = { module = "Write", config = { set = ["k"], value = 1 } }
        y      = { module = "Write", config = { set = ["m"], value = 2 } }
        z      = { module = "Write", config = { set = ["n"], value = 3 } }
        join   = { module = "Pass" }
        "#,
    );
    assert_eq!(body(&call(&e).await), "k=1 n=3");
}

#[tokio::test(start_paused = true)]
async fn a_join_that_is_done_cancels_the_remaining_paths() {
    let e = engine(
        "",
        &["decode -> fast", "decode -> slow", "fast -> join", "slow -> join", "join -> encode"],
        r#"
        decode = { module = "Seed" }
        fast   = { module = "Write", config = { set = ["k"], value = 1, delay_ms = 10 } }
        slow   = { module = "Write", config = { set = ["m"], value = 2, delay_ms = 500, counter = "done-slow" } }
        join   = { module = "First" }
        "#,
    );
    let started = Instant::now();
    let reply = call(&e).await;
    assert_eq!(body(&reply), "k=1");
    assert_eq!(started.elapsed(), Duration::from_millis(10));
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(count("done-slow"), 0, "the slow path was cancelled, not left running");
}

#[tokio::test]
async fn panics_are_isolated() {
    let e = engine(
        "",
        &["decode -> x", "x -> encode"],
        r#"
        decode = { module = "Seed" }
        x      = { module = "Write", config = { panic = true } }
        "#,
    );
    let reply = call(&e).await;
    assert_eq!(reply.status, Status::Error(ErrorKind::Panic));
    assert_eq!(body(&reply), "x: panicked: boom");

    let e = engine("", &["decode -> encode"], r#"decode = { module = "Seed", config = { panic = true } }"#);
    let reply = call(&e).await;
    assert_eq!(reply.status, Status::Error(ErrorKind::Panic));
    assert_eq!(body(&reply), "decode: panicked: decode boom");
}

#[tokio::test(start_paused = true)]
async fn a_failing_path_cancels_its_siblings() {
    let e = engine(
        "",
        &["decode -> bad", "decode -> slow", "bad -> join", "slow -> join", "join -> encode"],
        r#"
        decode = { module = "Seed" }
        bad    = { module = "Write", config = { fail = true, delay_ms = 5 } }
        slow   = { module = "Write", config = { set = ["m"], delay_ms = 100, counter = "sibling-slow" } }
        join   = { module = "Pass" }
        "#,
    );
    let started = Instant::now();
    let reply = call(&e).await;
    assert_eq!(body(&reply), "bad: failed on purpose");
    assert_eq!(started.elapsed(), Duration::from_millis(5));
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(count("sibling-slow"), 0);
}

#[tokio::test(start_paused = true)]
async fn the_deadline_covers_parallel_paths() {
    let e = engine(
        "time_budget_ms = 30",
        &["decode -> x", "decode -> y", "x -> join", "y -> join", "join -> encode"],
        r#"
        decode = { module = "Seed" }
        x      = { module = "Write", config = { delay_ms = 10 } }
        y      = { module = "Write", config = { delay_ms = 100 } }
        join   = { module = "Pass" }
        "#,
    );
    let started = Instant::now();
    let reply = call(&e).await;
    assert_eq!(reply.status, Status::Error(ErrorKind::Deadline), "{}", body(&reply));
    assert_eq!(started.elapsed(), Duration::from_millis(30));
}

#[tokio::test(start_paused = true)]
async fn detached_paths_do_not_delay_the_reply_and_are_tracked() {
    let e = engine(
        "time_budget_ms = 1000",
        &["decode -> main", "decode -> side", "main -> encode", "decode -> broken"],
        r#"
        decode = { module = "Seed" }
        main   = { module = "Write", config = { set = ["k"], value = 7 } }
        side   = { module = "Write", config = { delay_ms = 200, counter = "detached-side" } }
        broken = { module = "Write", config = { fail = true } }
        "#,
    );
    let started = Instant::now();
    let reply = call(&e).await;
    assert_eq!(reply.status, Status::Ok, "a detached failure does not fail the request: {}", body(&reply));
    assert_eq!(body(&reply), "k=7");
    assert_eq!(started.elapsed(), Duration::ZERO, "the reply did not wait for the detached path");
    assert_eq!(e.detached_running(), 1);
    e.wait_detached().await;
    assert_eq!(count("detached-side"), 1);
    assert_eq!(started.elapsed(), Duration::from_millis(200));
}

#[tokio::test]
async fn a_branch_into_a_dead_end_is_reported() {
    let e = engine(
        "",
        &["decode -> b", "b -> go : go", "b -> stop : stop", "go -> encode"],
        r#"
        decode = { module = "Seed" }
        b      = { module = "Pick", config = { labels = ["stop"] } }
        go     = { module = "Write" }
        stop   = { module = "Write" }
        "#,
    );
    let reply = call(&e).await;
    assert_eq!(reply.status, Status::Error(ErrorKind::Internal));
    assert_eq!(body(&reply), "p: no running path can reach the encode node");
}

/// decode forks into A and B; A writes k then forks into A1 and A2, which meet
/// at j1; j1 and B meet at j2. A's write of k happens before the inner fork,
/// so it must still count as A's change when j2 merges against decode's map.
fn nested(b_sets: &str) -> Engine {
    engine(
        "on_conflict = \"drop\"",
        &[
            "decode -> a",
            "decode -> b",
            "a -> a1",
            "a -> a2",
            "a1 -> j1",
            "a2 -> j1",
            "j1 -> j2",
            "b -> j2",
            "j2 -> encode",
        ],
        &format!(
            r#"
            decode = {{ module = "Seed" }}
            a      = {{ module = "Write", config = {{ set = ["k"], value = 1 }} }}
            a1     = {{ module = "Write", config = {{ set = ["m"], value = 2, delay_ms = 2 }} }}
            a2     = {{ module = "Write", config = {{ set = ["n"], value = 3 }} }}
            j1     = {{ module = "Pass" }}
            b      = {{ module = "Write", config = {{ set = [{b_sets}], value = 4, delay_ms = 1 }} }}
            j2     = {{ module = "Pass" }}
            "#
        ),
    )
}

#[tokio::test(start_paused = true)]
async fn nested_forks_merge_against_the_right_base() {
    assert_eq!(body(&call(&nested("\"p\"")).await), "k=1 m=2 n=3 p=4");
    // B also writes k: a conflict at j2 between A's prefix write and B.
    assert_eq!(body(&call(&nested("\"p\", \"k\"")).await), "m=2 n=3 p=4");
}

#[tokio::test(start_paused = true)]
async fn a_join_can_fork_again() {
    let e = engine(
        "",
        &[
            "decode -> x",
            "decode -> y",
            "x -> j1",
            "y -> j1",
            "j1 -> u",
            "j1 -> v",
            "u -> j2",
            "v -> j2",
            "j2 -> encode",
        ],
        r#"
        decode = { module = "Seed" }
        x      = { module = "Write", config = { set = ["a"], value = 1 } }
        y      = { module = "Write", config = { set = ["e"], value = 2 } }
        j1     = { module = "Pass" }
        u      = { module = "Write", config = { set = ["f"], value = 3, delay_ms = 3 } }
        v      = { module = "Write", config = { set = ["g"], value = 4 } }
        j2     = { module = "Pass" }
        "#,
    );
    assert_eq!(body(&call(&e).await), "a=1 e=2 f=3 g=4");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawned_forks_give_the_same_result() {
    let nodes = GUIDE_NODES.replace(
        r#"decode = { module = "Seed",  config = { set = ["a", "e", "f"], value = 1 } }"#,
        r#"decode = { module = "Seed",  config = { set = ["a", "e", "f"], value = 1 }, spawn = true }"#,
    );
    let e = engine("on_conflict = \"drop\"", GUIDE_LINKS, &nodes);
    for _ in 0..50 {
        assert_eq!(body(&call(&e).await), "a=5 f=2 g=3");
    }
}

// ---- Failure paths ----

#[tokio::test]
async fn a_join_that_fails_or_panics_fails_the_request() {
    let links = ["decode -> x", "decode -> y", "x -> join", "y -> join", "join -> encode"];
    let nodes = |join: &str| {
        format!(
            "decode = {{ module = \"Seed\" }}\nx = {{ module = \"Write\", config = {{ set = [\"k\"], value = 1 }} }}\n\
             y = {{ module = \"Write\", config = {{ set = [\"m\"], value = 2 }} }}\njoin = {join}\n"
        )
    };
    let e = engine("", &links, &nodes(r#"{ module = "FaultyJoin", config = { fail = true } }"#));
    let reply = call(&e).await;
    assert_eq!(reply.status, Status::Error(ErrorKind::Module));
    assert_eq!(body(&reply), "join: join refused");

    let e = engine("", &links, &nodes(r#"{ module = "FaultyJoin", config = { panic = true } }"#));
    let reply = call(&e).await;
    assert_eq!(reply.status, Status::Error(ErrorKind::Panic));
    assert_eq!(body(&reply), "join: panicked: join boom");
}

#[tokio::test]
async fn panics_in_branches_and_the_encoder_are_isolated() {
    let e = engine(
        "",
        &["decode -> pick", "pick -> x : x", "pick -> y : y", "x -> join", "y -> join", "join -> encode"],
        r#"
        decode = { module = "Seed" }
        pick   = { module = "BoomBranch" }
        x      = { module = "Write" }
        y      = { module = "Write" }
        join   = { module = "Pass" }
        "#,
    );
    let reply = call(&e).await;
    assert_eq!(reply.status, Status::Error(ErrorKind::Panic));
    assert_eq!(body(&reply), "pick: panicked: branch boom");

    let e = engine(
        "",
        &["decode -> pick", "pick -> x : x", "pick -> y : y", "x -> join", "y -> join", "join -> encode"],
        r#"
        decode = { module = "Seed" }
        pick   = { module = "BoomMulti" }
        x      = { module = "Write" }
        y      = { module = "Write" }
        join   = { module = "Pass" }
        "#,
    );
    assert_eq!(body(&call(&e).await), "pick: panicked: multibranch boom");

    let e = engine("", &["decode -> encode"], "decode = { module = \"Seed\" }\nencode = { module = \"BoomEncode\" }\n");
    let reply = call(&e).await;
    assert_eq!(reply.status, Status::Error(ErrorKind::Panic));
    assert_eq!(body(&reply), "encode: panicked: encode boom");
}

#[tokio::test]
async fn a_panicking_terminal_is_an_engine_error() {
    let e = engine(
        "",
        &["decode -> x", "x -> encode"],
        r#"
        decode = { module = "Seed" }
        x      = { module = "Write", config = { fail = true } }
        error  = { module = "BoomTerminal" }
        "#,
    );
    match e.call("p", Request::in_process("")).await.unwrap_err() {
        forkline_engine::EngineError::TerminalFailed { error, terminal, message } => {
            assert_eq!(&*terminal, "error");
            assert_eq!(message, "panicked: terminal boom");
            assert_eq!((&*error.node, error.kind), ("x", ErrorKind::Module));
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn detached_paths_are_cancelled_at_the_deadline() {
    let e = engine(
        "time_budget_ms = 100",
        &["decode -> main", "decode -> side", "main -> encode"],
        r#"
        decode = { module = "Seed" }
        main   = { module = "Write", config = { set = ["k"], value = 1 } }
        side   = { module = "Write", config = { delay_ms = 5000, counter = "detached-too-slow" } }
        "#,
    );
    let started = Instant::now();
    assert_eq!(body(&call(&e).await), "k=1");
    assert_eq!(e.detached_running(), 1);
    e.wait_detached().await;
    assert_eq!(started.elapsed(), Duration::from_millis(100), "stopped at the request's deadline");
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(count("detached-too-slow"), 0, "the slow path never finished");
    assert_eq!(e.detached_running(), 0);
}

#[tokio::test]
async fn a_multibranch_that_picks_nothing_fails_the_request() {
    let e = engine(
        "",
        &["decode -> mb", "mb -> x : x", "mb -> y : y", "x -> join", "y -> join", "join -> encode"],
        r#"
        decode = { module = "Seed" }
        mb     = { module = "MultiPick", config = { labels = [] } }
        x      = { module = "Write" }
        y      = { module = "Write" }
        join   = { module = "Pass" }
        "#,
    );
    let reply = call(&e).await;
    assert_eq!(reply.status, Status::Error(ErrorKind::Internal));
    assert_eq!(body(&reply), "p: no running path can reach the encode node");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn paths_that_finish_after_a_done_join_are_discarded() {
    // Both spawned paths finish at once, so the second one often arrives
    // after the join has fired. It must never be merged.
    let e = engine(
        "",
        &["decode -> x", "decode -> y", "x -> join", "y -> join", "join -> encode"],
        r#"
        decode = { module = "Seed", spawn = true }
        x      = { module = "Write", config = { set = ["k"], value = 1 } }
        y      = { module = "Write", config = { set = ["m"], value = 2 } }
        join   = { module = "First" }
        "#,
    );
    for _ in 0..200 {
        let text = body(&call(&e).await);
        assert!(text == "k=1" || text == "m=2", "exactly one path is merged: {text}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_deadline_between_two_nodes_stops_the_next_one() {
    let e = engine(
        "time_budget_ms = 50",
        &["decode -> a", "a -> b", "b -> encode"],
        r#"
        decode = { module = "Seed" }
        a      = { module = "Write", config = { delay_ms = 50 } }
        b      = { module = "Write", config = { set = ["k"], value = 1, counter = "after-deadline" } }
        "#,
    );
    let reply = call(&e).await;
    assert_eq!(reply.status, Status::Error(ErrorKind::Deadline));
    assert_eq!(body(&reply), "b: deadline passed before the node started");
    assert_eq!(count("after-deadline"), 0);
}

#[tokio::test(start_paused = true)]
async fn an_error_caused_by_the_deadline_is_reported_as_a_deadline() {
    // The module's own timer and the engine's fire at the same instant; the
    // module's error must still come back as a deadline, not a module error.
    let e = engine(
        "time_budget_ms = 80",
        &["decode -> call", "call -> encode"],
        "decode = { module = \"Seed\" }\ncall = { module = \"DeadlineBound\" }\n",
    );
    let reply = call(&e).await;
    assert_eq!(reply.status, Status::Error(ErrorKind::Deadline), "{}", body(&reply));
    assert_eq!(body(&reply), "call: deadline passed: backend call timed out");
}

#[tokio::test]
async fn a_request_cancelled_before_its_reply_is_reported_as_cancelled() {
    let e = engine(
        "",
        &["decode -> cancel", "cancel -> x", "x -> encode"],
        r#"
        decode = { module = "Seed" }
        cancel = { module = "CancelRequest" }
        x      = { module = "Write", config = { set = ["k"], value = 1, counter = "after-cancel" } }
        "#,
    );
    let reply = call(&e).await;
    assert_eq!(reply.status, Status::Error(ErrorKind::Cancelled));
    assert_eq!(body(&reply), "p: the request was cancelled before a reply");
    assert_eq!(count("after-cancel"), 0);
}

#[tokio::test]
async fn blocking_work_runs_off_the_runtime_and_its_panics_are_caught() {
    let e =
        engine("", &["decode -> b", "b -> encode"], "decode = { module = \"Seed\" }\nb = { module = \"Blocking\" }\n");
    assert_eq!(body(&call(&e).await), "k=55");

    let e = engine(
        "",
        &["decode -> b", "b -> encode"],
        "decode = { module = \"Seed\" }\nb = { module = \"Blocking\", config = { panic = true } }\n",
    );
    let reply = call(&e).await;
    assert_eq!(reply.status, Status::Error(ErrorKind::Internal));
    assert!(body(&reply).starts_with("b: blocking task failed:"), "{}", body(&reply));
}

#[tokio::test(start_paused = true)]
async fn dropping_a_request_cancels_its_spawned_paths() {
    // A client that disconnects drops the request's future; paths on their
    // own tasks must stop too, not run on unobserved.
    let e = Arc::new(engine(
        "",
        &["decode -> x", "decode -> y", "x -> join", "y -> join", "join -> encode"],
        r#"
        decode = { module = "Seed", spawn = true }
        x      = { module = "Write", config = { delay_ms = 200, counter = "orphan-x" } }
        y      = { module = "Write", config = { delay_ms = 200, counter = "orphan-y" } }
        join   = { module = "Pass" }
        "#,
    ));
    let request = tokio::spawn({
        let e = e.clone();
        async move { e.call("p", Request::in_process("")).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!((count("orphan-x"), count("orphan-y")), (0, 0), "spawned paths were aborted");
}
