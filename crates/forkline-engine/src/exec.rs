//! Running one request through a plan: segments, divergence, joins.

use std::any::Any;
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use bytes::Bytes;
use forkline_core::node::BoxFuture;
use forkline_core::{Ctx, ErrorKind, FixedBitSet, NodeError, NodeImpl, NodeKind, PlanError, Reply, RequestMap};
use forkline_plan::{NodeId, OnConflict};
use futures_util::FutureExt;
use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tokio_util::task::{AbortOnDropHandle, TaskTracker};
use tracing::Instrument;

use crate::EngineError;
use crate::analysis::ExecPlan;
use crate::merge::merge_paths;

/// How a segment stopped.
enum SegEnd {
    /// A fork, branch or multibranch ran; continue on these children.
    Diverge { node: NodeId, map: RequestMap, children: Vec<NodeId> },
    /// The next node is a join.
    Arrive { join: NodeId, map: RequestMap },
    /// The encode node produced the reply.
    Reply(Reply),
    /// A leaf (detached path end), or a branch that chose nothing.
    Ended,
    /// The segment's cancellation token fired.
    Cancelled,
    /// A node failed.
    Failed { node: NodeId, error: PlanError, map: RequestMap },
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        format!("panicked: {s}")
    } else if let Some(s) = payload.downcast_ref::<String>() {
        format!("panicked: {s}")
    } else {
        "panicked".to_owned()
    }
}

/// Run a synchronous node body, turning a panic into an error.
fn guarded<T>(span: &tracing::Span, f: impl FnOnce() -> Result<T, NodeError>) -> Result<T, NodeError> {
    span.in_scope(|| {
        std::panic::catch_unwind(AssertUnwindSafe(f))
            .unwrap_or_else(|p| Err(NodeError::new(ErrorKind::Panic, panic_message(&*p))))
    })
}

enum Step {
    Next,
    Labels(Vec<&'static str>),
    Reply(Reply),
}

/// Run nodes from `start` until the path diverges, reaches a join, the encode
/// node or a leaf, or fails.
async fn segment(
    plan: Arc<ExecPlan>,
    cx: Ctx,
    cancel: CancellationToken,
    start: NodeId,
    mut map: RequestMap,
    body: Option<Bytes>,
) -> SegEnd {
    let empty = Bytes::new();
    let body = body.as_ref().unwrap_or(&empty);
    let mut id = start;
    loop {
        let Some(node) = plan.compiled.node(id) else {
            let error = PlanError { kind: ErrorKind::Internal, node: Arc::from("?"), message: format!("no node {id}") };
            return SegEnd::Failed { node: id, error, map };
        };
        let fail = |kind: ErrorKind, message: &str, map: RequestMap| SegEnd::Failed {
            node: id,
            error: PlanError { kind, node: node.name.clone(), message: message.to_owned() },
            map,
        };
        if cancel.is_cancelled() {
            return SegEnd::Cancelled;
        }
        if Instant::now() >= cx.deadline() {
            return fail(ErrorKind::Deadline, "deadline passed before the node started", map);
        }
        let span = tracing::info_span!("node", node = %node.name, module = %node.module, kind = node.kind().as_str());
        let slots = &node.slots;
        let started = std::time::Instant::now();
        let step: Result<Step, NodeError> = match &node.imp {
            NodeImpl::Decode(m) => guarded(&span, || m.dyn_decode(&cx, body, &mut map, slots)).map(|()| Step::Next),
            NodeImpl::Module(m) => {
                let call = AssertUnwindSafe(m.dyn_call(&cx, &mut map, slots)).catch_unwind();
                let timed = tokio::time::timeout_at(cx.deadline(), call).instrument(span);
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => return SegEnd::Cancelled,
                    outcome = timed => match outcome {
                        Err(_elapsed) => Err(NodeError::new(ErrorKind::Deadline, "deadline passed while the node was running")),
                        Ok(Err(panic)) => Err(NodeError::new(ErrorKind::Panic, panic_message(&*panic))),
                        Ok(Ok(result)) => result.map(|()| Step::Next),
                    },
                }
            }
            NodeImpl::Branch(m) => guarded(&span, || m.dyn_choose(&cx, &mut map, slots)).map(|l| Step::Labels(vec![l])),
            NodeImpl::MultiBranch(m) => guarded(&span, || m.dyn_choose(&cx, &mut map, slots)).map(Step::Labels),
            NodeImpl::Encode(m) => guarded(&span, || m.dyn_encode(&cx, &mut map, slots)).map(Step::Reply),
            NodeImpl::Join(_) | NodeImpl::Terminal(_) => {
                return fail(ErrorKind::Internal, "join and terminal nodes are not run inside a path", map);
            }
        };
        plan.metrics.node(id, started.elapsed());
        let next = match step {
            Err(e) => {
                count_error(&plan, &node.name, e.kind());
                return SegEnd::Failed { node: id, error: PlanError::from_node(node.name.clone(), &e), map };
            }
            Ok(Step::Reply(reply)) => return SegEnd::Reply(reply),
            Ok(Step::Next) => match node.children.as_slice() {
                [] => return SegEnd::Ended,
                [only] => only.to,
                many => return SegEnd::Diverge { node: id, map, children: many.iter().map(|e| e.to).collect() },
            },
            Ok(Step::Labels(labels)) => {
                let mut chosen = Vec::with_capacity(labels.len());
                for label in labels {
                    match node.children.iter().find(|e| e.label.as_deref() == Some(label)) {
                        Some(edge) if !chosen.contains(&edge.to) => chosen.push(edge.to),
                        Some(_) => {}
                        None => {
                            let known: Vec<&str> = node.children.iter().filter_map(|e| e.label.as_deref()).collect();
                            let message = format!("returned unknown label `{label}`; links are labelled {known:?}");
                            return fail(ErrorKind::Module, &message, map);
                        }
                    }
                }
                if node.children.len() >= 2 {
                    return SegEnd::Diverge { node: id, map, children: chosen };
                }
                match chosen.first() {
                    Some(&only) => only,
                    None => return SegEnd::Ended,
                }
            }
        };
        if plan.compiled.node(next).is_some_and(|n| n.kind() == NodeKind::Join) {
            return SegEnd::Arrive { join: next, map };
        }
        id = next;
    }
}

