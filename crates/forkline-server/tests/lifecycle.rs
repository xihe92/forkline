//! Shutdown on every protocol, detached paths at shutdown, reloads that
//! would break a listener, and `server.toml` validation.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use common::{detached_plan, grpc_call, marks, plans, read_frame, registry, send_frame, start};
use forkline_engine::Engine;
use forkline_server::{BoundServer, Hooks, Reloader, ServerConfig, ServerError};
use tonic::Code;

#[tokio::test]
async fn the_drain_timeout_cuts_off_grpc_and_tcp_requests_too() {
    let engine = Arc::new(Engine::new(registry(), &plans(&[("p", "", 0, 60_000)])).unwrap());
    let s = start(
        "[[grpc]]\naddr = \"127.0.0.1:0\"\nmethods = { P = \"p\" }\n[[tcp]]\naddr = \"127.0.0.1:0\"\nplan = \"p\"\n\
         [shutdown]\ndrain_timeout_ms = 200\n",
        engine,
        Hooks::default(),
    )
    .await;
    let grpc = tokio::spawn(grpc_call(s.grpc(), "P", "30000|never"));
    let mut conn = s.tcp().await;
    send_frame(&mut conn, b"30000|never").await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let started = Instant::now();
    s.shutdown.cancel();

    let status = grpc.await.unwrap().unwrap_err();
    assert_eq!((status.code(), status.message()), (Code::Unavailable, "shutting down"));
    assert_eq!(read_frame(&mut conn).await.unwrap(), (4, Bytes::from("shutting down")));
    drop(conn);
    s.finish().await;
    assert!(started.elapsed() < Duration::from_millis(1500), "{:?}", started.elapsed());
}

#[tokio::test]
async fn shutdown_waits_for_detached_paths() {
    let engine = Arc::new(Engine::new(registry(), &detached_plan("d", "drained-side", 300, 5000)).unwrap());
    let s = start("[[http]]\naddr = \"127.0.0.1:0\"\nroutes = { \"/d\" = \"d\" }\n", engine.clone(), Hooks::default())
        .await;
    let started = Instant::now();
    let res = reqwest::Client::new().post(s.http("/d")).body("x").send().await.unwrap();
    assert_eq!(res.text().await.unwrap(), "x", "the reply did not wait for the side path");
    assert!(started.elapsed() < Duration::from_millis(250));
    assert_eq!(engine.detached_running(), 1);

    s.stop().await;
    assert_eq!(marks("drained-side"), 1, "shutdown let the detached path finish");
    assert!(started.elapsed() >= Duration::from_millis(300));
    assert_eq!(engine.detached_running(), 0);
}

#[tokio::test]
async fn shutdown_abandons_detached_paths_at_the_drain_timeout() {
    let engine = Arc::new(Engine::new(registry(), &detached_plan("d", "abandoned-side", 10_000, 60_000)).unwrap());
    let s = start(
        "[[http]]\naddr = \"127.0.0.1:0\"\nroutes = { \"/d\" = \"d\" }\n[shutdown]\ndrain_timeout_ms = 200\n",
        engine.clone(),
        Hooks::default(),
    )
    .await;
    reqwest::Client::new().post(s.http("/d")).body("x").send().await.unwrap();
    let started = Instant::now();
    // The abandoned path is still running when the server returns, by design.
    s.stop_unchecked().await;
    assert!(started.elapsed() < Duration::from_millis(1000), "{:?}", started.elapsed());
    assert_eq!(marks("abandoned-side"), 0);
}

