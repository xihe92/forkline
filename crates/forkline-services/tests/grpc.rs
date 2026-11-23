//! The tonic-backed gRPC client against a real tonic server.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use forkline_core::node::BoxFuture;
use forkline_core::{Ctx, RequestEnv, RequestId};
use forkline_services::grpc::{BytesCodec, GrpcSettings};
use forkline_services::{GrpcClient, ServiceErrorKind, StackConfig};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

fn cx(budget: Duration) -> Ctx {
    Ctx::new(
        RequestId(1),
        Arc::from("test"),
        tracing::Span::none(),
        Instant::now() + budget,
        CancellationToken::new(),
        RequestEnv::in_process(),
    )
}

/// Parse a `grpc-timeout` value (`<digits><H|M|S|m|u|n>`).
fn parse(value: &str) -> Duration {
    let (digits, unit) = value.split_at(value.len() - 1);
    let n: u64 = digits.parse().unwrap();
    match unit {
        "H" => Duration::from_secs(n * 3600),
        "M" => Duration::from_secs(n * 60),
        "S" => Duration::from_secs(n),
        "m" => Duration::from_millis(n),
        "u" => Duration::from_micros(n),
        "n" => Duration::from_nanos(n),
        _ => panic!("bad grpc-timeout unit in {value:?}"),
    }
}

/// Records each call's `grpc-timeout`. Every call waits `delay`; the first
/// `fail_first` then fail with `code`, later calls echo.
#[derive(Clone)]
struct Backend {
    seen: Arc<Mutex<Vec<Option<Duration>>>>,
    fail_first: usize,
    code: tonic::Code,
    delay: Duration,
}

impl Backend {
    fn echo() -> Self {
        Self::failing(0, tonic::Code::Ok, Duration::ZERO)
    }

    fn failing(fail_first: usize, code: tonic::Code, delay: Duration) -> Self {
        Self { seen: Arc::default(), fail_first, code, delay }
    }
}

impl tonic::server::NamedService for Backend {
    const NAME: &'static str = "test.Backend";
}

impl tower::Service<http::Request<tonic::body::Body>> for Backend {
    type Response = http::Response<tonic::body::Body>;
    type Error = Infallible;
    type Future = BoxFuture<'static, Result<Self::Response, Infallible>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<tonic::body::Body>) -> Self::Future {
        let unary = self.clone();
        Box::pin(async move { Ok(tonic::server::Grpc::new(BytesCodec).unary(unary, req).await) })
    }
}

impl tonic::server::UnaryService<Bytes> for Backend {
    type Response = Bytes;
    type Future = BoxFuture<'static, Result<tonic::Response<Bytes>, tonic::Status>>;

    fn call(&mut self, request: tonic::Request<Bytes>) -> Self::Future {
        let timeout = request.metadata().get("grpc-timeout").map(|v| parse(v.to_str().unwrap()));
        let attempt = {
            let mut seen = self.seen.lock().unwrap();
            seen.push(timeout);
            seen.len()
        };
        let (fail, code, delay) = (attempt <= self.fail_first, self.code, self.delay);
        Box::pin(async move {
            tokio::time::sleep(delay).await;
            if fail {
                return Err(tonic::Status::new(code, "backend says no"));
            }
            Ok(tonic::Response::new(request.into_inner()))
        })
    }
}

async fn serve(backend: Backend) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(
        tonic::transport::Server::builder()
            .add_service(backend)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
    );
    addr
}

fn client(addr: SocketAddr, stack: &StackConfig) -> GrpcClient {
    let settings = GrpcSettings { url: format!("http://{addr}"), connect_timeout_ms: Some(500) };
    GrpcClient::from_settings("backend", &settings, stack).unwrap()
}

#[tokio::test]
async fn the_time_left_is_sent_as_grpc_timeout() {
    let backend = Backend::echo();
    let seen = backend.seen.clone();
    let addr = serve(backend).await;
    let c = client(addr, &StackConfig::default());

    let reply = c.call(&cx(Duration::from_secs(2)), "/test.Backend/Echo", "ping").await.unwrap();
    assert_eq!(reply, Bytes::from("ping"));

    let sent = seen.lock().unwrap()[0].expect("grpc-timeout was sent");
    assert!(sent <= Duration::from_secs(2), "never more than the budget: {sent:?}");
    assert!(sent > Duration::from_millis(1500), "about the whole budget is left: {sent:?}");
}

