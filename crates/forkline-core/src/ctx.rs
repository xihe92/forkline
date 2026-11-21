//! The per-request context handed to every node.

use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use http::HeaderMap;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::error::NodeError;

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

struct CtxInner {
    request_id: RequestId,
    plan: Arc<str>,
    span: tracing::Span,
    deadline: Instant,
    cancel: CancellationToken,
    env: RequestEnv,
}

impl Ctx {
    /// Build a context. Called by the engine and by test harnesses.
    pub fn new(
        request_id: RequestId,
        plan: Arc<str>,
        span: tracing::Span,
        deadline: Instant,
        cancel: CancellationToken,
        env: RequestEnv,
    ) -> Self {
        Self { inner: Arc::new(CtxInner { request_id, plan, span, deadline, cancel, env }) }
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
