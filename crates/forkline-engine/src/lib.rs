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
mod metrics_handles;
mod service;

use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_swap::ArcSwap;
use bytes::Bytes;
use forkline_core::node::BoxFuture;
use forkline_core::{
    Ctx, CtxParts, NodeError, PlanError, PlanRunner, Registry, Reply, RequestEnv, RequestId, ServiceRegistry,
};
use forkline_plan::{CompiledPlan, Diagnostics, PlansFile, compile_all_with};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::Instrument;

pub use analysis::ExecPlan;
use metrics_handles::Outcome;
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
    /// The request was refused before any module ran (late, or queued too long).
    #[error("request for `{plan}` shed: {reason}")]
    Shed {
        /// Plan name.
        plan: String,
        /// Why.
        reason: &'static str,
    },
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
    /// Hash of the config files this snapshot was built from (0 if unknown).
    pub config_hash: u64,
    runner: Arc<SnapshotRunner>,
}

impl Snapshot {
    fn build(
        version: u64,
        input: LoadInput<'_>,
        registry: &Registry,
        detached: &TaskTracker,
    ) -> Result<Self, Diagnostics> {
        let LoadInput { plans: file, services, config_hash } = input;
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
        Ok(Self { version, plans, services, config_hash, runner })
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

/// How many previous snapshots are kept for rollback by default.
pub const DEFAULT_HISTORY: usize = 5;

/// What a load or reload builds a snapshot from.
#[derive(Debug)]
pub struct LoadInput<'a> {
    /// The plans.
    pub plans: &'a PlansFile,
    /// The services modules may resolve.
    pub services: Arc<ServiceRegistry>,
    /// A hash of the config files, shown on spans and the admin API.
    pub config_hash: u64,
}

/// Builds an [`Engine`].
#[derive(Debug)]
pub struct EngineBuilder {
    registry: Arc<Registry>,
    history: usize,
    max_queue: Option<std::time::Duration>,
}

impl EngineBuilder {
    /// Previous snapshots kept for [`Engine::rollback`] (default 5).
    pub fn history(mut self, snapshots: usize) -> Self {
        self.history = snapshots;
        self
    }

    /// Shed requests that waited longer than this before starting.
    pub fn max_queue(mut self, max: Option<std::time::Duration>) -> Self {
        self.max_queue = max;
        self
    }

    /// Compile the first snapshot.
    pub fn build(self, input: LoadInput<'_>) -> Result<Engine, Diagnostics> {
        let detached = TaskTracker::new();
        let snapshot = Snapshot::build(1, input, &self.registry, &detached)?;
        metrics::gauge!("forkline_snapshot_version").set(1.0);
        Ok(Engine {
            registry: self.registry,
            current: ArcSwap::from_pointee(snapshot),
            history: Mutex::new(VecDeque::new()),
            history_len: self.history,
            last_version: AtomicU64::new(1),
            reload_lock: Mutex::new(()),
            next_request: AtomicU64::new(1),
            max_queue_nanos: AtomicU64::new(
                self.max_queue.map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)),
            ),
            detached,
        })
    }
}

/// Runs plans.
#[derive(Debug)]
pub struct Engine {
    registry: Arc<Registry>,
    current: ArcSwap<Snapshot>,
    history: Mutex<VecDeque<Arc<Snapshot>>>,
    history_len: usize,
    last_version: AtomicU64,
    reload_lock: Mutex<()>,
    next_request: AtomicU64,
    max_queue_nanos: AtomicU64,
    detached: TaskTracker,
}

