//! Building a layer stack around a transport from config.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tower::retry::Policy;
use tower::retry::budget::{Budget, TpsBudget};
use tower::util::BoxCloneSyncService;
use tower::{BoxError, Service, ServiceExt};

use crate::breaker::{Breaker, BreakerConfig};
use crate::error::ServiceError;
use crate::limit::Limit;

/// Per-service layer settings, shared by every service kind.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StackConfig {
    /// Timeout per attempt, in milliseconds.
    pub timeout_ms: Option<u64>,
    /// Retries after a retryable failure (0 = none).
    #[serde(default)]
    pub retries: u32,
    /// Retries may add at most this percentage of extra load (default 20).
    pub retry_budget_percent: Option<f32>,
    /// Pause before each retry, in milliseconds.
    #[serde(default)]
    pub retry_backoff_ms: u64,
    /// Maximum calls in flight.
    pub concurrency_limit: Option<usize>,
    /// At the limit, fail at once instead of waiting.
    #[serde(default)]
    pub load_shed: bool,
    /// Circuit breaker.
    pub breaker: Option<BreakerConfig>,
}

impl StackConfig {
    /// Keys this struct reads from a service's table.
    pub const KEYS: &'static [&'static str] = &[
        "timeout_ms",
        "retries",
        "retry_budget_percent",
        "retry_backoff_ms",
        "concurrency_limit",
        "load_shed",
        "breaker",
    ];
}

type Retryable<Resp> = fn(&Result<Resp, ServiceError>) -> bool;

struct RetryPolicy<Resp> {
    remaining: u32,
    budget: Arc<TpsBudget>,
    backoff: Duration,
    retryable: Retryable<Resp>,
}

impl<Resp> Clone for RetryPolicy<Resp> {
    fn clone(&self) -> Self {
        Self {
            remaining: self.remaining,
            budget: self.budget.clone(),
            backoff: self.backoff,
            retryable: self.retryable,
        }
    }
}

impl<Req: Clone, Resp> Policy<Req, Resp, BoxError> for RetryPolicy<Resp> {
    type Future = tokio::time::Sleep;

    fn retry(&mut self, _req: &mut Req, result: &mut Result<Resp, BoxError>) -> Option<Self::Future> {
        let retry = match result {
            Ok(_) => false,
            Err(e) => match e.downcast_ref::<ServiceError>() {
                Some(se) => (self.retryable)(&Err(se.clone())),
                None => e.is::<tower::timeout::error::Elapsed>(),
            },
        };
        if !retry {
            self.budget.deposit();
            return None;
        }
        if self.remaining == 0 || !self.budget.withdraw() {
            return None;
        }
        self.remaining -= 1;
        Some(tokio::time::sleep(self.backoff))
    }

    fn clone_request(&mut self, req: &Req) -> Option<Req> {
        Some(req.clone())
    }
}

