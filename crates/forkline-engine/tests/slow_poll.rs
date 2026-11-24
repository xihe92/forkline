//! The blocking detector: slow polls of async and synchronous nodes are
//! counted per plan and node, the threshold can be changed or switched off,
//! and with `FORKLINE_PANIC_ON_SLOW_POLL_MS` (50 ms in this workspace's
//! `.cargo/config.toml`) a longer poll panics naming the node.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use forkline_core::{Build, BuildError, Ctx, Decode, Encode, Module, NodeError, Registry, Reply};
use forkline_engine::{Engine, LoadInput, Request};
use forkline_macros::interface;
use forkline_plan::PlansFile;
use serde::Deserialize;

interface! {
    #[forkline(crate = forkline_core)]
    pub struct BodyIo { out n: u64 }
}
interface! {
    #[forkline(crate = forkline_core)]
    pub struct WorkIo { inout n: u64 }
}

/// Decodes the body as a number of milliseconds to block for, and blocks
/// that long itself when configured to.
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct BlockConfig {
    block_ms: u64,
}

struct Decoder(BlockConfig);
impl Build for Decoder {
    type Config = BlockConfig;
    fn build(c: BlockConfig) -> Result<Self, BuildError> {
        Ok(Self(c))
    }
}
impl Decode for Decoder {
    type Io<'a> = BodyIo<'a>;
    fn decode(&self, _: &Ctx, body: &Bytes, mut io: BodyIo<'_>) -> Result<(), NodeError> {
        std::thread::sleep(Duration::from_millis(self.0.block_ms));
        let n = std::str::from_utf8(body).ok().and_then(|s| s.parse().ok()).unwrap_or(0);
        io.set_n(n);
        Ok(())
    }
}

