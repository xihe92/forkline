//! Concurrency limit with optional shedding.
//!
//! tower's `ConcurrencyLimit` holds a permit future that is not `Sync`, and
//! service clients shared through the registry must be. This layer takes a
//! permit inside the call future instead: with `shed`, a call at the limit
//! fails at once with `Overloaded`; without it, the call waits for a permit
//! (bounded by the request deadline the client applies).

use std::sync::Arc;
use std::task::{Context, Poll};

use forkline_core::node::BoxFuture;
use tokio::sync::Semaphore;
use tower::{BoxError, Service, ServiceExt};

use crate::error::{ServiceError, ServiceErrorKind};

#[derive(Clone, Debug)]
pub(crate) struct Limit<S> {
    inner: S,
    permits: Arc<Semaphore>,
    max: usize,
    shed: bool,
}

impl<S> Limit<S> {
    pub(crate) fn new(inner: S, max: usize, shed: bool) -> Self {
        Self { inner, permits: Arc::new(Semaphore::new(max)), max, shed }
    }
}

impl<S, Req> Service<Req> for Limit<S>
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
        let permits = self.permits.clone();
        let (max, shed) = (self.max, self.shed);
        Box::pin(async move {
            let _permit = if shed {
                permits.try_acquire_owned().map_err(|_| {
                    ServiceError::new(ServiceErrorKind::Overloaded, format!("at the concurrency limit of {max}"))
                })?
            } else {
                permits
                    .acquire_owned()
                    .await
                    .map_err(|_| ServiceError::new(ServiceErrorKind::Other, "limiter closed"))?
            };
            inner.oneshot(req).await
        })
    }
}
