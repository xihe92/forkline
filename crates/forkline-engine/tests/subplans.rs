//! Subplans (`cx.execute_plan`) and services resolved at build time.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use forkline_core::{
    Build, BuildContext, BuildError, Ctx, Decode, Empty, Encode, ErrorKind, Module, NoConfig, NodeError, PlanError,
    Registry, Reply, ServiceRegistry, Status, Terminal,
};
use forkline_engine::{Engine, Request};
use forkline_macros::interface;
use forkline_plan::PlansFile;
use serde::Deserialize;
use tokio::time::Instant;

interface! {
    #[forkline(crate = forkline_core)]
    pub struct BodyIo { out body: Bytes }
}
interface! {
    #[forkline(crate = forkline_core)]
    pub struct ReplyIo { in body: Bytes, out reply: Bytes }
}
interface! {
    #[forkline(crate = forkline_core)]
    pub struct OutIo { in reply: Bytes }
}

struct Dec;
impl Build for Dec {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Decode for Dec {
    type Io<'a> = BodyIo<'a>;
    fn decode(&self, _: &Ctx, body: &Bytes, mut io: BodyIo<'_>) -> Result<(), NodeError> {
        io.set_body(body.clone());
        Ok(())
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct CallConfig {
    plan: String,
    prefix: String,
    delay_ms: u64,
}

/// Runs `plan` as a subplan with the body (or, with no plan, prefixes the body).
struct Call(CallConfig);
impl Build for Call {
    type Config = CallConfig;
    fn build(c: CallConfig) -> Result<Self, BuildError> {
        Ok(Self(c))
    }
}
impl Module for Call {
    type Io<'a> = ReplyIo<'a>;
    async fn call(&self, cx: &Ctx, mut io: ReplyIo<'_>) -> Result<(), NodeError> {
        tokio::time::sleep(Duration::from_millis(self.0.delay_ms)).await;
        let body = io.body().cloned().unwrap_or_default();
        let out = if self.0.plan.is_empty() {
            Bytes::from(format!("{}{}", self.0.prefix, String::from_utf8_lossy(&body)))
        } else {
            let reply = cx.execute_plan(&self.0.plan, body).await?.into_result()?;
            Bytes::from(format!("{}{}", self.0.prefix, String::from_utf8_lossy(&reply)))
        };
        io.set_reply(out);
        Ok(())
    }
}

struct Enc;
impl Build for Enc {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Encode for Enc {
    type Io<'a> = OutIo<'a>;
    fn encode(&self, _: &Ctx, io: OutIo<'_>) -> Result<Reply, NodeError> {
        Ok(Reply::ok(io.reply().cloned().unwrap_or_default()))
    }
}

struct Term;
impl Build for Term {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Terminal for Term {
    type Io<'a> = Empty<'a>;
    fn respond(&self, _: &Ctx, _: Empty<'_>, e: &PlanError) -> Result<Reply, NodeError> {
        Ok(Reply::error(e.kind, format!("{}: {}", e.node, e.message)))
    }
}

/// A service handle a module resolves while it is built.
#[derive(Clone)]
struct Greeting(&'static str);

#[derive(Deserialize)]
struct GreetConfig {
    service: String,
}
struct Greet(Greeting);
impl Build for Greet {
    type Config = GreetConfig;
    fn build_with(c: GreetConfig, cx: &BuildContext<'_>) -> Result<Self, BuildError> {
        Ok(Self(cx.service::<Greeting>(&c.service)?))
    }
}
impl Module for Greet {
    type Io<'a> = ReplyIo<'a>;
    async fn call(&self, cx: &Ctx, mut io: ReplyIo<'_>) -> Result<(), NodeError> {
        // Request-time lookup works too.
        let again = cx.service::<Greeting>("greeting")?;
        let body = String::from_utf8_lossy(io.body().map_or(&[][..], |b| b.as_ref())).into_owned();
        io.set_reply(Bytes::from(format!("{} {} ({})", self.0.0, body, again.0)));
        Ok(())
    }
}

fn registry() -> Arc<Registry> {
    Arc::new(
        Registry::builder()
            .decode::<Dec>("Dec")
            .module::<Call>("Call")
            .module::<Greet>("Greet")
            .encode::<Enc>("Enc")
            .terminal::<Term>("Term")
            .build()
            .unwrap(),
    )
}

fn plan(name: &str, budget: u64, step: &str) -> String {
    format!(
        "[plans.{name}]\nroot = \"decode\"\nterminal = \"error\"\ntime_budget_ms = {budget}\n\
         links = [\"decode -> step\", \"step -> encode\"]\n[plans.{name}.nodes]\n\
         decode = {{ module = \"Dec\" }}\nencode = {{ module = \"Enc\" }}\nerror = {{ module = \"Term\" }}\nstep = {step}\n"
    )
}

fn engine(text: &str) -> Engine {
    Engine::new(registry(), &PlansFile::from_toml_str(text).unwrap()).unwrap()
}

async fn body(e: &Engine, plan: &str) -> (Status, String) {
    let r = e.call(plan, Request::in_process("x")).await.unwrap();
    (r.status, String::from_utf8(r.body.to_vec()).unwrap())
}

#[tokio::test]
async fn a_module_can_run_a_subplan() {
    let text = plan("outer", 1000, r#"{ module = "Call", config = { plan = "inner", prefix = "outer:" } }"#)
        + &plan("inner", 1000, r#"{ module = "Call", config = { prefix = "inner:" } }"#);
    assert_eq!(body(&engine(&text), "outer").await, (Status::Ok, "outer:inner:x".into()));
}

#[tokio::test]
async fn subplan_failures_reach_the_caller() {
    let text = plan("outer", 1000, r#"{ module = "Call", config = { plan = "missing" } }"#);
    let (status, text) = body(&engine(&text), "outer").await;
    assert_eq!(status, Status::Error(ErrorKind::Internal));
    assert_eq!(text, "step: unknown plan `missing`");
}

#[tokio::test(start_paused = true)]
async fn the_callers_deadline_bounds_the_subplan() {
    let text = plan("outer", 50, r#"{ module = "Call", config = { plan = "inner" } }"#)
        + &plan("inner", 10_000, r#"{ module = "Call", config = { delay_ms = 200 } }"#);
    let started = Instant::now();
    let (status, text) = body(&engine(&text), "outer").await;
    assert_eq!(status, Status::Error(ErrorKind::Deadline), "{text}");
    assert_eq!(started.elapsed(), Duration::from_millis(50));
}

#[tokio::test]
async fn runaway_recursion_is_stopped() {
    let text = plan("loop", 1000, r#"{ module = "Call", config = { plan = "loop" } }"#);
    let (status, text) = body(&engine(&text), "loop").await;
    assert_eq!(status, Status::Error(ErrorKind::Internal));
    assert!(text.contains("nesting limit of 8"), "{text}");
}

#[tokio::test]
async fn services_are_resolved_at_build_and_at_request_time() {
    let text = plan("p", 1000, r#"{ module = "Greet", config = { service = "greeting" } }"#);
    let file = PlansFile::from_toml_str(&text).unwrap();
    let services = Arc::new(ServiceRegistry::new().with("greeting", Greeting("hello")));
    let e = Engine::with_services(registry(), &file, services).unwrap();
    assert_eq!(body(&e, "p").await, (Status::Ok, "hello x (hello)".into()));

    let err = Engine::new(registry(), &file).unwrap_err().to_string();
    assert_eq!(err, "plan `p`, node `step`: module `Greet`: service `greeting` is not registered");

    let wrong = Arc::new(ServiceRegistry::new().with("greeting", 5u32));
    let err = Engine::with_services(registry(), &file, wrong).unwrap_err().to_string();
    assert!(err.contains("is a `u32`, not a"), "{err}");
}

#[tokio::test]
async fn a_reload_can_replace_the_services() {
    let text = plan("p", 1000, r#"{ module = "Greet", config = { service = "greeting" } }"#);
    let file = PlansFile::from_toml_str(&text).unwrap();
    let e =
        Engine::with_services(registry(), &file, Arc::new(ServiceRegistry::new().with("greeting", Greeting("hello"))))
            .unwrap();
    assert_eq!(body(&e, "p").await, (Status::Ok, "hello x (hello)".into()));

    let version =
        e.reload_with_services(&file, Arc::new(ServiceRegistry::new().with("greeting", Greeting("hi")))).unwrap();
    assert_eq!(version, 2);
    assert_eq!(body(&e, "p").await, (Status::Ok, "hi x (hi)".into()), "modules were rebuilt with the new service");

    // Dropping a service a plan still needs is rejected; the running snapshot stays.
    let err = e.reload_with_services(&file, Arc::new(ServiceRegistry::new())).unwrap_err().to_string();
    assert_eq!(err, "plan `p`, node `step`: module `Greet`: service `greeting` is not registered");
    assert_eq!(e.snapshot().version, 2);
    assert_eq!(body(&e, "p").await, (Status::Ok, "hi x (hi)".into()));

    // A plain reload keeps the services it has.
    assert_eq!(e.reload(&file).unwrap(), 3);
    assert_eq!(body(&e, "p").await, (Status::Ok, "hi x (hi)".into()));
}
