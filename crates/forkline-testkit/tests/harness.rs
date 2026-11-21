//! The test kit on a module with a mock service, a join, and the edge cases.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::time::Duration;

use bytes::Bytes;
use forkline_core::{
    Branch, Build, BuildContext, BuildError, Ctx, Decode, Empty, Encode, ErrorKind, Join, JoinStep, Module, NoConfig,
    NodeError, PlanError, Reply, ServiceRegistry, Status, Terminal,
};
use forkline_macros::interface;
use forkline_plan::OnConflict;
use forkline_services::{HttpClient, HttpRequest, HttpResponse, ServiceError, ServiceErrorKind};
use forkline_testkit::{Harness, TestError, inputs};
use serde::Deserialize;

interface! {
    #[forkline(crate = forkline_core)]
    pub struct LookupIo { in sign: String, out svc_response: String }
}

#[derive(Deserialize)]
struct LookupConfig {
    service: String,
}

/// Calls `GET /horoscope/{sign}` on its configured service.
struct Lookup {
    client: HttpClient,
}

impl Build for Lookup {
    type Config = LookupConfig;
    fn build_with(c: LookupConfig, cx: &BuildContext<'_>) -> Result<Self, BuildError> {
        Ok(Self { client: cx.service::<HttpClient>(&c.service)? })
    }
}

impl Module for Lookup {
    type Io<'a> = LookupIo<'a>;
    async fn call(&self, cx: &Ctx, mut io: LookupIo<'_>) -> Result<(), NodeError> {
        let sign = io.sign().cloned().ok_or_else(|| NodeError::bad_request("no sign"))?;
        let text = self.client.get(cx, format!("/horoscope/{sign}")).await?.text()?;
        io.set_svc_response(text);
        Ok(())
    }
}

fn mock(delay_ms: u64) -> HttpClient {
    HttpClient::from_service(
        "horoscope",
        tower::service_fn(move |req: HttpRequest| async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            match req.path.as_str() {
                "/horoscope/leo" => Ok(HttpResponse::ok("bold")),
                _ => Err(ServiceError::new(ServiceErrorKind::Status, "no such sign")),
            }
        }),
    )
}

#[tokio::test]
async fn a_module_with_a_mock_service() {
    let services = ServiceRegistry::new().with("horoscope", mock(0));
    let out = Harness::<Lookup>::from_config("service = \"horoscope\"", services.clone())
        .unwrap()
        .inputs(inputs! { sign: String::from("leo") })
        .run_module()
        .await
        .unwrap();
    assert_eq!(out.get::<String>("svc_response").map(String::as_str), Some("bold"));
    assert_eq!(out.changed(), ["svc_response"]);

    let err = Harness::<Lookup>::from_config("service = \"horoscope\"", services)
        .unwrap()
        .input("sign", String::from("ox"))
        .run_module()
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "module: service `horoscope`: status: no such sign");
}

#[tokio::test]
async fn a_missing_service_fails_at_build() {
    let err = Harness::<Lookup>::from_config("service = \"nope\"", ServiceRegistry::new()).unwrap_err();
    assert_eq!(err.to_string(), "build failed: service `nope` is not registered");
    let err = Harness::<Lookup>::from_config("srvice = 1", ServiceRegistry::new()).unwrap_err();
    assert!(matches!(err, TestError::Config(_)), "{err}");
}

#[tokio::test(start_paused = true)]
async fn the_budget_applies() {
    let services = ServiceRegistry::new().with("horoscope", mock(500));
    let err = Harness::<Lookup>::from_config("service = \"horoscope\"", services)
        .unwrap()
        .input("sign", String::from("leo"))
        .budget(Duration::from_millis(100))
        .run_module()
        .await
        .unwrap_err();
    // The client itself gives up at the request deadline.
    assert!(err.to_string().contains("deadline"), "{err}");
}

#[tokio::test]
async fn wrong_input_types_are_reported() {
    let services = ServiceRegistry::new().with("horoscope", mock(0));
    let err = Harness::<Lookup>::from_config("service = \"horoscope\"", services)
        .unwrap()
        .input("sign", 5u8)
        .run_module()
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "key `sign` is `alloc::string::String`, but the test gave a `u8`");
}

interface! {
    #[forkline(crate = forkline_core)]
    pub struct SumIo { inout a: i64, inout joined: i64 }
}

struct SumJoin {
    stop_after: Option<i64>,
}
impl Build for SumJoin {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self { stop_after: None })
    }
}
impl Join for SumJoin {
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
        Ok(if self.stop_after == Some(seen + 1) { JoinStep::Done } else { JoinStep::Wait })
    }
}

#[test]
fn the_guide_join_example() {
    let out = Harness::new(SumJoin { stop_after: None })
        .fork_base(inputs! { a: 1i64, e: 1i64, f: 1i64 })
        .path(inputs! { a: 2i64, e: 2i64, f: 2i64 })
        .path(inputs! { a: 3i64, e: 3i64, g: 1i64 })
        .on_conflict(OnConflict::Drop)
        .run_join()
        .unwrap();
    assert_eq!(out.conflicts, ["e"]);
    let o = &out.outputs;
    assert_eq!((o.get::<i64>("a"), o.get::<i64>("f"), o.get::<i64>("g")), (Some(&5), Some(&2), Some(&1)));
    assert_eq!(o.present(), ["a", "f", "g", "joined"]);
    assert_eq!(out.paths_seen, 2);
}

#[test]
fn a_join_that_is_done_early_ignores_later_paths() {
    let out = Harness::new(SumJoin { stop_after: Some(1) })
        .fork_base(inputs! { a: 1i64 })
        .path(inputs! { a: 2i64, f: 7i64 })
        .path(inputs! { a: 3i64, g: 9i64 })
        .run_join()
        .unwrap();
    assert_eq!(out.paths_seen, 1);
    assert_eq!(out.outputs.get::<i64>("a"), Some(&2));
    assert!(out.outputs.contains("f") && !out.outputs.contains("g"));
}

interface! {
    #[forkline(crate = forkline_core)]
    pub struct BodyIo { out body: Bytes }
}
interface! {
    #[forkline(crate = forkline_core)]
    pub struct WantIo { in want: String }
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

struct Pick;
impl Build for Pick {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Branch for Pick {
    type Io<'a> = WantIo<'a>;
    fn choose(&self, _: &Ctx, io: WantIo<'_>) -> Result<&'static str, NodeError> {
        match io.want().map(String::as_str) {
            Some("stock") => Ok("stock"),
            _ => Ok("horoscope"),
        }
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
    type Io<'a> = BodyIo<'a>;
    fn encode(&self, _: &Ctx, _: BodyIo<'_>) -> Result<Reply, NodeError> {
        Ok(Reply::ok("done"))
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
        Ok(Reply::error(e.kind, e.message.clone()))
    }
}

#[test]
fn decode_branch_encode_and_terminal() {
    let out = Harness::new(Dec).run_decode("hi").unwrap();
    assert_eq!(out.get::<Bytes>("body"), Some(&Bytes::from("hi")));
    assert_eq!(Harness::new(Pick).input("want", String::from("stock")).run_branch().unwrap(), "stock");
    assert_eq!(Harness::new(Enc).run_encode().unwrap().body, Bytes::from("done"));
    let error = PlanError { kind: ErrorKind::Deadline, node: "x".into(), message: "late".into() };
    let reply = Harness::new(Term).run_terminal(&error).unwrap();
    assert_eq!((reply.status, reply.body), (Status::Error(ErrorKind::Deadline), Bytes::from("late")));
}
