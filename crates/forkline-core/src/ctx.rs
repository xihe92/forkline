//! The per-request context handed to every node.

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::HeaderMap;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::error::NodeError;
use crate::node::BoxFuture;
use crate::reply::Reply;
use crate::service::ServiceRegistry;

/// Runs another plan on behalf of a module ([`Ctx::execute_plan`]).
/// Implemented by the engine.
pub trait PlanRunner: Send + Sync {
    /// Run `plan` with `body` under `cx` (already scoped to the subplan).
    fn run_plan<'a>(&'a self, cx: Ctx, plan: &'a str, body: Bytes) -> BoxFuture<'a, Result<Reply, NodeError>>;

    /// The time budget of `plan`, or `None` if there is no such plan.
    fn time_budget(&self, plan: &str) -> Option<Duration>;
}

/// The deepest chain of nested [`Ctx::execute_plan`] calls allowed.
pub const MAX_PLAN_DEPTH: u8 = 8;

/// Identifier of one request, unique within a process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(pub u64);

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

/// The inbound protocol a request arrived on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Protocol {
    /// HTTP/1.1 or HTTP/2 through the HTTP adapter.
    Http,
    /// gRPC through the gRPC adapter.
    Grpc,
    /// Length-delimited TCP frames.
    Tcp,
    /// Called in-process (tests, subplans, benchmarks).
    InProcess,
}

/// What the adapter knows about a request before any module runs.
#[derive(Clone, Debug)]
pub struct RequestEnv {
    /// Protocol the request arrived on.
    pub protocol: Protocol,
    /// Remote address, when the transport knows it.
    pub peer: Option<SocketAddr>,
    /// When the adapter received the request.
    pub received_at: Instant,
    /// Deadline sent by the caller (for example `grpc-timeout`), if any.
    pub inbound_deadline: Option<Instant>,
    /// Route, method or path the request was addressed to.
    pub route: String,
    /// Inbound headers or gRPC metadata.
    pub metadata: HeaderMap,
}

impl RequestEnv {
    /// An environment for an in-process call received now.
    pub fn in_process() -> Self {
        Self {
            protocol: Protocol::InProcess,
            peer: None,
            received_at: Instant::now(),
            inbound_deadline: None,
            route: String::new(),
            metadata: HeaderMap::new(),
        }
    }
}

/// The context every node receives.
#[derive(Clone)]
pub struct Ctx {
    inner: Arc<CtxInner>,
}

/// Everything a [`Ctx`] is made of. Used by the engine and test harnesses.
pub struct CtxParts {
    /// Request id.
    pub request_id: RequestId,
    /// Plan name.
    pub plan: Arc<str>,
    /// The span for this (sub)plan run.
    pub span: tracing::Span,
    /// Deadline.
    pub deadline: Instant,
    /// Cancellation token.
    pub cancel: CancellationToken,
    /// Request environment.
    pub env: RequestEnv,
    /// Named services.
    pub services: Arc<ServiceRegistry>,
    /// Runs subplans; `None` disables [`Ctx::execute_plan`].
    pub runner: Option<Arc<dyn PlanRunner>>,
    /// Nesting depth of subplan calls (0 for the request's own plan).
    pub depth: u8,
}

impl fmt::Debug for CtxParts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CtxParts").field("request_id", &self.request_id).field("plan", &self.plan).finish()
    }
}

struct CtxInner {
    request_id: RequestId,
    plan: Arc<str>,
    span: tracing::Span,
    deadline: Instant,
    cancel: CancellationToken,
    env: RequestEnv,
    services: Arc<ServiceRegistry>,
    runner: Option<Arc<dyn PlanRunner>>,
    depth: u8,
}

impl Ctx {
    /// Build a context with no services and no subplan runner.
    pub fn new(
        request_id: RequestId,
        plan: Arc<str>,
        span: tracing::Span,
        deadline: Instant,
        cancel: CancellationToken,
        env: RequestEnv,
    ) -> Self {
        Self::from_parts(CtxParts {
            request_id,
            plan,
            span,
            deadline,
            cancel,
            env,
            services: Arc::default(),
            runner: None,
            depth: 0,
        })
    }

