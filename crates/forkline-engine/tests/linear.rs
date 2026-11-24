//! Linear plan execution: success, module errors, deadlines, terminal failure, reload.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use forkline_core::{
    Build, BuildError, Ctx, Decode, Empty, Encode, ErrorKind, Module, NoConfig, NodeError, PlanError, Registry, Reply,
    Status, Terminal,
};
use forkline_engine::{Engine, EngineError, Request};
use forkline_macros::interface;
use forkline_plan::PlansFile;
use serde::Deserialize;
use tokio::time::Instant;
use tower::ServiceExt;

interface! {
    #[forkline(crate = forkline_core)]
    pub struct BodyIo { out payload: Bytes }
}
interface! {
    #[forkline(crate = forkline_core)]
    pub struct TextIo { inout text: String, in payload: Bytes }
}
interface! {
    #[forkline(crate = forkline_core)]
    pub struct OutIo { in text: String }
}

struct BodyDecoder;
impl Build for BodyDecoder {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Decode for BodyDecoder {
    type Io<'a> = BodyIo<'a>;
    fn decode(&self, _: &Ctx, body: &Bytes, mut io: BodyIo<'_>) -> Result<(), NodeError> {
        if body.as_ref() == b"bad" {
            return Err(NodeError::bad_request("body must not be `bad`"));
        }
        io.set_payload(body.clone());
        Ok(())
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct AppendConfig {
    suffix: String,
    sleep_ms: u64,
    fail: bool,
}

/// Appends `suffix` to `text` (seeding it from `payload`), optionally sleeping or failing.
struct Append(AppendConfig);
impl Build for Append {
    type Config = AppendConfig;
    fn build(c: AppendConfig) -> Result<Self, BuildError> {
        Ok(Self(c))
    }
}
impl Module for Append {
    type Io<'a> = TextIo<'a>;
    async fn call(&self, _: &Ctx, mut io: TextIo<'_>) -> Result<(), NodeError> {
        if self.0.sleep_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.0.sleep_ms)).await;
        }
        if self.0.fail {
            return Err(NodeError::msg("append failed on purpose"));
        }
        if io.text().is_none() {
            let seed = String::from_utf8_lossy(io.payload().map_or(&[][..], |b| b.as_ref())).into_owned();
            io.set_text(seed);
        }
        if let Some(text) = io.text_mut() {
            text.push_str(&self.0.suffix);
        }
        Ok(())
    }
}

