//! Spotting modules that block a Tokio worker thread.
//!
//! The engine times every poll of a module's future and every call of a
//! synchronous node (decode, branch, multibranch, join, encode, terminal).
//! One that takes longer than the threshold held its worker thread, and every
//! request queued behind it, for that long: it did blocking or CPU-heavy work
//! that belongs in `cx.blocking` or a pool. Each one adds to
//! `forkline_node_slow_polls_total{plan,node}` and logs a warning naming the
//! plan and node (at most one per node every 10 s).
//!
//! The threshold is per engine, 10 ms by default, 0 to switch it off. A test
//! suite can also set `FORKLINE_PANIC_ON_SLOW_POLL_MS`: any poll longer than
//! that panics, so a blocking module fails the test that ran it.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use forkline_plan::NodeId;

use crate::analysis::ExecPlan;

/// Default slow-poll threshold.
pub const DEFAULT_SLOW_POLL: Duration = Duration::from_millis(10);

/// Environment variable: polls longer than this many milliseconds panic.
pub const PANIC_ENV: &str = "FORKLINE_PANIC_ON_SLOW_POLL_MS";

/// How often a node may log a slow-poll warning, in milliseconds.
const WARN_EVERY_MS: u64 = 10_000;

static PANIC_ABOVE: LazyLock<Option<Duration>> = LazyLock::new(|| {
    std::env::var(PANIC_ENV).ok().and_then(|v| v.parse::<u64>().ok()).filter(|&ms| ms > 0).map(Duration::from_millis)
});

/// Reference point for the per-node "last warned" stamps.
static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

/// One engine's slow-poll setting, shared with every plan it loads.
#[derive(Debug)]
pub(crate) struct SlowPoll {
    /// Threshold in nanoseconds; 0 is off.
    nanos: AtomicU64,
}

impl SlowPoll {
    pub(crate) fn new(threshold: Option<Duration>) -> Self {
        let this = Self { nanos: AtomicU64::new(0) };
        this.set(threshold);
        this
    }

    pub(crate) fn set(&self, threshold: Option<Duration>) {
        let nanos = threshold.map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
        self.nanos.store(nanos, Ordering::Relaxed);
    }

    pub(crate) fn threshold(&self) -> Option<Duration> {
        Some(self.nanos.load(Ordering::Relaxed)).filter(|&n| n > 0).map(Duration::from_nanos)
    }

    /// Record a poll of `node` that took `took`.
    #[inline]
    #[allow(
        clippy::panic,
        reason = "only when a test suite sets FORKLINE_PANIC_ON_SLOW_POLL_MS: failing the test is the point"
    )]
    pub(crate) fn check(&self, took: Duration, plan: &ExecPlan, node: NodeId) {
        let threshold = self.nanos.load(Ordering::Relaxed);
        if threshold > 0 && took.as_nanos() > u128::from(threshold) {
            Self::slow(took, Duration::from_nanos(threshold), plan, node);
        }
        if let Some(limit) = *PANIC_ABOVE
            && took > limit
        {
            let name = plan.compiled.node(node).map_or("?", |n| &*n.name);
            panic!(
                "slow poll: node `{name}` of plan `{}` held a worker thread for {took:?} \
                 ({PANIC_ENV} = {limit:?}); move blocking or CPU-heavy work to cx.blocking or a pool",
                plan.compiled.name
            );
        }
    }

    #[cold]
    fn slow(took: Duration, threshold: Duration, plan: &ExecPlan, node: NodeId) {
        plan.metrics.slow_poll(node);
        let now = u64::try_from(EPOCH.elapsed().as_millis()).unwrap_or(u64::MAX).max(1);
        let Some(last) = plan.metrics.last_warned(node) else { return };
        let previous = last.load(Ordering::Relaxed);
        let due = previous == 0 || now.saturating_sub(previous) >= WARN_EVERY_MS;
        if due && last.compare_exchange(previous, now, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
            let name = plan.compiled.node(node).map_or("?", |n| &*n.name);
            tracing::warn!(
                plan = %plan.compiled.name,
                node = %name,
                took_ms = took.as_secs_f64() * 1000.0,
                threshold_ms = threshold.as_secs_f64() * 1000.0,
                "slow poll: the node held a worker thread; move blocking or CPU-heavy work to cx.blocking or a pool \
                 (at most one warning per node every 10 s; forkline_node_slow_polls_total counts them all)"
            );
        }
    }
}