fn count_error(plan: &ExecPlan, node: &str, kind: ErrorKind) {
    metrics::counter!(
        "forkline_node_errors_total",
        "plan" => plan.compiled.name.to_string(),
        "node" => node.to_owned(),
        "kind" => kind.as_str(),
    )
    .increment(1);
}

/// The map at a divergence point, which joins it owns merge against.
struct Frame {
    base: RequestMap,
    /// Slots written before the divergence, relative to the enclosing frame.
    prefix_dirty: FixedBitSet,
}

struct JoinState {
    merged: RequestMap,
    stack: Vec<NodeId>,
    arrivals: Vec<RequestMap>,
    fired: bool,
}

struct SegInfo {
    start: NodeId,
    cancel: CancellationToken,
    stack: Vec<NodeId>,
}

/// A failure that answers the request through the terminal node.
type Failure = (PlanError, RequestMap);

/// One request's scheduler state.
struct Run {
    plan: Arc<ExecPlan>,
    cx: Ctx,
    frames: Vec<Option<Frame>>,
    joins: HashMap<NodeId, JoinState>,
    running: FuturesUnordered<BoxFuture<'static, (u64, SegEnd)>>,
    segments: HashMap<u64, SegInfo>,
    next_segment: u64,
    reply: Option<Reply>,
    replied: bool,
}

impl Run {
    fn new(plan: Arc<ExecPlan>, cx: Ctx) -> Self {
        let n = plan.compiled.nodes.len();
        Self {
            plan,
            cx,
            frames: std::iter::repeat_with(|| None).take(n).collect(),
            joins: HashMap::new(),
            running: FuturesUnordered::new(),
            segments: HashMap::new(),
            next_segment: 0,
            reply: None,
            replied: false,
        }
    }

    fn empty_map(&self) -> RequestMap {
        RequestMap::with_slots(self.plan.compiled.keys.len())
    }

    fn node_name(&self, id: NodeId) -> Arc<str> {
        self.plan.compiled.node(id).map_or_else(|| Arc::from("?"), |n| n.name.clone())
    }

