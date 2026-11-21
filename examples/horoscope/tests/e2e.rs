//! Phase 3 gate: the horoscope/stock/time application end to end against
//! wiremock backends, through the engine and through the real HTTP server.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use forkline::engine::{Engine, Request};
use forkline::plan::PlansFile;
use forkline::{ErrorKind, Status};
use forkline_server::{Bound, BoundServer, ServerConfig};
use forkline_services::{ServiceKinds, ServicesFile};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Three backends on one mock server, with the example's service settings.
async fn backends() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/time"))
        .respond_with(ResponseTemplate::new(200).set_body_string("12:00"))
        .mount(&server)
        .await;
    server
}

fn engine(server: &MockServer, extra_services: &str) -> Engine {
    let plans = PlansFile::from_toml_str(include_str!("../plans.toml")).unwrap();
    let url = server.uri();
    let services = format!(
        r#"
        [services.time]
        kind = "http"
        url = "{url}"
        timeout_ms = 30
        [services.horoscope]
        kind = "http"
        url = "{url}"
        timeout_ms = 40
        retries = 1
        [services.stock]
        kind = "http"
        url = "{url}"
        timeout_ms = 40
        {extra_services}
        "#
    );
    let services = ServiceKinds::default().build(&ServicesFile::from_toml_str(&services).unwrap()).unwrap();
    Engine::with_services(Arc::new(forkline_horoscope::registry().unwrap()), &plans, Arc::new(services)).unwrap()
}

async fn query(e: &Engine, body: &str) -> (Status, Value) {
    let reply = e.call("qp1", Request::in_process(body.to_owned())).await.unwrap();
    assert_eq!(reply.headers.get("content-type").unwrap(), "application/json");
    (reply.status, serde_json::from_slice(&reply.body).unwrap())
}

