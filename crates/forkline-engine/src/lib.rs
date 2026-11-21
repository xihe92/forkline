//! The Forkline plan executor.
//!
//! [`Engine`] holds the current [`Snapshot`] of compiled plans in an
//! `arc-swap` cell. Each request pins the snapshot it started with and keeps
//! it until it finishes, so a reload never mixes two configurations inside
//! one request.
//!
//! # Execution model
//!
//! A request runs as a set of *segments*: straight runs of nodes executed
//! inline on the request's task. A segment ends where the graph diverges (a
//! fork, branch or multibranch), where it reaches a join, at the encode node,
//! or at a leaf. Paths of a fork run concurrently on the request's task, or
//! on separate tasks when the fork has `spawn = true`.
//!
//! A join receives one arrival per path that reaches it, calls its module
//! once per arrival, serially, and fires once no running path can still
//! reach it. It then applies the merge rules (see [`merge`]) relative to the
//! map at the join's owning divergence point (its immediate dominator).
//!
//! The reply is sent as soon as the encode node has run. Paths that cannot
//! reach the encode node are *detached*: they keep running under the
//! request's deadline, their failures are logged rather than answered, and
//! [`Engine::wait_detached`] waits for them at shutdown.

#![forbid(unsafe_code)]

mod analysis;
mod exec;
pub mod merge;
mod service;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_swap::ArcSwap;
use bytes::Bytes;
use forkline_core::node::BoxFuture;
use forkline_core::{
    Ctx, CtxParts, NodeError, PlanError, PlanRunner, Registry, Reply, RequestEnv, RequestId, ServiceRegistry,
};
use forkline_plan::{CompiledPlan, Diagnostics, PlansFile, compile_all_with};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::Instrument;

pub use analysis::ExecPlan;
pub use service::PlanService;

/// One request as an adapter hands it to the engine.
#[derive(Clone, Debug)]
pub struct Request {
    /// Raw request body.
    pub body: Bytes,
    /// What the adapter knows about the request.
    pub env: RequestEnv,
}

impl Request {
    /// An in-process request received now.
    pub fn in_process(body: impl Into<Bytes>) -> Self {
        Self { body: body.into(), env: RequestEnv::in_process() }
    }
}

/// A request the engine could not answer with a reply.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// No plan of that name in the current snapshot.
    #[error("unknown plan `{0}`")]
    UnknownPlan(String),
    /// The plan failed and its terminal node failed too.
    #[error("terminal node `{terminal}` failed ({message}) while handling: {error}")]
    TerminalFailed {
        /// The original failure.
        error: PlanError,
        /// Terminal node name.
        terminal: Arc<str>,
        /// The terminal's own error.
        message: String,
    },
}

/// An immutable set of compiled plans and the services they were built with.
#[derive(Debug)]
pub struct Snapshot {
    /// Increases by one on every successful load.
    pub version: u64,
    /// Plans by name.
    pub plans: Arc<BTreeMap<String, Arc<ExecPlan>>>,
    /// Services available to modules.
    pub services: Arc<ServiceRegistry>,
    runner: Arc<SnapshotRunner>,
}

impl Snapshot {
    fn build(
        version: u64,
        file: &PlansFile,
        registry: &Registry,
        services: Arc<ServiceRegistry>,
        detached: &TaskTracker,
    ) -> Result<Self, Diagnostics> {
        let compiled = compile_all_with(file, registry, &services)?;
        let plans: BTreeMap<String, Arc<ExecPlan>> = compiled
            .into_iter()
            .map(|(name, plan)| {
                let plan: CompiledPlan = Arc::try_unwrap(plan).unwrap_or_else(|shared| (*shared).clone());
                (name, Arc::new(ExecPlan::new(plan)))
            })
            .collect();
        let plans = Arc::new(plans);
        let runner = Arc::new(SnapshotRunner { plans: plans.clone(), detached: detached.clone() });
        Ok(Self { version, plans, services, runner })
    }
}

/// Runs subplans against the snapshot the request pinned.
#[derive(Debug)]
struct SnapshotRunner {
    plans: Arc<BTreeMap<String, Arc<ExecPlan>>>,
    detached: TaskTracker,
}