    /// Start a path at `start`, or deliver it if `start` is a join.
    fn start_path(
        &mut self,
        start: NodeId,
        map: RequestMap,
        stack: Vec<NodeId>,
        body: Option<Bytes>,
        spawn: bool,
    ) -> Result<(), Failure> {
        if self.plan.compiled.node(start).is_some_and(|n| n.kind() == NodeKind::Join) {
            return self.arrive(start, map, stack);
        }
        let id = self.next_segment;
        self.next_segment += 1;
        let cancel = self.cx.cancellation().child_token();
        let fut = segment(self.plan.clone(), self.cx.clone(), cancel.clone(), start, map, body);
        if spawn {
            let handle = AbortOnDropHandle::new(tokio::spawn(fut.instrument(self.cx.span().clone())));
            let slots = self.plan.compiled.keys.len();
            let name = self.node_name(start);
            self.running.push(Box::pin(async move {
                let end = handle.await.unwrap_or_else(|e| SegEnd::Failed {
                    node: start,
                    error: PlanError {
                        kind: ErrorKind::Internal,
                        node: name,
                        message: format!("path task failed: {e}"),
                    },
                    map: RequestMap::with_slots(slots),
                });
                (id, end)
            }));
        } else {
            self.running.push(Box::pin(async move { (id, fut.await) }));
        }
        self.segments.insert(id, SegInfo { start, cancel, stack });
        Ok(())
    }

    /// Continue from `from` on `children`, opening a frame if `from` diverges.
    fn continue_from(
        &mut self,
        from: NodeId,
        map: RequestMap,
        mut stack: Vec<NodeId>,
        children: Vec<NodeId>,
    ) -> Result<(), Failure> {
        let Some(node) = self.plan.compiled.node(from) else { return Ok(()) };
        let spawn = node.spawn;
        if node.children.len() < 2 {
            // Not a divergence point: the single child continues on the same map.
            if let Some(&child) = children.first() {
                self.start_path(child, map, stack, None, false)?;
            }
            return Ok(());
        }
        let base = map.clean_copy();
        let prefix_dirty = map.dirty_bits().clone();
        stack.push(from);
        // The frame must exist before any child starts: a child that is a
        // join is delivered at once and merges against this base.
        if let Some(slot) = self.frames.get_mut(from) {
            *slot = Some(Frame { base: base.clone(), prefix_dirty });
        }
        for &child in &children {
            self.start_path(child, base.clone(), stack.clone(), None, spawn)?;
        }
        Ok(())
    }

    /// Deliver a path to a join.
    fn arrive(&mut self, join: NodeId, mut map: RequestMap, mut stack: Vec<NodeId>) -> Result<(), Failure> {
        let plan = self.plan.clone();
        let Some(node) = plan.compiled.node(join) else { return Ok(()) };
        if self.joins.get(&join).is_some_and(|j| j.fired) {
            tracing::debug!(join = %node.name, "path arrived after the join fired; discarded");
            return Ok(());
        }
        let Some(owner) = plan.owner.get(join).copied().flatten() else {
            return self.fail_at(join, ErrorKind::Internal, "join has no owning divergence point".into(), map);
        };
        // Make the path's changes relative to the owner's base.
        while let Some(&top) = stack.last() {
            if top == owner {
                break;
            }
            stack.pop();
            if let Some(Some(frame)) = self.frames.get(top) {
                map.union_dirty(&frame.prefix_dirty);
            }
        }
        if stack.last() != Some(&owner) {
            return self.fail_at(join, ErrorKind::Internal, "path did not pass the join's owner".into(), map);
        }
        if !self.joins.contains_key(&join) {
            let Some(Some(frame)) = self.frames.get(owner) else {
                return self.fail_at(join, ErrorKind::Internal, "owner frame missing".into(), map);
            };
            let state =
                JoinState { merged: frame.base.clone(), stack: stack.clone(), arrivals: Vec::new(), fired: false };
            self.joins.insert(join, state);
        }
        let NodeImpl::Join(module) = &node.imp else {
            return self.fail_at(join, ErrorKind::Internal, "not a join module".into(), map);
        };
        let span = tracing::info_span!("node", node = %node.name, module = %node.module, kind = "join");
        let step = match self.joins.get_mut(&join) {
            Some(state) => guarded(&span, || module.dyn_on_path(&self.cx, &mut map, &mut state.merged, &node.slots)),
            None => return Ok(()),
        };
        match step {
            Err(e) => {
                let error = PlanError::from_node(node.name.clone(), &e);
                self.failure(join, error, map)
            }
            Ok(step) => {
                if let Some(state) = self.joins.get_mut(&join) {
                    state.arrivals.push(map);
                }
                if step == forkline_core::JoinStep::Done {
                    // Cancel every running path that can still reach this join.
                    for info in self.segments.values() {
                        if plan.reaches(info.start, join) {
                            info.cancel.cancel();
                        }
                    }
                    self.fire(join)?;
                }
                Ok(())
            }
        }
    }