#[tokio::test]
async fn horoscope_requests_fork_branch_and_join() {
    let server = backends().await;
    Mock::given(path("/horoscope/leo"))
        .respond_with(ResponseTemplate::new(200).set_body_string("bold"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/stock/ACME")).respond_with(ResponseTemplate::new(200)).expect(0).mount(&server).await;

    let e = engine(&server, "");
    let (status, json) = query(&e, r#"{"want": "horoscope", "sign": "leo"}"#).await;
    assert_eq!(status, Status::Ok, "{json}");
    assert_eq!(json["want"], "horoscope");
    assert_eq!(json["time"], "12:00");
    assert_eq!(json["result"], "bold");
    assert_eq!(json["summary"], "result: bold\ntime: 12:00");
}

#[tokio::test]
async fn stock_requests_take_the_other_branch() {
    let server = backends().await;
    Mock::given(path("/stock/ACME"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ACME: 42.00"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/horoscope/leo")).respond_with(ResponseTemplate::new(200)).expect(0).mount(&server).await;

    let (status, json) = query(&engine(&server, ""), r#"{"want": "stock", "ticker": "ACME"}"#).await;
    assert_eq!(status, Status::Ok, "{json}");
    assert_eq!(json["result"], "ACME: 42.00");
}

#[tokio::test]
async fn bad_requests_are_400s_with_a_json_body() {
    let server = backends().await;
    let e = engine(&server, "");
    let (status, json) = query(&e, "not json").await;
    assert_eq!(status, Status::Error(ErrorKind::BadRequest));
    assert_eq!(json["error"], "bad_request");
    assert_eq!(json["node"], "request");

    let (status, json) = query(&e, r#"{"want": "weather"}"#).await;
    assert_eq!(status, Status::Error(ErrorKind::BadRequest));
    assert_eq!(json["node"], "branch");
}

#[tokio::test]
async fn a_flaky_backend_is_retried() {
    let server = backends().await;
    Mock::given(path("/horoscope/leo")).respond_with(ResponseTemplate::new(503)).up_to_n_times(1).mount(&server).await;
    Mock::given(path("/horoscope/leo"))
        .respond_with(ResponseTemplate::new(200).set_body_string("bold"))
        .mount(&server)
        .await;
    let (status, json) = query(&engine(&server, ""), r#"{"sign": "leo"}"#).await;
    assert_eq!(status, Status::Ok, "the horoscope service retries once: {json}");
    assert_eq!(json["result"], "bold");
}

#[tokio::test]
async fn a_slow_backend_hits_its_service_timeout() {
    let server = backends().await;
    Mock::given(path("/stock/ACME"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(300)))
        .mount(&server)
        .await;
    let (status, json) = query(&engine(&server, ""), r#"{"want": "stock", "ticker": "ACME"}"#).await;
    assert_eq!(status, Status::Error(ErrorKind::Module), "{json}");
    assert_eq!(json["node"], "stock");
    assert_eq!(json["message"], "service `stock`: timeout: attempt timed out");
}

#[tokio::test]
async fn the_plan_budget_caps_everything() {
    let server = MockServer::start().await;
    // No service timeout on `time`, so only the plan's 80 ms budget stops it.
    Mock::given(path("/time"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(500)))
        .mount(&server)
        .await;
    Mock::given(path("/horoscope/leo"))
        .respond_with(ResponseTemplate::new(200).set_body_string("bold"))
        .mount(&server)
        .await;
    let plans = PlansFile::from_toml_str(include_str!("../plans.toml")).unwrap();
    let services = format!(
        "[services.time]\nkind = \"http\"\nurl = \"{u}\"\n[services.horoscope]\nkind = \"http\"\nurl = \"{u}\"\n\
         [services.stock]\nkind = \"http\"\nurl = \"{u}\"\n",
        u = server.uri()
    );
    let services = ServiceKinds::default().build(&ServicesFile::from_toml_str(&services).unwrap()).unwrap();
    let e =
        Engine::with_services(Arc::new(forkline_horoscope::registry().unwrap()), &plans, Arc::new(services)).unwrap();
    let started = std::time::Instant::now();
    let (status, json) = query(&e, r#"{"sign": "leo"}"#).await;
    assert!(
        matches!(status, Status::Error(ErrorKind::Deadline) | Status::Error(ErrorKind::Module)),
        "{status:?} {json}"
    );
    assert!(json["message"].as_str().unwrap().contains("deadline"), "{json}");
    assert!(started.elapsed() < Duration::from_millis(300), "stopped near the 80 ms budget");
}

#[tokio::test]
async fn the_check_fails_without_services() {
    let plans = PlansFile::from_toml_str(include_str!("../plans.toml")).unwrap();
    let err = Engine::new(Arc::new(forkline_horoscope::registry().unwrap()), &plans).unwrap_err().to_string();
    assert_eq!(err.lines().count(), 3, "{err}");
    assert!(err.contains("node `time`: module `TimeLookup`: service `time` is not registered"), "{err}");
}

#[tokio::test]
async fn through_the_http_server() {
    let server = backends().await;
    Mock::given(path("/horoscope/leo"))
        .respond_with(ResponseTemplate::new(200).set_body_string("bold"))
        .mount(&server)
        .await;
    let config =
        ServerConfig::from_toml_str("[[http]]\naddr = \"127.0.0.1:0\"\nroutes = { \"/query\" = \"qp1\" }\n").unwrap();
    let bound = BoundServer::bind(&config, Arc::new(engine(&server, ""))).await.unwrap();
    let Bound::Http(addr) = bound.bound()[0] else { panic!("no http listener") };
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(bound.run(shutdown.clone()));

    let client = reqwest::Client::new();
    let res = client.post(format!("http://{addr}/query")).body(r#"{"sign":"leo"}"#).send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "application/json");
    let json: Value = serde_json::from_str(&res.text().await.unwrap()).unwrap();
    assert_eq!(json["result"], "bold");

    let res = client.post(format!("http://{addr}/query")).body("{").send().await.unwrap();
    assert_eq!(res.status(), 400);

    shutdown.cancel();
    task.await.unwrap().unwrap();
}