impl Engine {
    /// Start building an engine for `registry`'s modules.
    pub fn builder(registry: Arc<Registry>) -> EngineBuilder {
        EngineBuilder { registry, history: DEFAULT_HISTORY, max_queue: None }
    }

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
        Self::builder(registry).build(LoadInput { plans, services, config_hash: 0 })
    }

    /// The current snapshot.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.current.load_full()
    }

    /// Versions of the snapshots kept for rollback, oldest first.
    pub fn history(&self) -> Vec<u64> {
        self.history.lock().map(|h| h.iter().map(|s| s.version).collect()).unwrap_or_default()
    }

    /// True when the current snapshot has a plan of this name.
    pub fn has_plan(&self, plan: &str) -> bool {
        self.current.load().plans.contains_key(plan)
    }

    /// Shed requests that waited longer than `max` before starting (`None`: never).
    pub fn set_max_queue(&self, max: Option<std::time::Duration>) {
        let nanos = max.map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
        self.max_queue_nanos.store(nanos, Ordering::Relaxed);
    }

    /// Compile a new plans file and swap it in, keeping the services. On
    /// failure the running snapshot stays in place. Requests already running
    /// keep the snapshot they started with.
    pub fn reload(&self, plans: &PlansFile) -> Result<u64, Diagnostics> {
        let current = self.current.load();
        let input = LoadInput { plans, services: current.services.clone(), config_hash: current.config_hash };
        self.load(input)
    }

    /// Like [`reload`](Self::reload), replacing the services as well.
    pub fn reload_with_services(&self, plans: &PlansFile, services: Arc<ServiceRegistry>) -> Result<u64, Diagnostics> {
        self.load(LoadInput { plans, services, config_hash: 0 })
    }

    /// Build a snapshot from `input` and swap it in. Returns its version.
    pub fn load(&self, input: LoadInput<'_>) -> Result<u64, Diagnostics> {
        // One load at a time, so versions and history stay in order.
        let _guard = self.reload_lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let version = self.last_version.load(Ordering::Relaxed) + 1;
        match Snapshot::build(version, input, &self.registry, &self.detached) {
            Ok(snapshot) => {
                self.last_version.store(version, Ordering::Relaxed);
                let old = self.current.swap(Arc::new(snapshot));
                if let Ok(mut history) = self.history.lock() {
                    history.push_back(old);
                    while history.len() > self.history_len {
                        history.pop_front();
                    }
                }
                metrics::counter!("forkline_reloads_total", "result" => "ok").increment(1);
                metrics::gauge!("forkline_snapshot_version").set(version as f64);
                tracing::info!(version, "snapshot swapped in");
                Ok(version)
            }
            Err(d) => {
                metrics::counter!("forkline_reloads_total", "result" => "error").increment(1);
                tracing::warn!(problems = d.len(), "reload rejected; the running snapshot stays");
                Err(d)
            }
        }
    }

    /// Swap back to the previous snapshot. Returns its version, or `None`
    /// when there is nothing to roll back to.
    pub fn rollback(&self) -> Option<u64> {
        let _guard = self.reload_lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let previous = self.history.lock().ok()?.pop_back()?;
        let version = previous.version;
        self.current.store(previous);
        metrics::counter!("forkline_reloads_total", "result" => "rollback").increment(1);
        metrics::gauge!("forkline_snapshot_version").set(version as f64);
        tracing::warn!(version, "rolled back");
        Some(version)
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
        let Request { body, env } = request;
        let now = Instant::now();
        let budget_deadline = env.received_at + exec.compiled.time_budget;
        let deadline = env.inbound_deadline.map_or(budget_deadline, |d| d.min(budget_deadline));

        // Stale-request shedding: nothing runs for a request that is already
        // late or that waited too long to start.
        let max_queue = self.max_queue_nanos.load(Ordering::Relaxed);
        let waited = now.saturating_duration_since(env.received_at);
        if now >= deadline || (max_queue > 0 && waited.as_nanos() > u128::from(max_queue)) {
            exec.metrics.record(Outcome::Shed, waited);
            let reason = if now >= deadline { "deadline passed before start" } else { "waited too long to start" };
            return Err(EngineError::Shed { plan: plan.to_owned(), reason });
        }

        let id = RequestId(self.next_request.fetch_add(1, Ordering::Relaxed));
        let span = tracing::info_span!(
            "request",
            request_id = %id,
            plan = %exec.compiled.name,
            snapshot_version = snapshot.version,
            config_hash = %format_args!("{:016x}", snapshot.config_hash),
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
        let metrics = exec.metrics.clone();
        let result = exec::run(exec, cx, body, &self.detached).instrument(span).await;
        metrics.record(Outcome::of(&result), now.elapsed());
        result
    }
}