#[tokio::test]
async fn a_retry_sends_what_is_left_not_the_original_budget() {
    let delay = Duration::from_millis(300);
    let backend = Backend::failing(1, tonic::Code::Unavailable, delay);
    let seen = backend.seen.clone();
    let addr = serve(backend).await;
    let c = client(addr, &StackConfig { retries: 1, ..Default::default() });

    let reply = c.call(&cx(Duration::from_secs(2)), "/test.Backend/Echo", "ping").await.unwrap();
    assert_eq!(reply, Bytes::from("ping"));

    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2, "one failure, one retry");
    let (first, second) = (seen[0].unwrap(), seen[1].unwrap());
    assert!(first - second >= delay, "the retry's timeout is smaller by the time spent: {first:?} then {second:?}");
}

#[tokio::test]
async fn backend_statuses_map_to_error_kinds() {
    let cases = [
        (tonic::Code::DeadlineExceeded, ServiceErrorKind::Timeout),
        (tonic::Code::Cancelled, ServiceErrorKind::Cancelled),
        (tonic::Code::ResourceExhausted, ServiceErrorKind::Unavailable),
        (tonic::Code::NotFound, ServiceErrorKind::Status),
        (tonic::Code::Internal, ServiceErrorKind::Status),
    ];
    for (code, kind) in cases {
        let backend = Backend::failing(usize::MAX, code, Duration::ZERO);
        let seen = backend.seen.clone();
        let c = client(serve(backend).await, &StackConfig { retries: 1, ..Default::default() });
        let err = c.call(&cx(Duration::from_secs(2)), "/test.Backend/Echo", "x").await.unwrap_err();
        assert_eq!(err.kind, kind, "{code:?}: {err}");
        let attempts = seen.lock().unwrap().len();
        assert_eq!(attempts, if err.is_retryable() { 2 } else { 1 }, "{code:?}");
    }
}

#[tokio::test]
async fn a_slow_backend_hits_the_request_deadline() {
    let addr = serve(Backend::failing(0, tonic::Code::Ok, Duration::from_secs(5))).await;
    let c = client(addr, &StackConfig::default());
    let started = std::time::Instant::now();
    let err = c.call(&cx(Duration::from_millis(100)), "/test.Backend/Echo", "x").await.unwrap_err();
    assert_eq!(err.kind, ServiceErrorKind::Deadline, "{err}");
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn a_backend_giving_up_at_our_deadline_is_a_deadline_not_a_retryable_timeout() {
    // The backend fails with DEADLINE_EXCEEDED right as our deadline passes,
    // as a gRPC server honouring grpc-timeout does.
    let backend = Backend::failing(usize::MAX, tonic::Code::DeadlineExceeded, Duration::from_millis(100));
    let seen = backend.seen.clone();
    let c = client(serve(backend).await, &StackConfig { retries: 3, ..Default::default() });
    let err = c.call(&cx(Duration::from_millis(100)), "/test.Backend/Echo", "x").await.unwrap_err();
    assert_eq!(err.kind, ServiceErrorKind::Deadline, "{err}");
    assert_eq!(seen.lock().unwrap().len(), 1, "no retries once the deadline has passed");
}

#[tokio::test]
async fn a_cancelled_request_stops_its_call() {
    let addr = serve(Backend::failing(0, tonic::Code::Ok, Duration::from_secs(5))).await;
    let c = client(addr, &StackConfig::default());
    let cx = cx(Duration::from_secs(10));
    let cancel = cx.cancellation().clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();
    });
    let err = c.call(&cx, "/test.Backend/Echo", "x").await.unwrap_err();
    assert_eq!(err.kind, ServiceErrorKind::Cancelled);
}

#[tokio::test]
async fn unreachable_backends_and_bad_paths_fail_cleanly() {
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
    let err = client(free, &StackConfig::default())
        .call(&cx(Duration::from_secs(2)), "/test.Backend/Echo", "x")
        .await
        .unwrap_err();
    // tonic reports a refused connection as UNAVAILABLE; it is retried like a transport error.
    assert_eq!(err.kind, ServiceErrorKind::Unavailable, "{err}");
    assert!(err.is_retryable());

    let c = client(serve(Backend::echo()).await, &StackConfig::default());
    let err = c.call(&cx(Duration::from_secs(2)), "not a path", "x").await.unwrap_err();
    assert_eq!(err.kind, ServiceErrorKind::Other);
    assert!(err.message.starts_with("bad method path"), "{err}");

    let settings = GrpcSettings { url: "not a url".into(), connect_timeout_ms: None };
    assert!(GrpcClient::from_settings("x", &settings, &StackConfig::default()).unwrap_err().starts_with("invalid url"));
}
