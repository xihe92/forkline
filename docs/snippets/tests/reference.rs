//! The book's reference appendix documents every config key and every
//! metric. The struct patterns below list every field without `..`, so a new
//! field doesn't compile until it is added here, and then this test fails
//! until it is documented.

#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use forkline_server::config::{
    AdminConfig, AdmissionConfig, GrpcListener, HttpListener, PoolConfig, ReloadConfig, RuntimeConfig, ServerConfig,
    ShutdownConfig, TcpListenerConfig, TelemetryConfig,
};

fn page(name: &str) -> String {
    std::fs::read_to_string(format!("{}/../src/appendix/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn example(name: &str) -> String {
    std::fs::read_to_string(format!("{}/reference/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn documented(page_text: &str, keys: &[&str]) {
    let missing: Vec<_> = keys.iter().filter(|k| !page_text.contains(&format!("| `{k}` |"))).collect();
    assert!(missing.is_empty(), "keys missing from the reference tables: {missing:?}");
}

#[test]
fn every_server_toml_key_is_documented_and_the_example_parses() {
    let config = ServerConfig::from_toml_str(&example("server.toml")).unwrap();
    config.build_pools().unwrap();
    let ServerConfig { runtime, http, grpc, telemetry, shutdown, tcp, admin, admission, reload, pools } = config;
    let RuntimeConfig { worker_threads: _, max_blocking_threads: _, slow_poll_ms: _ } = runtime;
    let HttpListener { addr: _, routes: _, max_body_bytes: _, header_read_timeout_ms: _, idle_timeout_ms: _ } =
        http.into_iter().next().unwrap();
    let GrpcListener { addr: _, methods: _, idle_timeout_ms: _ } = grpc.into_iter().next().unwrap();
    let TelemetryConfig { log_format: _, filter: _, otlp_endpoint: _, service_name: _ } = telemetry;
    let ShutdownConfig { drain_timeout_ms: _ } = shutdown;
    let TcpListenerConfig { addr: _, plan: _, max_frame_bytes: _, max_in_flight_per_connection: _, idle_timeout_ms: _ } =
        tcp.into_iter().next().unwrap();
    let AdminConfig { addr: _, allow: _ } = admin.unwrap();
    let AdmissionConfig { red: _, green: _, max_queue_ms: _ } = admission.unwrap();
    let ReloadConfig { history: _, watch_interval_ms: _ } = reload;
    let PoolConfig { threads: _, queue: _ } = pools.into_values().next().unwrap();
    documented(
        &page("server-toml.md"),
        &[
            "worker_threads",
            "max_blocking_threads",
            "slow_poll_ms",
            "addr",
            "routes",
            "max_body_bytes",
            "header_read_timeout_ms",
            "idle_timeout_ms",
            "methods",
            "log_format",
            "filter",
            "otlp_endpoint",
            "service_name",
            "drain_timeout_ms",
            "plan",
            "max_frame_bytes",
            "max_in_flight_per_connection",
            "allow",
            "red",
            "green",
            "max_queue_ms",
            "history",
            "watch_interval_ms",
            "threads",
            "queue",
        ],
    );
}

#[test]
fn every_plans_and_services_key_is_documented() {
    use forkline::plan::{NodeConfig, OnConflict, PlanConfig};
    use forkline_services::grpc::GrpcSettings;
    use forkline_services::http::HttpSettings;
    use forkline_services::{BreakerConfig, ServicesFile, StackConfig};

    let plans = forkline::plan::PlansFile::load(format!("{}/plans.toml", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let PlanConfig { root: _, terminal: _, time_budget_ms: _, on_conflict, links: _, nodes } =
        plans.plans["quote"].clone();
    let NodeConfig { module: _, rename: _, config: _, spawn: _ } = nodes["decode"].clone();
    match on_conflict {
        OnConflict::Drop | OnConflict::Error => {}
    }

    let services = ServicesFile::from_toml_str(&example("services.toml")).unwrap();
    let http = &services.services["inventory"];
    let StackConfig {
        timeout_ms: _,
        retries: _,
        retry_budget_percent: _,
        retry_backoff_ms: _,
        concurrency_limit: _,
        load_shed: _,
        breaker,
    } = http.stack.clone();
    let BreakerConfig { failures: _, open_ms: _ } = breaker.unwrap();
    let HttpSettings { url: _, connect_timeout_ms: _, pool_max_idle_per_host: _ } = http.settings().unwrap();
    let GrpcSettings { url: _, connect_timeout_ms: _ } = services.services["ranking"].settings().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let _entered = runtime.enter(); // the gRPC client needs a runtime to be built
    forkline_services::ServiceKinds::default().build_into(&services, &mut forkline::ServiceRegistry::new()).unwrap();

    documented(
        &page("plans-services-toml.md"),
        &[
            "root",
            "terminal",
            "time_budget_ms",
            "on_conflict",
            "links",
            "nodes",
            "module",
            "config",
            "rename",
            "spawn",
            "kind",
            "timeout_ms",
            "retries",
            "retry_budget_percent",
            "retry_backoff_ms",
            "concurrency_limit",
            "load_shed",
            "breaker",
            "url",
            "connect_timeout_ms",
            "pool_max_idle_per_host",
        ],
    );
    assert_eq!(StackConfig::KEYS.len(), 7, "a new layer setting: add it above and to the page");
}

/// Every metric name registered anywhere in the crates' sources: the string
/// right after `register(`, or a `forkline_allocator_*` name (a collector,
/// whose names get a `_bytes` unit suffix).
fn registered_metrics() -> Vec<String> {
    let root = format!("{}/../../crates", env!("CARGO_MANIFEST_DIR"));
    let mut names = Vec::new();
    for krate in std::fs::read_dir(root).unwrap() {
        let src = krate.unwrap().path().join("src");
        for file in std::fs::read_dir(&src).into_iter().flatten() {
            let path = file.unwrap().path();
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let mut rest = text.as_str();
            while let Some(at) = rest.find("\"forkline_") {
                let tail = &rest[at + 1..];
                let name: String = tail.chars().take_while(|c| c.is_ascii_lowercase() || *c == '_').collect();
                let before = rest[..at].trim_end();
                if before.ends_with("register(") || name.starts_with("forkline_allocator_") {
                    names.push(name);
                }
                rest = &tail[1..];
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

#[test]
fn every_metric_is_documented() {
    let names = registered_metrics();
    assert!(names.len() >= 20, "only {} metrics found: {names:?}", names.len());
    let page = page("metrics.md");
    let missing: Vec<_> = names
        .iter()
        .filter(|n| ["", "_total", "_bytes"].iter().all(|suffix| !page.contains(&format!("`{n}{suffix}`"))))
        .collect();
    assert!(missing.is_empty(), "metrics missing from the reference: {missing:?}");
}
