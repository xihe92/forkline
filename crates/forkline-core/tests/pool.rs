//! Named thread pools: results, a full queue, deadline, cancellation,
//! panics, shutdown and metrics.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::time::Duration;

use forkline_core::{Ctx, ErrorKind, Pool, RequestEnv, RequestId, ServiceRegistry};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

fn cx_with(budget: Duration, cancel: CancellationToken) -> Ctx {
    Ctx::new(
        RequestId(1),
        Arc::from("p"),
        tracing::Span::none(),
        Instant::now() + budget,
        cancel,
        RequestEnv::in_process(),
    )
}

fn cx() -> Ctx {
    cx_with(Duration::from_secs(5), CancellationToken::new())
}

/// Occupy every thread of a one-thread pool until `release` is sent.
fn occupy(pool: &Pool) -> (mpsc::Sender<()>, tokio::task::JoinHandle<()>) {
    let (release, wait) = mpsc::channel::<()>();
    let started = Arc::new(Barrier::new(2));
    let (pool, gate) = (pool.clone(), started.clone());
    let handle = tokio::spawn(async move {
        pool.run(&cx(), move || {
            gate.wait();
            let _ = wait.recv();
        })
        .await
        .unwrap();
    });
    started.wait();
    (release, handle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runs_work_on_the_pool_threads_and_returns_the_value() {
    let pool = Pool::new("named", 2, 8).unwrap();
    let (value, thread) = pool.run(&cx(), || (6 * 7, std::thread::current().name().map(str::to_owned))).await.unwrap();
    assert_eq!(value, 42);
    assert!(thread.unwrap().starts_with("pool-named-"));
    assert_eq!((pool.name(), pool.threads(), pool.queue_capacity()), ("named", 2, 8));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_queue_refuses_at_once_with_overloaded() {
    let pool = Pool::new("full", 1, 1).unwrap();
    let (release, busy) = occupy(&pool);
    let queued = {
        let pool = pool.clone();
        tokio::spawn(async move { pool.run(&cx(), || 1).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await; // the second job is now in the queue
    let started = std::time::Instant::now();
    let refused = pool.run(&cx(), || 2).await.unwrap_err();
    assert_eq!(refused.kind(), ErrorKind::Overloaded);
    assert!(refused.message().contains("pool `full` is full"), "{}", refused.message());
    assert!(started.elapsed() < Duration::from_millis(50), "refused without waiting");
    release.send(()).unwrap();
    busy.await.unwrap();
    assert_eq!(queued.await.unwrap().unwrap(), 1, "the queued job still runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_deadline_ends_the_wait_and_a_job_still_queued_is_skipped() {
    let pool = Pool::new("late", 1, 4).unwrap();
    let (release, busy) = occupy(&pool);
    let ran = Arc::new(AtomicBool::new(false));
    let flag = ran.clone();
    let err = pool
        .run(&cx_with(Duration::from_millis(50), CancellationToken::new()), move || flag.store(true, Ordering::SeqCst))
        .await
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Deadline);
    release.send(()).unwrap();
    busy.await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!ran.load(Ordering::SeqCst), "a job whose request gave up is skipped, not run");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_ends_the_wait() {
    let pool = Pool::new("cancel", 1, 4).unwrap();
    let (release, busy) = occupy(&pool);
    let token = CancellationToken::new();
    let cancel = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(30)).await;
        cancel.cancel();
    });
    let err = pool.run(&cx_with(Duration::from_secs(5), token), || ()).await.unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Cancelled);
    release.send(()).unwrap();
    busy.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_already_given_up_never_reaches_the_queue() {
    let pool = Pool::new("early", 1, 1).unwrap();
    let token = CancellationToken::new();
    token.cancel();
    assert_eq!(
        pool.run(&cx_with(Duration::from_secs(5), token), || ()).await.unwrap_err().kind(),
        ErrorKind::Cancelled
    );
    let expired = cx_with(Duration::ZERO, CancellationToken::new());
    assert_eq!(pool.run(&expired, || ()).await.unwrap_err().kind(), ErrorKind::Deadline);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_panic_fails_the_job_and_the_thread_survives() {
    let pool = Pool::new("panics", 1, 4).unwrap();
    let err = pool.run(&cx(), || -> u8 { panic!("model file missing") }).await.unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Panic);
    assert!(err.message().contains("model file missing"), "{}", err.message());
    assert_eq!(pool.run(&cx(), || 7).await.unwrap(), 7, "the same single thread runs the next job");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_finishes_the_queue_then_refuses_new_jobs() {
    let pool = Pool::new("stop", 1, 4).unwrap();
    let (release, busy) = occupy(&pool);
    let queued = {
        let pool = pool.clone();
        tokio::spawn(async move { pool.run(&cx(), || "done").await })
    };
    tokio::time::sleep(Duration::from_millis(30)).await;
    let stopping = {
        let pool = pool.clone();
        tokio::task::spawn_blocking(move || pool.shutdown(Duration::from_secs(5)))
    };
    tokio::time::sleep(Duration::from_millis(30)).await;
    release.send(()).unwrap();
    busy.await.unwrap();
    assert!(stopping.await.unwrap(), "every thread finished");
    assert_eq!(queued.await.unwrap().unwrap(), "done", "the queue drains before the threads exit");
    let err = pool.run(&cx(), || ()).await.unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Internal);
    assert!(err.message().contains("shut down"), "{}", err.message());
}

#[test]
fn shutdown_gives_up_after_the_timeout() {
    let pool = Pool::new("stuck", 1, 1).unwrap();
    let (tx, rx) = mpsc::channel::<()>();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let job = {
        let pool = pool.clone();
        runtime.spawn(async move {
            let _ = pool.run(&cx(), move || rx.recv()).await;
        })
    };
    runtime.block_on(async { tokio::time::sleep(Duration::from_millis(30)).await });
    assert!(!pool.shutdown(Duration::from_millis(50)), "a job still running holds its thread");
    tx.send(()).unwrap();
    runtime.block_on(job).unwrap();
}

#[test]
fn threads_and_queue_must_be_at_least_one() {
    assert!(Pool::new("zero", 0, 1).unwrap_err().contains("at least 1"));
    assert!(Pool::new("zero", 1, 0).unwrap_err().contains("at least 1"));
}

#[test]
fn pools_are_their_own_namespace_in_the_registry() {
    let mut registry = ServiceRegistry::new().with("scoring", 7u32);
    registry.insert_pool("scoring", Pool::new("scoring", 1, 1).unwrap());
    assert_eq!(registry.get::<u32>("scoring").unwrap(), 7, "a service and a pool may share a name");
    assert_eq!(registry.pool("scoring").unwrap().name(), "scoring");
    assert_eq!(registry.len(), 1, "pools don't count as services");
    let missing = registry.pool("scorng").unwrap_err().to_string();
    assert_eq!(missing, "pool `scorng` is not configured; add `[pools.scorng]` to server.toml");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metrics_count_jobs_by_result() {
    let mut registry = prometheus_client::registry::Registry::default();
    forkline_core::pool::register_metrics(&mut registry);
    let pool = Pool::new("metered", 1, 1).unwrap();
    pool.run(&cx(), || ()).await.unwrap();
    let _ = pool.run(&cx(), || -> () { panic!("boom") }).await;
    let mut text = String::new();
    prometheus_client::encoding::text::encode(&mut text, &registry).unwrap();
    for line in [
        r#"forkline_pool_threads{pool="metered"} 1"#,
        r#"forkline_pool_jobs_total{pool="metered",result="done"} 1"#,
        r#"forkline_pool_jobs_total{pool="metered",result="panicked"} 1"#,
        r#"forkline_pool_wait_seconds_count{pool="metered"} 2"#,
    ] {
        assert!(text.contains(line), "missing `{line}` in:\n{text}");
    }
}
