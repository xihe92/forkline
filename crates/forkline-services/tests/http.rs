//! The reqwest-backed HTTP client against a real (wiremock) server.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use forkline_core::{Ctx, RequestEnv, RequestId};
use forkline_services::http::HttpSettings;
use forkline_services::{HttpClient, ServiceErrorKind, StackConfig};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

fn client(server: &MockServer, stack: StackConfig) -> HttpClient {
    let settings = HttpSettings { url: server.uri(), connect_timeout_ms: Some(500), pool_max_idle_per_host: None };
    HttpClient::from_settings("backend", &settings, &stack).unwrap()
}

#[tokio::test]
async fn get_and_post() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/horoscope/leo"))
        .respond_with(ResponseTemplate::new(200).set_body_string("bold"))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/echo"))
        .respond_with(ResponseTemplate::new(201).set_body_string("made"))
        .mount(&server)
        .await;
    let c = client(&server, StackConfig::default());
    let cx = cx(Duration::from_secs(5));
    assert_eq!(c.get(&cx, "/horoscope/leo").await.unwrap().text().unwrap(), "bold");
    let res = c.post(&cx, "echo", "x").await.unwrap();
    assert_eq!(res.status, 201);
    let missing = c.get(&cx, "/nope").await.unwrap();
    assert_eq!(missing.status, 404, "4xx is a response, not an error");
    assert_eq!(missing.error_for_status().unwrap_err().kind, ServiceErrorKind::Status);
}

#[tokio::test]
async fn a_503_is_retried() {
    let server = MockServer::start().await;
    Mock::given(path("/flaky")).respond_with(ResponseTemplate::new(503)).up_to_n_times(1).mount(&server).await;
    Mock::given(path("/flaky")).respond_with(ResponseTemplate::new(200).set_body_string("ok")).mount(&server).await;
    let c = client(&server, StackConfig { retries: 1, ..Default::default() });
    assert_eq!(c.get(&cx(Duration::from_secs(5)), "/flaky").await.unwrap().text().unwrap(), "ok");

    let c = client(&server, StackConfig::default());
    Mock::given(path("/down")).respond_with(ResponseTemplate::new(503)).mount(&server).await;
    let err = c.get(&cx(Duration::from_secs(5)), "/down").await.unwrap_err();
    assert_eq!(err.kind, ServiceErrorKind::Unavailable);
    assert_eq!(err.to_string(), "service `backend`: unavailable: HTTP 503 Service Unavailable");
}

#[tokio::test]
async fn slow_backends_hit_the_timeout_or_the_deadline() {
    let server = MockServer::start().await;
    Mock::given(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(300)))
        .mount(&server)
        .await;

    let c = client(&server, StackConfig { timeout_ms: Some(50), ..Default::default() });
    let started = std::time::Instant::now();
    let err = c.get(&cx(Duration::from_secs(5)), "/slow").await.unwrap_err();
    assert_eq!(err.kind, ServiceErrorKind::Timeout);
    assert!(started.elapsed() < Duration::from_millis(250));

    let c = client(&server, StackConfig::default());
    let err = c.get(&cx(Duration::from_millis(50)), "/slow").await.unwrap_err();
    assert_eq!(err.kind, ServiceErrorKind::Deadline, "the request deadline caps the call");
}

#[tokio::test]
async fn connection_failures_are_transport_errors() {
    // Port 9 (discard) on localhost is almost never listening.
    let settings =
        HttpSettings { url: "http://127.0.0.1:9/".into(), connect_timeout_ms: Some(200), pool_max_idle_per_host: None };
    let c = HttpClient::from_settings("gone", &settings, &StackConfig::default()).unwrap();
    let err = c.get(&cx(Duration::from_secs(2)), "/").await.unwrap_err();
    assert_eq!(err.kind, ServiceErrorKind::Transport, "{err}");
}

#[tokio::test]
async fn bad_gateway_and_gateway_timeout_are_retried_but_500_is_not() {
    let server = MockServer::start().await;
    for (route, status) in [("/b502", 502), ("/b504", 504)] {
        Mock::given(path(route)).respond_with(ResponseTemplate::new(status)).up_to_n_times(1).mount(&server).await;
        Mock::given(path(route)).respond_with(ResponseTemplate::new(200).set_body_string("ok")).mount(&server).await;
    }
    Mock::given(path("/b500")).respond_with(ResponseTemplate::new(500)).expect(1).mount(&server).await;
    let c = client(&server, StackConfig { retries: 1, ..Default::default() });
    let cx = cx(Duration::from_secs(5));
    assert_eq!(c.get(&cx, "/b502").await.unwrap().text().unwrap(), "ok");
    assert_eq!(c.get(&cx, "/b504").await.unwrap().text().unwrap(), "ok");
    let err = c.get(&cx, "/b500").await.unwrap_err();
    assert_eq!(err.kind, ServiceErrorKind::Status, "a 500 is the backend's answer, not a transient failure");
    assert_eq!(err.to_string(), "service `backend`: status: HTTP 500 Internal Server Error");
    // `expect(1)` is checked when the server drops: the 500 was not retried.
}

#[tokio::test]
async fn a_cancelled_request_stops_its_call() {
    let server = MockServer::start().await;
    Mock::given(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;
    let c = client(&server, StackConfig::default());
    let cx = cx(Duration::from_secs(10));
    let cancel = cx.cancellation().clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();
    });
    let started = std::time::Instant::now();
    let err = c.get(&cx, "/slow").await.unwrap_err();
    assert_eq!(err.kind, ServiceErrorKind::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(1));
}
