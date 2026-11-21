//! Prometheus metrics through the admin listener. Its own test binary,
//! because the recorder is process-global.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::sync::Arc;

use common::{plans, registry, start};
use forkline_engine::Engine;
use forkline_server::Hooks;

#[tokio::test]
async fn metrics_are_exported() {
    // The recorder must exist before the engine so plan handles bind to it.
    let handle = forkline_server::telemetry::install_metrics().unwrap();
    let engine = Arc::new(Engine::new(registry(), &plans(&[("p", "", 0, 1000)])).unwrap());
    let s = start(
        "[[http]]\naddr = \"127.0.0.1:0\"\nroutes = { \"/p\" = \"p\" }\n[admin]\naddr = \"127.0.0.1:0\"\n",
        engine,
        Hooks { reloader: None, metrics: Some(handle) },
    )
    .await;
    let c = reqwest::Client::new();
    for body in ["a", "b", "bad"] {
        c.post(s.http("/p")).body(body).send().await.unwrap();
    }
    let text = c.get(s.admin("/metrics")).send().await.unwrap().text().await.unwrap();
    for expected in [
        r#"forkline_requests_total{plan="p",outcome="ok"} 2"#,
        r#"forkline_requests_total{plan="p",outcome="bad_request"} 1"#,
        r#"forkline_node_errors_total{plan="p",node="work",kind="bad_request"} 1"#,
        r#"forkline_request_duration_seconds_bucket{plan="p",le="0.001"}"#,
        r#"forkline_node_duration_seconds_count{plan="p",node="work"} 3"#,
        "forkline_snapshot_version 1",
    ] {
        assert!(text.contains(expected), "missing `{expected}` in:\n{text}");
    }
    assert!(text.contains("forkline_inflight_requests{listener=\"http:127.0.0.1:"), "{text}");
    s.stop().await;
}