/// Blocks the worker thread for `n` ms with a synchronous sleep: the bug the
/// detector is for.
struct Blocker;
impl Build for Blocker {
    type Config = forkline_core::NoConfig;
    fn build(_: forkline_core::NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Module for Blocker {
    type Io<'a> = WorkIo<'a>;
    async fn call(&self, _: &Ctx, io: WorkIo<'_>) -> Result<(), NodeError> {
        let ms = io.n().copied().unwrap_or(0);
        tokio::task::yield_now().await; // a first, quick poll
        std::thread::sleep(Duration::from_millis(ms));
        Ok(())
    }
}

/// Blocks for 80 ms, then panics.
struct BlocksThenPanics;
impl Build for BlocksThenPanics {
    type Config = forkline_core::NoConfig;
    fn build(_: forkline_core::NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Module for BlocksThenPanics {
    type Io<'a> = WorkIo<'a>;
    async fn call(&self, _: &Ctx, _: WorkIo<'_>) -> Result<(), NodeError> {
        std::thread::sleep(Duration::from_millis(80));
        panic!("gave up");
    }
}

/// Waits 60 ms without blocking: longer than the 10 ms threshold and the
/// 50 ms panic limit, yet never a slow poll.
struct Sleeper;
impl Build for Sleeper {
    type Config = forkline_core::NoConfig;
    fn build(_: forkline_core::NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Module for Sleeper {
    type Io<'a> = WorkIo<'a>;
    async fn call(&self, _: &Ctx, _: WorkIo<'_>) -> Result<(), NodeError> {
        tokio::time::sleep(Duration::from_millis(60)).await;
        Ok(())
    }
}

struct Encoder;
impl Build for Encoder {
    type Config = forkline_core::NoConfig;
    fn build(_: forkline_core::NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Encode for Encoder {
    type Io<'a> = WorkIo<'a>;
    fn encode(&self, _: &Ctx, io: WorkIo<'_>) -> Result<Reply, NodeError> {
        Ok(Reply::ok(io.n().copied().unwrap_or(0).to_string()))
    }
}

struct Terminal;
impl Build for Terminal {
    type Config = forkline_core::NoConfig;
    fn build(_: forkline_core::NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl forkline_core::Terminal for Terminal {
    type Io<'a> = forkline_core::Empty<'a>;
    fn respond(&self, _: &Ctx, _: forkline_core::Empty<'_>, e: &forkline_core::PlanError) -> Result<Reply, NodeError> {
        Ok(Reply::error(e.kind, e.message.clone()))
    }
}

fn engine(plan: &str, decode_block_ms: u64, slow_poll: Option<Duration>) -> Engine {
    engine_with(plan, "Blocker", decode_block_ms, slow_poll)
}

fn engine_with(plan: &str, block_module: &str, decode_block_ms: u64, slow_poll: Option<Duration>) -> Engine {
    let registry = Registry::builder()
        .decode::<Decoder>("Decoder")
        .module::<Blocker>("Blocker")
        .module::<Sleeper>("Sleeper")
        .module::<BlocksThenPanics>("BlocksThenPanics")
        .encode::<Encoder>("Encoder")
        .terminal::<Terminal>("Terminal")
        .build()
        .unwrap();
    let text = format!(
        "[plans.{plan}]\nroot = \"decode\"\nterminal = \"error\"\ntime_budget_ms = 5000\n\
         links = [\"decode -> block\", \"block -> wait\", \"wait -> encode\"]\n[plans.{plan}.nodes]\n\
         decode = {{ module = \"Decoder\", config = {{ block_ms = {decode_block_ms} }} }}\n\
         block = {{ module = \"{block_module}\" }}\nwait = {{ module = \"Sleeper\" }}\n\
         encode = {{ module = \"Encoder\" }}\nerror = {{ module = \"Terminal\" }}\n"
    );
    let plans = PlansFile::from_toml_str(&text).unwrap();
    Engine::builder(Arc::new(registry))
        .slow_poll(slow_poll)
        .build(LoadInput { plans: &plans, services: Arc::default(), config_hash: 0 })
        .unwrap()
}

/// `forkline_node_slow_polls_total{plan, node}`.
fn slow_polls(plan: &str, node: &str) -> u64 {
    let mut registry = prometheus_client::registry::Registry::default();
    forkline_engine::metrics::register(&mut registry);
    let mut text = String::new();
    prometheus_client::encoding::text::encode(&mut text, &registry).unwrap();
    let prefix = format!("forkline_node_slow_polls_total{{plan=\"{plan}\",node=\"{node}\"}} ");
    text.lines().find_map(|l| l.strip_prefix(&prefix)).map_or(0, |n| n.trim().parse().unwrap())
}

#[tokio::test]
async fn a_module_that_blocks_is_counted_by_plan_and_node() {
    let e = engine("blocks", 0, Some(Duration::from_millis(10)));
    assert_eq!(e.slow_poll(), Some(Duration::from_millis(10)));
    let reply = e.call("blocks", Request::in_process("25")).await.unwrap();
    assert_eq!(reply.body, Bytes::from("25"), "a slow poll is reported, not failed");
    assert_eq!(slow_polls("blocks", "block"), 1);
    assert_eq!(slow_polls("blocks", "wait"), 0, "waiting without blocking is never slow");
    assert_eq!(slow_polls("blocks", "decode"), 0);
    support::assert_settled().await;
}

#[tokio::test]
async fn a_synchronous_node_that_blocks_is_counted_too() {
    let e = engine("sync", 25, Some(Duration::from_millis(10)));
    e.call("sync", Request::in_process("0")).await.unwrap();
    assert_eq!(slow_polls("sync", "decode"), 1);
    assert_eq!(slow_polls("sync", "block"), 0);
    support::assert_settled().await;
}

#[tokio::test]
async fn a_long_wait_is_not_a_slow_poll() {
    let e = engine("waits", 0, Some(Duration::from_millis(10)));
    // Blocker blocks for 0 ms; Sleeper waits 60 ms, yielding the thread.
    let started = std::time::Instant::now();
    e.call("waits", Request::in_process("0")).await.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(60));
    assert_eq!(slow_polls("waits", "wait"), 0);
    support::assert_settled().await;
}

#[tokio::test]
async fn the_threshold_can_be_raised_or_switched_off() {
    let e = engine("tuned", 0, None);
    assert_eq!(e.slow_poll(), None);
    e.call("tuned", Request::in_process("25")).await.unwrap();
    assert_eq!(slow_polls("tuned", "block"), 0, "off");
    e.set_slow_poll(Some(Duration::from_millis(40)));
    e.call("tuned", Request::in_process("25")).await.unwrap();
    assert_eq!(slow_polls("tuned", "block"), 0, "under 40 ms");
    e.set_slow_poll(Some(Duration::from_millis(10)));
    e.call("tuned", Request::in_process("25")).await.unwrap();
    assert_eq!(slow_polls("tuned", "block"), 1, "a reload is not needed");
    support::assert_settled().await;
}

#[test]
fn this_workspace_runs_its_tests_with_the_panic_threshold() {
    assert_eq!(
        std::env::var(forkline_engine::SLOW_POLL_PANIC_ENV).as_deref(),
        Ok("50"),
        "set in .cargo/config.toml; run the tests with cargo or cargo nextest from the workspace"
    );
}

#[tokio::test]
#[should_panic(expected = "slow poll: node `block` of plan `panics` held a worker thread")]
async fn in_tests_a_poll_over_the_panic_threshold_panics_naming_the_node() {
    let e = engine("panics", 0, Some(Duration::from_millis(10)));
    let _ = e.call("panics", Request::in_process("80")).await;
}

#[tokio::test]
async fn a_poll_that_panics_is_reported_as_a_panic_not_a_slow_poll() {
    // Unwinding is slow the first time in a process, and the panic is
    // reported as one already: neither counted nor, in tests, a second panic.
    let e = engine_with("gives_up", "BlocksThenPanics", 0, Some(Duration::from_millis(10)));
    let reply = e.call("gives_up", Request::in_process("0")).await.unwrap();
    assert_eq!(reply.status, forkline_core::Status::Error(forkline_core::ErrorKind::Panic));
    assert_eq!(slow_polls("gives_up", "block"), 0);
    support::assert_settled().await;
}

#[tokio::test]
async fn a_module_cut_off_by_its_deadline_was_waiting_not_blocking() {
    // Sleeper waits 60 ms; a 20 ms budget cuts it off with no final poll.
    let registry = Registry::builder()
        .decode::<Decoder>("Decoder")
        .module::<Sleeper>("Sleeper")
        .encode::<Encoder>("Encoder")
        .terminal::<Terminal>("Terminal")
        .build()
        .unwrap();
    let plans = PlansFile::from_toml_str(
        "[plans.cut]\nroot = \"decode\"\nterminal = \"error\"\ntime_budget_ms = 20\n\
         links = [\"decode -> wait\", \"wait -> encode\"]\n[plans.cut.nodes]\n\
         decode = { module = \"Decoder\" }\nwait = { module = \"Sleeper\" }\n\
         encode = { module = \"Encoder\" }\nerror = { module = \"Terminal\" }\n",
    )
    .unwrap();
    let e = Engine::builder(Arc::new(registry))
        .slow_poll(Some(Duration::from_millis(10)))
        .build(LoadInput { plans: &plans, services: Arc::default(), config_hash: 0 })
        .unwrap();
    let reply = e.call("cut", Request::in_process("0")).await.unwrap();
    assert_eq!(reply.status, forkline_core::Status::Error(forkline_core::ErrorKind::Deadline));
    assert_eq!(slow_polls("cut", "wait"), 0);
    support::assert_settled().await;
}
