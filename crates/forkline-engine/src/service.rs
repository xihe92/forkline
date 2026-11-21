//! Each plan as a `tower::Service`, so adapters and tower layers compose with it.

use std::fmt;
use std::sync::Arc;
use std::task::{Context, Poll};

use forkline_core::Reply;
use forkline_core::node::BoxFuture;

use crate::{Engine, EngineError, Request};

/// Runs one named plan. Cheap to clone.
#[derive(Clone)]
pub struct PlanService {
    engine: Arc<Engine>,
    plan: Arc<str>,
}

impl PlanService {
    pub(crate) fn new(engine: Arc<Engine>, plan: Arc<str>) -> Self {
        Self { engine, plan }
    }

    /// The plan this service runs.
    pub fn plan(&self) -> &str {
        &self.plan
    }
}

impl fmt::Debug for PlanService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlanService").field("plan", &self.plan).finish()
    }
}

impl tower::Service<Request> for PlanService {
    type Response = Reply;
    type Error = EngineError;
    type Future = BoxFuture<'static, Result<Reply, EngineError>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request) -> Self::Future {
        let engine = self.engine.clone();
        let plan = self.plan.clone();
        Box::pin(async move { engine.call(&plan, request).await })
    }
}
