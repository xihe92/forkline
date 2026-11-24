//! Thread pools from `server.toml`: modules run work on them, a full queue
//! answers "overloaded" on every protocol, a plan naming a missing pool is
//! rejected at load, and `check --config` sees pools and listeners.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use common::{grpc_call, modules, read_frame, registry, send_frame, start};
use forkline_core::{Pool, ServiceRegistry};
use forkline_engine::Engine;
use forkline_plan::PlansFile;
use forkline_server::cli::App;
use forkline_server::{Hooks, ServerConfig};
use tonic::Code;

const LISTENERS: &str = "[[http]]\naddr = \"127.0.0.1:0\"\nroutes = { \"/p\" = \"p\" }\n\
     [[grpc]]\naddr = \"127.0.0.1:0\"\nmethods = { P = \"p\" }\n[[tcp]]\naddr = \"127.0.0.1:0\"\nplan = \"p\"\n";

const PLAN: &str = "[plans.p]\nroot = \"decode\"\nterminal = \"error\"\ntime_budget_ms = 3000\n\
     links = [\"decode -> work\", \"work -> encode\"]\n[plans.p.nodes]\n\
     decode = { module = \"BodyDecoder\" }\nwork = { module = \"Pooled\", config = { pool = \"calc\" } }\n\
     encode = { module = \"ReplyEncoder\" }\nerror = { module = \"ErrorReply\" }\n";

fn engine_with(pool: Option<Pool>) -> Result<Engine, String> {
    let mut services = ServiceRegistry::new();
    if let Some(pool) = pool {
        services.insert_pool("calc", pool);
    }
    Engine::with_services(registry(), &PlansFile::from_toml_str(PLAN).unwrap(), Arc::new(services))
        .map_err(|d| d.to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_pool_answers_overloaded_on_every_protocol() {
    let pool = Pool::new("calc", 1, 1).unwrap();
    let s = start(LISTENERS, Arc::new(engine_with(Some(pool)).unwrap()), Hooks::default()).await;
    let c = reqwest::Client::new();
    let slow = |body: &'static str| {
        let (c, url) = (c.clone(), s.http("/p"));
        tokio::spawn(async move { c.post(url).body(body).send().await.unwrap().text().await.unwrap() })
    };
    // One job runs on the only thread, one waits in the one-slot queue.
    let running = slow("600|a");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let queued = slow("10|b");
    tokio::time::sleep(Duration::from_millis(100)).await;

    let res = c.post(s.http("/p")).body("x").send().await.unwrap();
    assert_eq!(res.status(), 503, "HTTP: overloaded");
    assert!(res.text().await.unwrap().contains("pool `calc` is full"));
    let status = grpc_call(s.grpc(), "P", "x").await.unwrap_err();
    assert_eq!(status.code(), Code::ResourceExhausted, "gRPC: {status:?}");
    let mut tcp = s.tcp().await;
    send_frame(&mut tcp, b"x").await;
    let (code, body) = read_frame(&mut tcp).await.unwrap();
    assert_eq!(code, forkline_server::tcp::STATUS_OVERLOADED, "TCP: {}", String::from_utf8_lossy(&body));

    assert_eq!(running.await.unwrap(), "a@pool-calc-0", "the job ran on the pool's thread");
    assert_eq!(queued.await.unwrap(), "b@pool-calc-0", "the queued job ran after it");
    drop(c);
    drop(tcp);
    s.stop().await;
}

#[test]
fn a_plan_naming_a_missing_pool_is_rejected_at_load() {
    let err = engine_with(None).unwrap_err();
    assert!(err.contains("pool `calc` is not configured; add `[pools.calc]` to server.toml"), "{err}");
}

#[test]
fn server_toml_pools_are_checked_and_built() {
    let config =
        ServerConfig::from_toml_str(&format!("{LISTENERS}[pools.calc]\nthreads = 2\nqueue = 5\n[pools.io]\n")).unwrap();
    let pools = config.build_pools().unwrap();
    assert_eq!((pools["calc"].threads(), pools["calc"].queue_capacity()), (2, 5));
    let cores = std::thread::available_parallelism().unwrap().get();
    assert_eq!((pools["io"].threads(), pools["io"].queue_capacity()), (cores, 1024), "defaults");

    for (pools, problem) in [
        ("[pools.calc]\nthreads = 0\n", "pools.calc: threads and queue must be at least 1"),
        ("[pools.calc]\nqueue = 0\n", "pools.calc: threads and queue must be at least 1"),
        ("[pools.\"a b\"]\n", "pools: `a b` must be letters, digits, `_` or `-`"),
    ] {
        let config = ServerConfig::from_toml_str(&format!("{LISTENERS}{pools}")).unwrap();
        let err = config.build_pools().unwrap_err();
        assert!(err.contains(problem), "{err}");
    }
    assert!(ServerConfig::from_toml_str(&format!("{LISTENERS}[pools.calc]\nsize = 2\n")).is_err(), "unknown fields");
}

/// A scratch directory removed on drop.
struct Dir(PathBuf);
impl Dir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("forkline-pools-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn check_with_config_sees_pools_and_served_plans() {
    let dir = Dir::new("check");
    let plans = dir.write("plans.toml", PLAN);
    let app = App::new(modules());
    let check = |config: Option<&Path>| app.check_files(config, &plans, None);

    let good = dir.write("server.toml", &format!("{LISTENERS}[pools.calc]\nthreads = 1\n"));
    assert_eq!(check(Some(&good)), Ok((1, 0)));

    let problems = check(None).unwrap_err();
    assert!(problems.iter().any(|p| p.contains("pool `calc` is not configured")), "without the config: {problems:?}");

    let no_pool = dir.write("no-pool.toml", LISTENERS);
    let problems = check(Some(&no_pool)).unwrap_err();
    assert!(problems.iter().any(|p| p.contains("pool `calc` is not configured")), "{problems:?}");

    let wrong_route =
        dir.write("route.toml", &format!("{}[pools.calc]\n", LISTENERS.replace("\"/p\" = \"p\"", "\"/p\" = \"q\"")));
    let problems = check(Some(&wrong_route)).unwrap_err();
    assert!(problems.iter().any(|p| p.contains("route `/p` names unknown plan `q`")), "{problems:?}");

    let bad_pool = dir.write("bad.toml", &format!("{LISTENERS}[pools.calc]\nthreads = 0\n"));
    let problems = check(Some(&bad_pool)).unwrap_err();
    assert_eq!(problems, ["pools.calc: threads and queue must be at least 1"]);
}

#[tokio::test]
async fn pool_metrics_are_served_with_the_rest() {
    let pool = Pool::new("exported", 1, 1).unwrap();
    let cx = forkline_core::Ctx::new(
        forkline_core::RequestId(1),
        Arc::from("p"),
        tracing::Span::none(),
        tokio::time::Instant::now() + Duration::from_secs(5),
        tokio_util::sync::CancellationToken::new(),
        forkline_core::RequestEnv::in_process(),
    );
    pool.run(&cx, || ()).await.unwrap();
    let registry = forkline_server::telemetry::metrics_registry();
    let mut text = String::new();
    prometheus_client::encoding::text::encode(&mut text, &registry).unwrap();
    assert!(text.contains(r#"forkline_pool_jobs_total{pool="exported",result="done"} 1"#), "{text}");
}
