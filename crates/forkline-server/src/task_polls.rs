//! Slow task polls anywhere in the server, not only in nodes.
//!
//! The engine times nodes (see `Engine::set_slow_poll`), but blocking code in
//! an adapter, a service client or anything else running on the runtime is
//! invisible to it. Built with `RUSTFLAGS="--cfg tokio_unstable"`, the
//! server's runtime also times every task poll with Tokio's poll hooks: a
//! poll longer than `[runtime] slow_poll_ms` adds to
//! `forkline_task_slow_polls_total` and logs a warning naming where the task
//! was spawned (at most one every 10 s). The nightly CI job runs the example
//! server this way under load and fails on any slow poll.

use std::sync::LazyLock;
#[cfg(tokio_unstable)]
use std::sync::atomic::{AtomicU64, Ordering};

use prometheus_client::metrics::counter::Counter;
use prometheus_client::registry::Registry;

static SLOW: LazyLock<Counter> = LazyLock::new(Counter::default);

/// Add the task-poll counter to `registry`.
pub(crate) fn register(registry: &mut Registry) {
    registry.register(
        "forkline_task_slow_polls",
        "Task polls longer than the slow-poll threshold (counted only in builds with --cfg tokio_unstable)",
        SLOW.clone(),
    );
}

/// Time every task poll on runtimes `builder` builds.
#[cfg(tokio_unstable)]
pub(crate) fn install(builder: &mut tokio::runtime::Builder, threshold: std::time::Duration) {
    use std::cell::Cell;
    use std::time::Instant;

    thread_local! {
        static STARTED: Cell<Option<Instant>> = const { Cell::new(None) };
    }
    static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);
    static LAST_WARNED_MS: AtomicU64 = AtomicU64::new(0);

    builder.on_before_task_poll(|_| STARTED.with(|s| s.set(Some(Instant::now()))));
    builder.on_after_task_poll(move |meta| {
        let Some(started) = STARTED.with(Cell::take) else { return };
        let took = started.elapsed();
        if took <= threshold {
            return;
        }
        SLOW.inc();
        let now = u64::try_from(EPOCH.elapsed().as_millis()).unwrap_or(u64::MAX).max(1);
        let last = LAST_WARNED_MS.load(Ordering::Relaxed);
        if (last == 0 || now.saturating_sub(last) >= 10_000)
            && LAST_WARNED_MS.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_ok()
        {
            tracing::warn!(
                spawned_at = %meta.spawned_at(),
                took_ms = took.as_secs_f64() * 1000.0,
                threshold_ms = threshold.as_secs_f64() * 1000.0,
                "slow task poll: a task held a worker thread (at most one warning every 10 s; \
                 forkline_task_slow_polls_total counts them all)"
            );
        }
    });
}