/// Wrap `transport` in the layers `config` asks for.
///
/// `retryable` decides which failures are retried (see
/// [`ServiceError::is_retryable`] for the usual choice).
pub fn layered<S, Req, Resp>(
    transport: S,
    config: &StackConfig,
    retryable: Retryable<Resp>,
) -> BoxCloneSyncService<Req, Resp, ServiceError>
where
    S: Service<Req, Response = Resp, Error = ServiceError> + Clone + Send + Sync + 'static,
    S::Future: Send + 'static,
    Req: Clone + Send + Sync + 'static,
    Resp: Send + 'static,
{
    type Boxed<Req, Resp> = BoxCloneSyncService<Req, Resp, BoxError>;
    let mut svc: Boxed<Req, Resp> = BoxCloneSyncService::new(transport.map_err(BoxError::from));
    if let Some(ms) = config.timeout_ms {
        svc = BoxCloneSyncService::new(tower::timeout::Timeout::new(svc, Duration::from_millis(ms)));
    }
    if config.retries > 0 {
        let percent = config.retry_budget_percent.unwrap_or(20.0).clamp(0.0, 1000.0) / 100.0;
        let policy = RetryPolicy {
            remaining: config.retries,
            budget: Arc::new(TpsBudget::new(Duration::from_secs(10), 10, percent)),
            backoff: Duration::from_millis(config.retry_backoff_ms),
            retryable,
        };
        svc = BoxCloneSyncService::new(tower::retry::Retry::new(policy, svc));
    }
    if let Some(breaker) = config.breaker {
        svc = BoxCloneSyncService::new(Breaker::new(svc, breaker));
    }
    if let Some(max) = config.concurrency_limit {
        svc = BoxCloneSyncService::new(Limit::new(svc, max.max(1), config.load_shed));
    }
    BoxCloneSyncService::new(svc.map_err(ServiceError::from_box))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;
    use crate::error::ServiceErrorKind;

    /// A transport that fails its first `fail_first` calls, sleeping `delay_ms` each.
    fn flaky(
        fail_first: u32,
        delay_ms: u64,
        kind: ServiceErrorKind,
    ) -> (
        impl Service<u32, Response = u32, Error = ServiceError, Future = impl Send> + Clone + Send + Sync + 'static,
        Arc<AtomicU32>,
    ) {
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let svc = tower::service_fn(move |req: u32| {
            let n = c.fetch_add(1, Ordering::SeqCst);
            async move {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                if n < fail_first { Err(ServiceError::new(kind, "flaky")) } else { Ok(req * 2) }
            }
        });
        (svc, calls)
    }

    fn retry_all(r: &Result<u32, ServiceError>) -> bool {
        r.as_ref().err().is_some_and(ServiceError::is_retryable)
    }

    #[tokio::test]
    async fn retries_recover_a_transient_failure() {
        let (svc, calls) = flaky(1, 0, ServiceErrorKind::Transport);
        let stack = layered(svc, &StackConfig { retries: 2, ..Default::default() }, retry_all);
        assert_eq!(stack.oneshot(21).await, Ok(42));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn non_retryable_errors_are_not_retried() {
        let (svc, calls) = flaky(1, 0, ServiceErrorKind::Status);
        let stack = layered(svc, &StackConfig { retries: 2, ..Default::default() }, retry_all);
        assert_eq!(stack.oneshot(1).await.unwrap_err().kind, ServiceErrorKind::Status);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn timeouts_apply_per_attempt_and_are_retried() {
        let (svc, calls) = flaky(0, 50, ServiceErrorKind::Transport);
        let config = StackConfig { timeout_ms: Some(10), retries: 1, ..Default::default() };
        let stack = layered(svc, &config, retry_all);
        let err = stack.oneshot(1).await.unwrap_err();
        assert_eq!(err.kind, ServiceErrorKind::Timeout);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "one retry after the first timeout");
    }

    #[tokio::test(start_paused = true)]
    async fn the_limit_sheds_when_full() {
        let (svc, _) = flaky(0, 100, ServiceErrorKind::Transport);
        let config = StackConfig { concurrency_limit: Some(1), load_shed: true, ..Default::default() };
        let stack = layered(svc, &config, retry_all);
        let first = tokio::spawn(stack.clone().oneshot(1));
        tokio::task::yield_now().await;
        let second = stack.clone().oneshot(2).await;
        assert_eq!(second.unwrap_err().kind, ServiceErrorKind::Overloaded);
        assert_eq!(first.await.unwrap(), Ok(2));
        assert_eq!(stack.oneshot(3).await, Ok(6), "capacity is back after the first call");
    }

    #[tokio::test(start_paused = true)]
    async fn the_breaker_opens_and_recovers() {
        let (svc, calls) = flaky(3, 0, ServiceErrorKind::Transport);
        let config = StackConfig { breaker: Some(BreakerConfig { failures: 3, open_ms: 100 }), ..Default::default() };
        let stack = layered(svc, &config, retry_all);
        for _ in 0..3 {
            assert_eq!(stack.clone().oneshot(1).await.unwrap_err().kind, ServiceErrorKind::Transport);
        }
        assert_eq!(stack.clone().oneshot(1).await.unwrap_err().kind, ServiceErrorKind::CircuitOpen);
        assert_eq!(calls.load(Ordering::SeqCst), 3, "an open breaker does not call the backend");
        tokio::time::advance(Duration::from_millis(100)).await;
        assert_eq!(stack.clone().oneshot(5).await, Ok(10), "the half-open trial succeeds and closes it");
        assert_eq!(stack.oneshot(6).await, Ok(12));
    }

    #[tokio::test(start_paused = true)]
    async fn a_failed_half_open_trial_reopens_and_other_calls_are_refused_meanwhile() {
        let (svc, calls) = flaky(2, 50, ServiceErrorKind::Transport);
        let config = StackConfig { breaker: Some(BreakerConfig { failures: 1, open_ms: 100 }), ..Default::default() };
        let stack = layered(svc, &config, retry_all);
        assert_eq!(stack.clone().oneshot(1).await.unwrap_err().kind, ServiceErrorKind::Transport);
        tokio::time::advance(Duration::from_millis(100)).await;

        // The trial call is in flight: the breaker lets nothing else through.
        let trial = tokio::spawn(stack.clone().oneshot(1));
        tokio::task::yield_now().await;
        assert_eq!(stack.clone().oneshot(1).await.unwrap_err().kind, ServiceErrorKind::CircuitOpen);
        assert_eq!(trial.await.unwrap().unwrap_err().kind, ServiceErrorKind::Transport, "the trial fails");

        // A failed trial opens the breaker again for the full period.
        assert_eq!(stack.clone().oneshot(1).await.unwrap_err().kind, ServiceErrorKind::CircuitOpen);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        tokio::time::advance(Duration::from_millis(100)).await;
        assert_eq!(stack.oneshot(4).await, Ok(8), "the next trial succeeds");
    }

    #[tokio::test(start_paused = true)]
    async fn without_shedding_the_limit_queues_callers() {
        let (svc, calls) = flaky(0, 100, ServiceErrorKind::Transport);
        let config = StackConfig { concurrency_limit: Some(1), ..Default::default() };
        let stack = layered(svc, &config, retry_all);
        let started = tokio::time::Instant::now();
        let (a, b) = tokio::join!(stack.clone().oneshot(1), stack.clone().oneshot(2));
        assert_eq!((a, b), (Ok(2), Ok(4)), "the second caller waited its turn instead of failing");
        assert_eq!(started.elapsed(), Duration::from_millis(200), "one at a time");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
