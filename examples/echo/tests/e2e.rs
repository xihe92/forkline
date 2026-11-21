//! Phase 1 gate: the echo plan serves over real HTTP and gRPC sockets.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use forkline::engine::Engine;
use forkline::plan::PlansFile;
use forkline_server::grpc::BytesCodec;
use forkline_server::{Bound, BoundServer, ServerConfig};
use tokio_util::sync::CancellationToken;

struct Running {
    http: SocketAddr,
    grpc: SocketAddr,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<Result<(), forkline_server::ServerError>>,
}

async fn start() -> Running {
    let plans = PlansFile::from_toml_str(include_str!("../plans.toml")).unwrap();
    let engine = Arc::new(Engine::new(Arc::new(forkline_echo::registry().unwrap()), &plans).unwrap());
    let config = ServerConfig::from_toml_str(
        r#"
        [[http]]
        addr = "127.0.0.1:0"
        routes = { "/echo" = "echo", "/slow" = "slow" }
        [[grpc]]
        addr = "127.0.0.1:0"
        methods = { Echo = "echo", Slow = "slow" }
        "#,
    )
    .unwrap();
    let server = BoundServer::bind(&config, engine).await.unwrap();
    let (mut http, mut grpc) = (None, None);
    for b in server.bound() {
        match *b {
            Bound::Http(a) => http = Some(a),
            Bound::Grpc(a) => grpc = Some(a),
            _ => {}
        }
    }
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(server.run(shutdown.clone()));
    Running { http: http.unwrap(), grpc: grpc.unwrap(), shutdown, task }
}

async fn grpc_call(addr: SocketAddr, method: &str, body: &'static [u8]) -> Result<Bytes, tonic::Status> {
    let channel = tonic::transport::Endpoint::from_shared(format!("http://{addr}")).unwrap().connect().await.unwrap();
    let mut client = tonic::client::Grpc::new(channel);
    client.ready().await.unwrap();
    let request = tonic::Request::new(Bytes::from_static(body));
    let path = http::uri::PathAndQuery::try_from(format!("/forkline.v1.Plans/{method}")).unwrap();
    client.unary(request, path, BytesCodec).await.map(tonic::Response::into_inner)
}

#[tokio::test]
async fn echo_over_http() {
    let s = start().await;
    let client = reqwest::Client::new();

    let res = client.post(format!("http://{}/echo", s.http)).body("hello").send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.text().await.unwrap(), "echo: hello");

    let res = client.post(format!("http://{}/echo", s.http)).body("").send().await.unwrap();
    assert_eq!(res.status(), 400, "bad request from the module maps to 400");
    assert_eq!(res.text().await.unwrap(), "empty body");

    let res = client.post(format!("http://{}/missing", s.http)).body("x").send().await.unwrap();
    assert_eq!(res.status(), 404);

    s.shutdown.cancel();
    s.task.await.unwrap().unwrap();
}

#[tokio::test]
async fn echo_over_grpc() {
    let s = start().await;

    let reply = grpc_call(s.grpc, "Echo", b"hello").await.unwrap();
    assert_eq!(reply, Bytes::from("echo: hello"));

    let err = grpc_call(s.grpc, "Echo", b"").await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);
    assert_eq!(err.message(), "empty body");

    let err = grpc_call(s.grpc, "Nope", b"x").await.unwrap_err();
    assert_eq!(err.code(), tonic::Code::Unimplemented);

    s.shutdown.cancel();
    s.task.await.unwrap().unwrap();
}

/// Calls the gRPC adapter as a tower service with a raw `grpc-timeout` header,
/// so the server's own deadline handling is observed (a tonic client would
/// enforce the same timeout locally and race it).
async fn raw_grpc_status(method: &str, timeout: Option<&str>) -> (String, Bytes) {
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let plans = PlansFile::from_toml_str(include_str!("../plans.toml")).unwrap();
    let engine = Arc::new(Engine::new(Arc::new(forkline_echo::registry().unwrap()), &plans).unwrap());
    let methods = [("Slow".to_string(), "slow".to_string())].into_iter().collect();
    let svc = forkline_server::grpc::GrpcPlans::new(engine, &methods);

    let mut frame = vec![0u8];
    frame.extend_from_slice(&1u32.to_be_bytes());
    frame.push(b'x');
    let mut req = http::Request::post(format!("http://localhost/forkline.v1.Plans/{method}"))
        .header("content-type", "application/grpc")
        .header("te", "trailers");
    if let Some(t) = timeout {
        req = req.header("grpc-timeout", t);
    }
    let req = req.body(http_body_util::Full::new(Bytes::from(frame))).unwrap();

    let res = svc.oneshot(req).await.unwrap();
    let mut status = res.headers().get("grpc-status").map(|v| v.to_str().unwrap().to_owned());
    let collected = res.into_body().collect().await.unwrap();
    if let Some(trailers) = collected.trailers() {
        status = status.or_else(|| trailers.get("grpc-status").map(|v| v.to_str().unwrap().to_owned()));
    }
    let data = collected.to_bytes();
    (status.unwrap_or_default(), data)
}

#[tokio::test]
async fn grpc_timeout_caps_the_plan_budget() {
    // The slow plan takes 50 ms and has a 1 s budget: a 10 ms grpc-timeout wins.
    let started = std::time::Instant::now();
    let (status, _) = raw_grpc_status("Slow", Some("10m")).await;
    assert_eq!(status, "4", "DEADLINE_EXCEEDED");
    assert!(started.elapsed() < Duration::from_millis(45), "stopped at the caller's deadline, not after 50 ms");

    // Without a timeout it completes: status 0 and the framed reply `x`.
    let (status, data) = raw_grpc_status("Slow", None).await;
    assert_eq!(status, "0");
    assert_eq!(&data[5..], b"x");
}

#[tokio::test]
async fn config_must_name_loaded_plans() {
    let plans = PlansFile::from_toml_str(include_str!("../plans.toml")).unwrap();
    let engine = Arc::new(Engine::new(Arc::new(forkline_echo::registry().unwrap()), &plans).unwrap());
    let config =
        ServerConfig::from_toml_str("[[http]]\naddr = \"127.0.0.1:0\"\nroutes = { \"echo\" = \"nope\" }\n").unwrap();
    let err = BoundServer::bind(&config, engine).await.unwrap_err().to_string();
    assert!(err.contains("must start with `/`") && err.contains("unknown plan `nope`"), "{err}");
}
