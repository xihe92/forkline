//! The Forkline plan executor.
//!
//! [`Engine`] holds the current [`Snapshot`] of compiled plans in an
//! `arc-swap` cell. Each request pins the snapshot it started with and keeps
//! it until it finishes, so a reload never mixes two configurations inside
//! one request.
//!
//! Phase 1 executes straight chains (decode, modules, encode). Plans that
//! fork, branch or join pass validation but are refused by the engine until
//! phase 2 adds the scheduler.

#![forbid(unsafe_code)]

mod exec;
mod service;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_swap::ArcSwap;
use bytes::Bytes;
use forkline_core::{Ctx, PlanError, Registry, Reply, RequestEnv, RequestId};
use forkline_plan::{CompiledPlan, Diagnostic, Diagnostics, NodeId, PlansFile, compile_all};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

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

/// A compiled plan plus the engine's precomputed execution order.
#[derive(Debug)]
pub struct ExecPlan {
    /// The validated plan.
    pub compiled: CompiledPlan,
    /// Node order from root to encode (phase 1: plans are straight chains).
    pub(crate) chain: Vec<NodeId>,
}

impl ExecPlan {
    fn new(compiled: CompiledPlan) -> Result<Self, Diagnostic> {
        let name = compiled.name.to_string();
        let mut chain = Vec::with_capacity(compiled.nodes.len());
        let mut current = Some(compiled.root);
        while let Some(id) = current {
            let Some(node) = compiled.node(id) else { break };
            if node.children.len() > 1 || node.parents.len() > 1 {
                return Err(Diagnostic {
                    plan: name,
                    node: Some(node.name.to_string()),
                    message:
                        "forks, branches and joins are not executed yet (phase 2); this engine runs straight chains"
                            .into(),
                });
            }
            chain.push(id);
            current = node.children.first().map(|e| e.to);
        }
        Ok(Self { compiled, chain })
    }
}

/// An immutable set of compiled plans.
#[derive(Debug)]
pub struct Snapshot {
    /// Increases by one on every successful load.
    pub version: u64,
    /// Plans by name.
    pub plans: BTreeMap<String, Arc<ExecPlan>>,
}

impl Snapshot {
    fn build(version: u64, file: &PlansFile, registry: &Registry) -> Result<Self, Diagnostics> {
        let compiled = compile_all(file, registry)?;
        let mut plans = BTreeMap::new();
        let mut diags = Vec::new();
        for (name, plan) in compiled {
            let plan = Arc::try_unwrap(plan).unwrap_or_else(|shared| (*shared).clone());
            match ExecPlan::new(plan) {
                Ok(p) => {
                    plans.insert(name, Arc::new(p));
                }
                Err(d) => diags.push(d),
            }
        }
        if diags.is_empty() { Ok(Self { version, plans }) } else { Err(Diagnostics(diags)) }
    }
}

/// Runs plans.
#[derive(Debug)]
pub struct Engine {
    registry: Arc<Registry>,
    current: ArcSwap<Snapshot>,
    next_request: AtomicU64,
}

impl Engine {
    /// Compile `plans` against `registry`. Fails with every problem found.
    pub fn new(registry: Arc<Registry>, plans: &PlansFile) -> Result<Self, Diagnostics> {
        let snapshot = Snapshot::build(1, plans, &registry)?;
        Ok(Self { registry, current: ArcSwap::from_pointee(snapshot), next_request: AtomicU64::new(1) })
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
        let version = self.current.load().version + 1;
        let snapshot = Snapshot::build(version, plans, &self.registry)?;
        self.current.store(Arc::new(snapshot));
        tracing::info!(version, "plans reloaded");
        Ok(version)
    }

    /// A `tower::Service` that runs one plan.
    pub fn plan_service(self: &Arc<Self>, plan: impl Into<Arc<str>>) -> PlanService {
        PlanService::new(self.clone(), plan.into())
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
        let cx = Ctx::new(id, exec.compiled.name.clone(), span.clone(), deadline, CancellationToken::new(), env);
        exec::run(&exec, &cx, body).instrument(span).await
    }
}