    /// Build a context from all of its parts.
    pub fn from_parts(p: CtxParts) -> Self {
        Self {
            inner: Arc::new(CtxInner {
                request_id: p.request_id,
                plan: p.plan,
                span: p.span,
                deadline: p.deadline,
                cancel: p.cancel,
                env: p.env,
                services: p.services,
                runner: p.runner,
                depth: p.depth,
            }),
        }
    }

    /// Look up a service by name and type at request time.
    ///
    /// Prefer resolving services once in [`Build::build_with`](crate::Build::build_with),
    /// which also makes a missing service fail at plan load.
    pub fn service<T: Clone + Send + Sync + 'static>(&self, name: &str) -> Result<T, NodeError> {
        self.inner.services.get::<T>(name).map_err(|e| NodeError::internal(e.to_string()))
    }

    /// The services available to this request.
    pub fn services(&self) -> &Arc<ServiceRegistry> {
        &self.inner.services
    }

    /// Nesting depth of subplan calls.
    pub fn depth(&self) -> u8 {
        self.inner.depth
    }

    /// Run another plan of the same snapshot inline and return its reply.
    ///
    /// The subplan shares this request's id, environment, services and
    /// cancellation; its deadline is the earlier of its own budget and this
    /// request's deadline. A failure reply from the subplan (one its terminal
    /// produced) is returned as `Ok` with an error [`Status`](crate::Status), so the caller
    /// can inspect it; use [`Reply::into_result`] to treat it as an error.
    pub async fn execute_plan(&self, plan: &str, body: impl Into<Bytes>) -> Result<Reply, NodeError> {
        let runner = self.inner.runner.clone().ok_or_else(|| NodeError::internal("subplans are not available here"))?;
        if self.inner.depth >= MAX_PLAN_DEPTH {
            return Err(NodeError::internal(format!("subplan `{plan}` exceeds the nesting limit of {MAX_PLAN_DEPTH}")));
        }
        let budget = runner.time_budget(plan).ok_or_else(|| NodeError::internal(format!("unknown plan `{plan}`")))?;
        let deadline = (Instant::now() + budget).min(self.inner.deadline);
        let span = tracing::info_span!(parent: &self.inner.span, "subplan", plan = %plan);
        let child = Ctx::from_parts(CtxParts {
            request_id: self.inner.request_id,
            plan: Arc::from(plan),
            span,
            deadline,
            cancel: self.inner.cancel.child_token(),
            env: self.inner.env.clone(),
            services: self.inner.services.clone(),
            runner: Some(runner.clone()),
            depth: self.inner.depth + 1,
        });
        runner.run_plan(child, plan, body.into()).await
    }

    /// The request's id.
    pub fn request_id(&self) -> RequestId {
        self.inner.request_id
    }

    /// Name of the plan being run.
    pub fn plan(&self) -> &str {
        &self.inner.plan
    }

    /// The request's tracing span.
    pub fn span(&self) -> &tracing::Span {
        &self.inner.span
    }

    /// The request's deadline.
    pub fn deadline(&self) -> Instant {
        self.inner.deadline
    }

    /// Time left before the deadline (zero once it has passed).
    pub fn remaining(&self) -> Duration {
        self.inner.deadline.saturating_duration_since(Instant::now())
    }

    /// The request-wide cancellation token. Modules may await `cancelled()`.
    pub fn cancellation(&self) -> &CancellationToken {
        &self.inner.cancel
    }

    /// True once the request has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancel.is_cancelled()
    }

    /// The request environment.
    pub fn env(&self) -> &RequestEnv {
        &self.inner.env
    }

    /// Run CPU-heavy work on tokio's blocking pool so it does not stall the runtime.
    pub async fn blocking<F, R>(&self, work: F) -> Result<R, NodeError>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        tokio::task::spawn_blocking(work).await.map_err(|e| NodeError::internal(format!("blocking task failed: {e}")))
    }
}

impl fmt::Debug for Ctx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ctx")
            .field("request_id", &self.inner.request_id)
            .field("plan", &self.inner.plan)
            .field("remaining", &self.remaining())
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}