    /// Merge a join's arrivals and continue past it.
    fn fire(&mut self, join: NodeId) -> Result<(), Failure> {
        let plan = self.plan.clone();
        let Some(state) = self.joins.get_mut(&join) else { return Ok(()) };
        if state.fired {
            return Ok(());
        }
        state.fired = true;
        let mut merged = std::mem::replace(&mut state.merged, RequestMap::with_slots(0));
        let arrivals = std::mem::take(&mut state.arrivals);
        let stack = std::mem::take(&mut state.stack);
        let on_conflict = plan.compiled.on_conflict;
        let conflicts = merge_paths(&mut merged, &arrivals, on_conflict);
        let name = self.node_name(join);
        if !conflicts.is_empty() {
            let keys: Vec<&str> =
                conflicts.iter().map(|&s| plan.compiled.keys.info(s).map_or("?", |k| &*k.name)).collect();
            tracing::warn!(join = %name, keys = ?keys, policy = ?on_conflict, "merge conflict");
            for key in &keys {
                metrics::counter!(
                    "forkline_merge_conflicts_total",
                    "plan" => plan.compiled.name.to_string(),
                    "node" => name.to_string(),
                    "key" => (*key).to_owned(),
                )
                .increment(1);
            }
            if on_conflict == OnConflict::Error {
                let message = format!("keys changed on more than one path: {}", keys.join(", "));
                return self.fail_at(join, ErrorKind::MergeConflict, message, merged);
            }
        }
        let children: Vec<NodeId> =
            plan.compiled.node(join).map(|n| n.children.iter().map(|e| e.to).collect()).unwrap_or_default();
        self.continue_from(join, merged, stack, children)
    }

    /// Fire every join that has arrivals and that no live path can still reach.
    fn fire_ready(&mut self) -> Result<(), Failure> {
        loop {
            let plan = &self.plan;
            let ready = self.joins.iter().find_map(|(&j, state)| {
                if state.fired || state.arrivals.is_empty() {
                    return None;
                }
                let segment_can_reach = self.segments.values().any(|s| s.start == j || plan.reaches(s.start, j));
                let join_can_reach =
                    self.joins.iter().any(|(&k, o)| k != j && !o.fired && !o.arrivals.is_empty() && plan.reaches(k, j));
                (!segment_can_reach && !join_can_reach).then_some(j)
            });
            match ready {
                Some(j) => self.fire(j)?,
                None => return Ok(()),
            }
        }
    }

    /// Handle a request-level failure at `node`, or log it if the node is detached.
    fn failure(&mut self, node: NodeId, error: PlanError, map: RequestMap) -> Result<(), Failure> {
        if self.replied || !self.plan.leads_to_encode(node) {
            tracing::warn!(kind = %error.kind, node = %error.node, message = %error.message, "detached path failed");
            return Ok(());
        }
        Err((error, map))
    }

    fn fail_at(&mut self, node: NodeId, kind: ErrorKind, message: String, map: RequestMap) -> Result<(), Failure> {
        let error = PlanError { kind, node: self.node_name(node), message };
        self.failure(node, error, map)
    }

    fn handle(&mut self, id: u64, end: SegEnd) -> Result<(), Failure> {
        let Some(info) = self.segments.remove(&id) else { return Ok(()) };
        self.after(info.stack, end)
    }

    /// Act on how a segment ended. `stack` is the segment's divergence stack.
    fn after(&mut self, stack: Vec<NodeId>, end: SegEnd) -> Result<(), Failure> {
        match end {
            SegEnd::Diverge { node, map, children } => self.continue_from(node, map, stack, children)?,
            SegEnd::Arrive { join, map } => self.arrive(join, map, stack)?,
            SegEnd::Reply(reply) => {
                self.replied = true;
                self.reply = Some(reply);
            }
            SegEnd::Ended | SegEnd::Cancelled => {}
            SegEnd::Failed { node, error, map } => self.failure(node, error, map)?,
        }
        self.fire_ready()
    }