#[tokio::test]
async fn a_reload_that_drops_a_served_plan_is_rejected() {
    let engine = Arc::new(Engine::new(registry(), &plans(&[("p", "-v1", 0, 1000)])).unwrap());
    let next = Arc::new(std::sync::Mutex::new(plans(&[("q", "", 0, 1000)])));
    let reloader: Reloader = {
        let (engine, next) = (engine.clone(), next.clone());
        Arc::new(move || engine.reload(&next.lock().unwrap()).map_err(|d| d.to_string()))
    };
    let s = start(
        "[[http]]\naddr = \"127.0.0.1:0\"\nroutes = { \"/p\" = \"p\" }\n[admin]\naddr = \"127.0.0.1:0\"\n",
        engine.clone(),
        Hooks { reloader: Some(reloader), metrics: None },
    )
    .await;
    let c = reqwest::Client::new();

    // The new config renames `p` to `q`: rejected, and `/p` keeps working.
    let res = c.post(s.admin("/admin/reload")).send().await.unwrap();
    assert_eq!(res.status(), 400);
    let body: serde_json::Value = serde_json::from_str(&res.text().await.unwrap()).unwrap();
    assert_eq!(
        body["errors"],
        serde_json::json!(["plan `p`: is served by a listener, so the new configuration must keep it"])
    );
    assert_eq!(c.post(s.http("/p")).body("x").send().await.unwrap().text().await.unwrap(), "x-v1");

    // Keeping `p` and adding `q` is fine.
    *next.lock().unwrap() = plans(&[("p", "-v2", 0, 1000), ("q", "", 0, 1000)]);
    assert_eq!(c.post(s.admin("/admin/reload")).send().await.unwrap().status(), 200);
    assert_eq!(c.post(s.http("/p")).body("x").send().await.unwrap().text().await.unwrap(), "x-v2");
    drop(c);
    s.stop().await;
}

#[tokio::test]
async fn server_config_problems_are_reported_together() {
    let engine = Arc::new(Engine::new(registry(), &plans(&[("p", "", 0, 1000)])).unwrap());
    let config = ServerConfig::from_toml_str(
        r#"
        [[http]]
        addr = "127.0.0.1:0"
        routes = { "no-slash" = "p", "/ghost" = "ghost" }
        max_body_bytes = 0
        idle_timeout_ms = 0
        [[grpc]]
        addr = "127.0.0.1:0"
        methods = { "a/b" = "p", "" = "p", Ghost = "ghost" }
        idle_timeout_ms = 0
        [[tcp]]
        addr = "127.0.0.1:0"
        plan = "ghost"
        max_frame_bytes = 0
        idle_timeout_ms = 0
        [admin]
        addr = "127.0.0.1:0"
        allow = ["10.0.0.0/33", "not-an-ip"]
        [admission]
        red = 5
        green = 5
        "#,
    )
    .unwrap();
    let err = match BoundServer::bind(&config, engine.clone()).await {
        Err(ServerError::Config(e)) => e,
        Err(other) => panic!("unexpected error {other}"),
        Ok(_) => panic!("an invalid config was accepted"),
    };
    let problems: Vec<&str> = err.split("; ").collect();
    let expected = [
        "tcp 127.0.0.1:0: unknown plan `ghost`",
        "tcp 127.0.0.1:0: max_frame_bytes and max_in_flight_per_connection must be > 0",
        "tcp 127.0.0.1:0: idle_timeout_ms must be > 0",
        "admission: need 0 < green < red (green = 5, red = 5)",
        "admin: `10.0.0.0/33` is not an IP address or network",
        "admin: `not-an-ip` is not an IP address or network",
        "http 127.0.0.1:0: max_body_bytes must be > 0",
        "http 127.0.0.1:0: header_read_timeout_ms and idle_timeout_ms must be > 0",
        "http 127.0.0.1:0: route `/ghost` names unknown plan `ghost`",
        "http 127.0.0.1:0: route `no-slash` must start with `/`",
        "grpc 127.0.0.1:0: idle_timeout_ms must be > 0",
        "grpc 127.0.0.1:0: `` is not a valid method name",
        "grpc 127.0.0.1:0: method `Ghost` names unknown plan `ghost`",
        "grpc 127.0.0.1:0: `a/b` is not a valid method name",
    ];
    assert_eq!(problems, expected);

    let err = match BoundServer::bind(&ServerConfig::default(), engine).await {
        Err(e) => e.to_string(),
        Ok(_) => panic!("a config without listeners was accepted"),
    };
    assert!(err.contains("no listeners configured"), "{err}");
}

#[test]
fn server_config_files_report_where_they_failed() {
    let err = ServerConfig::load("/nonexistent/server.toml").unwrap_err();
    assert!(err.starts_with("cannot read /nonexistent/server.toml:"), "{err}");

    let dir = std::env::temp_dir().join(format!("forkline-cfg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("server.toml");
    std::fs::write(&path, "[[http]]\naddr = \"127.0.0.1:0\"\nroutes = {}\nbogus = 1\n").unwrap();
    let err = ServerConfig::load(&path).unwrap_err();
    assert!(err.starts_with(&format!("{}:", path.display())) && err.contains("unknown field `bogus`"), "{err}");
    std::fs::remove_dir_all(dir).unwrap();
}
