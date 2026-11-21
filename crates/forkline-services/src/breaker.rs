//! A small consecutive-failure circuit breaker.
//!
//! Closed: calls pass; `failures` consecutive errors open it. Open: calls fail
//! at once with `CircuitOpen` for `open_ms`. Then one trial call is let
//! through (half-open): success closes the breaker, failure opens it again.

use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use forkline_core::node::BoxFuture;
use serde::Deserialize;
use tokio::time::Instant;
use tower::{BoxError, Service, ServiceExt};

use crate::error::{ServiceError, ServiceErrorKind};

/// Circuit breaker settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreakerConfig {
    /// Consecutive failures that open the breaker.
    pub failures: u32,
    /// How long it stays open before a trial call, in milliseconds.
    pub open_ms: u64,
}

#[derive(Debug)]
enum State {
    Closed { failures: u32 },
    Open { until: Instant },
    HalfOpen,
}

#[derive(Clone, Debug)]
pub(crate) struct Breaker<S> {
    inner: S,
    state: Arc<Mutex<State>>,
    config: BreakerConfig,
}

impl<S> Breaker<S> {
    pub(crate) fn new(inner: S, config: BreakerConfig) -> Self {
        Self { inner, state: Arc::new(Mutex::new(State::Closed { failures: 0 })), config }
    }
}

/// Decide whether a call may proceed; moves Open to HalfOpen when the wait is over.
fn admit(state: &Mutex<State>) -> bool {
    let Ok(mut s) = state.lock() else { return true };
    match *s {
        State::Closed { .. } => true,
        State::HalfOpen => false,
        State::Open { until } => {
            if Instant::now() >= until {
                *s = State::HalfOpen;
                true
            } else {
                false
            }
        }
    }
}

fn record(state: &Mutex<State>, ok: bool, config: BreakerConfig) {
    let Ok(mut s) = state.lock() else { return };
    let open = State::Open { until: Instant::now() + Duration::from_millis(config.open_ms) };
    *s = match (&*s, ok) {
        (_, true) => State::Closed { failures: 0 },
        (State::HalfOpen | State::Open { .. }, false) => open,
        (State::Closed { failures }, false) => {
            let failures = failures + 1;
            if failures >= config.failures { open } else { State::Closed { failures } }
        }
    };
}

impl<S, Req> Service<Req> for Breaker<S>
where
    S: Service<Req, Error = BoxError> + Clone + Send + 'static,
    S::Future: Send + 'static,
    Req: Send + 'static,
{
    type Response = S::Response;
    type Error = BoxError;
    type Future = BoxFuture<'static, Result<S::Response, BoxError>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), BoxError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Req) -> Self::Future {
        let inner = self.inner.clone();
        let state = self.state.clone();
        let config = self.config;
        Box::pin(async move {
            if !admit(&state) {
                return Err(ServiceError::new(ServiceErrorKind::CircuitOpen, "circuit breaker is open").into());
            }
            let result = inner.oneshot(req).await;
            record(&state, result.is_ok(), config);
            result
        })
    }
}