    /// True while some running path or waiting join can still reach the encode node.
    fn reply_possible(&self) -> bool {
        self.reply.is_some()
            || self.segments.values().any(|s| self.plan.leads_to_encode(s.start))
            || self.joins.iter().any(|(&j, s)| !s.fired && !s.arrivals.is_empty() && self.plan.leads_to_encode(j))
    }

    async fn until_reply(&mut self) -> Result<Reply, Failure> {
        loop {
            if let Some(reply) = self.reply.take() {
                return Ok(reply);
            }
            if !self.reply_possible() {
                let error = PlanError {
                    kind: ErrorKind::Internal,
                    node: self.plan.compiled.name.clone(),
                    message: "no running path can reach the encode node".into(),
                };
                return Err((error, self.empty_map()));
            }
            let deadline = self.cx.deadline();
            let next = tokio::select! {
                biased;
                next = self.running.next() => next,
                () = tokio::time::sleep_until(deadline) => {
                    let error = PlanError {
                        kind: ErrorKind::Deadline,
                        node: self.plan.compiled.name.clone(),
                        message: "deadline passed while paths were running".into(),
                    };
                    return Err((error, self.empty_map()));
                }
            };
            if let Some((id, end)) = next {
                self.handle(id, end)?;
            }
        }
    }

    /// Run detached paths to completion after the reply has gone out.
    async fn drain(mut self) {
        let deadline = self.cx.deadline();
        while !self.running.is_empty() {
            let next = tokio::select! {
                next = self.running.next() => next,
                () = tokio::time::sleep_until(deadline) => {
                    tracing::warn!(paths = self.segments.len(), "detached paths cancelled at the deadline");
                    self.cx.cancellation().cancel();
                    return;
                }
            };
            let Some((id, end)) = next else { continue };
            if let Err((error, _)) = self.handle(id, end) {
                tracing::warn!(kind = %error.kind, node = %error.node, message = %error.message, "detached path failed");
            }
        }
    }
}

pub(crate) async fn run(
    plan: Arc<ExecPlan>,
    cx: Ctx,
    body: Bytes,
    detached: &TaskTracker,
) -> Result<Reply, EngineError> {
    // The first segment runs directly: a plan that never diverges (a straight
    // chain) finishes here without any scheduler state.
    let root = plan.compiled.root;
    let map = RequestMap::with_slots(plan.compiled.keys.len());
    let first = segment(plan.clone(), cx.clone(), cx.cancellation().clone(), root, map, Some(body)).await;
    let failure = match first {
        SegEnd::Reply(reply) => return Ok(reply),
        SegEnd::Failed { error, map, .. } => (error, map),
        other => {
            let mut run = Run::new(plan.clone(), cx.clone());
            let outcome = match run.after(Vec::new(), other) {
                Ok(()) => run.until_reply().await,
                Err(failure) => Err(failure),
            };
            match outcome {
                Ok(reply) => {
                    if !run.running.is_empty() {
                        detached.spawn(run.drain().instrument(cx.span().clone()));
                    }
                    return Ok(reply);
                }
                Err(failure) => failure,
            }
        }
    };
    // Every path still running is dropped with `run`; cancel spawned ones too.
    cx.cancellation().cancel();
    let (error, mut map) = failure;
    if error.kind == ErrorKind::BadRequest {
        tracing::debug!(kind = %error.kind, node = %error.node, message = %error.message, "plan failed");
    } else {
        tracing::warn!(kind = %error.kind, node = %error.node, message = %error.message, "plan failed");
    }
    run_terminal(&plan, &cx, &mut map, error)
}

fn run_terminal(plan: &ExecPlan, cx: &Ctx, map: &mut RequestMap, error: PlanError) -> Result<Reply, EngineError> {
    let Some(node) = plan.compiled.node(plan.compiled.terminal) else {
        return Err(EngineError::TerminalFailed {
            error,
            terminal: Arc::from("?"),
            message: "terminal node id out of range".into(),
        });
    };
    let NodeImpl::Terminal(terminal) = &node.imp else {
        return Err(EngineError::TerminalFailed {
            error,
            terminal: node.name.clone(),
            message: "not a terminal module".into(),
        });
    };
    let span = tracing::info_span!("node", node = %node.name, module = %node.module, kind = "terminal");
    guarded(&span, || terminal.dyn_respond(cx, map, &node.slots, &error)).map_err(|e| EngineError::TerminalFailed {
        error,
        terminal: node.name.clone(),
        message: e.message().to_owned(),
    })
}