struct TextEncoder;
impl Build for TextEncoder {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Encode for TextEncoder {
    type Io<'a> = OutIo<'a>;
    fn encode(&self, _: &Ctx, io: OutIo<'_>) -> Result<Reply, NodeError> {
        Ok(Reply::ok(io.text().cloned().unwrap_or_default()))
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct ErrConfig {
    fail: bool,
}
struct ErrorReply(ErrConfig);
impl Build for ErrorReply {
    type Config = ErrConfig;
    fn build(c: ErrConfig) -> Result<Self, BuildError> {
        Ok(Self(c))
    }
}
impl Terminal for ErrorReply {
    type Io<'a> = Empty<'a>;
    fn respond(&self, _: &Ctx, _: Empty<'_>, e: &PlanError) -> Result<Reply, NodeError> {
        if self.0.fail {
            return Err(NodeError::msg("terminal broke"));
        }
        Ok(Reply::error(e.kind, format!("{}: {}", e.node, e.message)))
    }
}

fn registry() -> Arc<Registry> {
    Arc::new(
        Registry::builder()
            .decode::<BodyDecoder>("BodyDecoder")
            .module::<Append>("Append")
            .encode::<TextEncoder>("TextEncoder")
            .terminal::<ErrorReply>("ErrorReply")
            .build()
            .unwrap(),
    )
}

fn plans(a: &str, b: &str, terminal: &str, budget_ms: u64) -> PlansFile {
    PlansFile::from_toml_str(&format!(
        r#"
        [plans.p]
        root = "decode"
        terminal = "error"
        time_budget_ms = {budget_ms}
        links = ["decode -> a", "a -> b", "b -> encode"]
        [plans.p.nodes]
        decode = {{ module = "BodyDecoder" }}
        a      = {{ module = "Append", config = {a} }}
        b      = {{ module = "Append", config = {b} }}
        encode = {{ module = "TextEncoder" }}
        error  = {{ module = "ErrorReply", config = {terminal} }}
        "#
    ))
    .unwrap()
}

fn engine(a: &str, b: &str) -> Engine {
    Engine::new(registry(), &plans(a, b, "{}", 100)).unwrap()
}

#[tokio::test]
async fn a_chain_runs_in_order() {
    let e = engine(r#"{ suffix = "-a" }"#, r#"{ suffix = "-b" }"#);
    let reply = e.call("p", Request::in_process("hi")).await.unwrap();
    assert_eq!(reply.status, Status::Ok);
    assert_eq!(reply.body, Bytes::from("hi-a-b"));
    support::assert_settled().await;
}

#[tokio::test]
async fn module_errors_reach_the_terminal() {
    let e = engine(r#"{ suffix = "-a" }"#, "{ fail = true }");
    let reply = e.call("p", Request::in_process("hi")).await.unwrap();
    assert_eq!(reply.status, Status::Error(ErrorKind::Module));
    assert_eq!(reply.body, Bytes::from("b: append failed on purpose"));
    support::assert_settled().await;
}

#[tokio::test]
async fn bad_requests_keep_their_kind() {
    let e = engine("{}", "{}");
    let reply = e.call("p", Request::in_process("bad")).await.unwrap();
    assert_eq!(reply.status, Status::Error(ErrorKind::BadRequest));
    assert_eq!(reply.body, Bytes::from("decode: body must not be `bad`"));
    support::assert_settled().await;
}

#[tokio::test(start_paused = true)]
async fn the_time_budget_is_enforced() {
    let e = engine("{ sleep_ms = 60 }", "{ sleep_ms = 60 }");
    let started = Instant::now();
    let reply = e.call("p", Request::in_process("hi")).await.unwrap();
    assert_eq!(reply.status, Status::Error(ErrorKind::Deadline));
    assert_eq!(reply.body, Bytes::from("b: deadline passed while the node was running"));
    assert_eq!(started.elapsed(), Duration::from_millis(100), "stopped exactly at the 100 ms budget");
    support::assert_settled().await;
}

#[tokio::test(start_paused = true)]
async fn an_earlier_inbound_deadline_wins() {
    let e = engine("{ sleep_ms = 30 }", "{}");
    let mut req = Request::in_process("hi");
    req.env.inbound_deadline = Some(req.env.received_at + Duration::from_millis(10));
    let reply = e.call("p", req).await.unwrap();
    assert_eq!(reply.status, Status::Error(ErrorKind::Deadline));
    assert!(reply.body.starts_with(b"a: "));
    support::assert_settled().await;
}

#[tokio::test]
async fn a_failing_terminal_is_an_engine_error() {
    let e = Engine::new(registry(), &plans("{ fail = true }", "{}", "{ fail = true }", 100)).unwrap();
    let err = e.call("p", Request::in_process("hi")).await.unwrap_err();
    match err {
        EngineError::TerminalFailed { error, terminal, message } => {
            assert_eq!(&*terminal, "error");
            assert_eq!(message, "terminal broke");
            assert_eq!(&*error.node, "a");
        }
        other => panic!("unexpected {other:?}"),
    }
    support::assert_settled().await;
}

#[tokio::test]
async fn unknown_plans_are_refused() {
    let e = engine("{}", "{}");
    assert!(matches!(
        e.call("nope", Request::in_process("")).await,
        Err(EngineError::UnknownPlan(p)) if p == "nope"
    ));
    support::assert_settled().await;
}

#[tokio::test]
async fn the_plan_is_a_tower_service() {
    let e = Arc::new(engine(r#"{ suffix = "!" }"#, "{}"));
    let reply = e.plan_service("p").oneshot(Request::in_process("svc")).await.unwrap();
    assert_eq!(reply.body, Bytes::from("svc!"));
    support::assert_settled().await;
}

#[tokio::test(start_paused = true)]
async fn reload_swaps_plans_and_in_flight_requests_keep_their_snapshot() {
    let e = Arc::new(engine(r#"{ suffix = "-old", sleep_ms = 20 }"#, "{}"));
    let in_flight = tokio::spawn({
        let e = e.clone();
        async move { e.call("p", Request::in_process("x")).await }
    });
    tokio::time::sleep(Duration::from_millis(5)).await;

    let version = e.reload(&plans(r#"{ suffix = "-new" }"#, "{}", "{}", 100)).unwrap();
    assert_eq!(version, 2);

    let fresh = e.call("p", Request::in_process("x")).await.unwrap();
    assert_eq!(fresh.body, Bytes::from("x-new"));
    let old = in_flight.await.unwrap().unwrap();
    assert_eq!(old.body, Bytes::from("x-old"), "the in-flight request finished on the snapshot it pinned");

    // A bad reload leaves the running snapshot in place.
    let broken = PlansFile::from_toml_str("[plans.p]\nroot = \"x\"\nterminal = \"y\"\nnodes = {}\n").unwrap();
    assert!(e.reload(&broken).is_err());
    assert_eq!(e.snapshot().version, 2);
    support::assert_settled().await;
}

#[tokio::test]
async fn rollback_returns_to_the_previous_snapshot() {
    let e = Engine::builder(registry())
        .history(2)
        .build(forkline_engine::LoadInput {
            plans: &plans(r#"{ suffix = "-v1" }"#, "{}", "{}", 100),
            services: Arc::default(),
            config_hash: 0xabc,
        })
        .unwrap();
    assert_eq!(e.snapshot().config_hash, 0xabc);
    for v in 2..=4 {
        let version = e.reload(&plans(&format!(r#"{{ suffix = "-v{v}" }}"#), "{}", "{}", 100)).unwrap();
        assert_eq!(version, v);
    }
    assert_eq!(e.history(), vec![2, 3], "only the last two are kept");
    assert_eq!(e.call("p", Request::in_process("x")).await.unwrap().body, Bytes::from("x-v4"));

    assert_eq!(e.rollback(), Some(3));
    assert_eq!(e.call("p", Request::in_process("x")).await.unwrap().body, Bytes::from("x-v3"));
    assert_eq!(e.rollback(), Some(2));
    assert_eq!(e.rollback(), None, "history exhausted");
    assert_eq!(e.snapshot().version, 2);

    // A reload after a rollback gets a fresh version number.
    assert_eq!(e.reload(&plans("{}", "{}", "{}", 100)).unwrap(), 5);
    assert_eq!(e.snapshot().config_hash, 0xabc, "reload keeps the hash it was given last");
    support::assert_settled().await;
}

#[tokio::test(start_paused = true)]
async fn late_or_long_queued_requests_are_shed_before_any_module_runs() {
    let e = engine(r#"{ fail = true }"#, "{}");

    let mut late = Request::in_process("x");
    late.env.inbound_deadline = Some(late.env.received_at);
    let err = e.call("p", late).await.unwrap_err();
    assert!(matches!(err, EngineError::Shed { reason: "deadline passed before start", .. }), "{err}");

    e.set_max_queue(Some(Duration::from_millis(10)));
    let queued = Request::in_process("x");
    tokio::time::advance(Duration::from_millis(20)).await;
    let err = e.call("p", queued).await.unwrap_err();
    assert!(matches!(err, EngineError::Shed { reason: "waited too long to start", .. }), "{err}");

    // A fresh request runs (and fails in its module, proving it ran).
    let reply = e.call("p", Request::in_process("x")).await.unwrap();
    assert_eq!(reply.status, Status::Error(ErrorKind::Module));
    support::assert_settled().await;
}

/// The same plan under another name.
fn renamed(to: &str) -> PlansFile {
    let text = format!(
        "[plans.{to}]\nroot = \"decode\"\nterminal = \"error\"\nlinks = [\"decode -> a\", \"a -> b\", \"b -> encode\"]\n\
         [plans.{to}.nodes]\ndecode = {{ module = \"BodyDecoder\" }}\na = {{ module = \"Append\" }}\n\
         b = {{ module = \"Append\" }}\nencode = {{ module = \"TextEncoder\" }}\nerror = {{ module = \"ErrorReply\" }}\n"
    );
    PlansFile::from_toml_str(&text).unwrap()
}

#[tokio::test]
async fn a_reload_or_rollback_may_not_remove_a_served_plan() {
    let e = engine("{}", "{}");
    // Before anything requires it, a plan may be renamed away and back.
    assert_eq!(e.reload(&renamed("q")).unwrap(), 2);
    assert_eq!(e.reload(&plans("{}", "{}", "{}", 100)).unwrap(), 3);

    e.require_plans(["p"]);
    let err = e.reload(&renamed("q")).unwrap_err();
    assert_eq!(err.to_string(), "plan `p`: is served by a listener, so the new configuration must keep it");
    assert_eq!(e.snapshot().version, 3, "the running snapshot stays");
    assert!(e.has_plan("p"));

    // Snapshot 2 (only `q`) is in the history; rolling back to it is refused.
    assert_eq!(e.history(), vec![1, 2]);
    assert_eq!(e.rollback(), None, "the previous snapshot lacks `p`");
    assert_eq!(e.snapshot().version, 3);
    assert_eq!(e.history(), vec![1, 2], "the refused snapshot stays in the history");
    support::assert_settled().await;
}