impl PlanRunner for SnapshotRunner {
    fn run_plan<'a>(&'a self, cx: Ctx, plan: &'a str, body: Bytes) -> BoxFuture<'a, Result<Reply, NodeError>> {
        Box::pin(async move {
            let exec =
                self.plans.get(plan).cloned().ok_or_else(|| NodeError::internal(format!("unknown plan `{plan}`")))?;
            let span = cx.span().clone();
            exec::run(exec, cx, body, &self.detached)
                .instrument(span)
                .await
                .map_err(|e| NodeError::internal(e.to_string()))
        })
    }

    fn time_budget(&self, plan: &str) -> Option<std::time::Duration> {
        self.plans.get(plan).map(|p| p.compiled.time_budget)
    }
}

/// Runs plans.
#[derive(Debug)]
pub struct Engine {
    registry: Arc<Registry>,
    current: ArcSwap<Snapshot>,
    next_request: AtomicU64,
    detached: TaskTracker,
}

impl Engine {
    /// Compile `plans` against `registry`, with no services. Fails with every problem found.
    pub fn new(registry: Arc<Registry>, plans: &PlansFile) -> Result<Self, Diagnostics> {
        Self::with_services(registry, plans, Arc::new(ServiceRegistry::new()))
    }

    /// Compile `plans` against `registry`; modules may resolve `services` while they are built.
    pub fn with_services(
        registry: Arc<Registry>,
        plans: &PlansFile,
        services: Arc<ServiceRegistry>,
    ) -> Result<Self, Diagnostics> {
        let detached = TaskTracker::new();
        let snapshot = Snapshot::build(1, plans, &registry, services, &detached)?;
        Ok(Self { registry, current: ArcSwap::from_pointee(snapshot), next_request: AtomicU64::new(1), detached })
    }

    /// The current snapshot.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.current.load_full()
    }

    /// True when the current snapshot has a plan of this name.
    pub fn has_plan(&self, plan: &str) -> bool {
        self.current.load().plans.contains_key(plan)
    }

    /// Compile a new plans file and swap it in. On failure the running
    /// snapshot stays in place. Requests already running keep the snapshot
    /// they started with.
    pub fn reload(&self, plans: &PlansFile) -> Result<u64, Diagnostics> {
        let services = self.current.load().services.clone();
        self.reload_with_services(plans, services)
    }

    /// Like [`reload`](Self::reload), replacing the services as well.
    pub fn reload_with_services(&self, plans: &PlansFile, services: Arc<ServiceRegistry>) -> Result<u64, Diagnostics> {
        let version = self.current.load().version + 1;
        let snapshot = Snapshot::build(version, plans, &self.registry, services, &self.detached)?;
        self.swap(snapshot);
        Ok(version)
    }

    fn swap(&self, snapshot: Snapshot) {
        let version = snapshot.version;
        self.current.store(Arc::new(snapshot));
        tracing::info!(version, "snapshot swapped in");
    }

    /// A `tower::Service` that runs one plan.
    pub fn plan_service(self: &Arc<Self>, plan: impl Into<Arc<str>>) -> PlanService {
        PlanService::new(self.clone(), plan.into())
    }

    /// Number of requests whose detached paths are still running.
    pub fn detached_running(&self) -> usize {
        self.detached.len()
    }

    /// Wait until every detached path has finished. Paths stop on their own
    /// at their request's deadline at the latest.
    pub async fn wait_detached(&self) {
        self.detached.close();
        self.detached.wait().await;
        self.detached.reopen();
    }

    /// Run `plan` for one request.
    pub async fn call(&self, plan: &str, request: Request) -> Result<Reply, EngineError> {
        let snapshot = self.current.load_full();
        let exec = snapshot.plans.get(plan).cloned().ok_or_else(|| EngineError::UnknownPlan(plan.to_owned()))?;
        let id = RequestId(self.next_request.fetch_add(1, Ordering::Relaxed));
        let Request { body, env } = request;
        let budget_deadline = env.received_at + exec.compiled.time_budget;
        let deadline = env.inbound_deadline.map_or(budget_deadline, |d| d.min(budget_deadline));
        let span = tracing::info_span!(
            "request",
            request_id = %id,
            plan = %exec.compiled.name,
            snapshot_version = snapshot.version,
        );
        let cx = Ctx::from_parts(CtxParts {
            request_id: id,
            plan: exec.compiled.name.clone(),
            span: span.clone(),
            deadline,
            cancel: CancellationToken::new(),
            env,
            services: snapshot.services.clone(),
            runner: Some(snapshot.runner.clone()),
            depth: 0,
        });
        exec::run(exec, cx, body, &self.detached).instrument(span).await
    }
}
