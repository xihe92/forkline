//! Running one request through a plan.

use std::sync::Arc;

use bytes::Bytes;
use forkline_core::{Ctx, ErrorKind, NodeImpl, PlanError, Reply, RequestMap};
use forkline_plan::CompiledNode;
use tokio::time::Instant;
use tracing::Instrument;

use crate::{EngineError, ExecPlan};

pub(crate) async fn run(plan: &ExecPlan, cx: &Ctx, body: Bytes) -> Result<Reply, EngineError> {
    let mut map = RequestMap::with_slots(plan.compiled.keys.len());
    match run_chain(plan, cx, &mut map, &body).await {
        Ok(reply) => Ok(reply),
        Err(error) => {
            cx.cancellation().cancel();
            if error.kind == ErrorKind::BadRequest {
                // The caller's mistake, not the server's: keep it out of warning-level logs.
                tracing::debug!(kind = %error.kind, node = %error.node, message = %error.message, "plan failed");
            } else {
                tracing::warn!(kind = %error.kind, node = %error.node, message = %error.message, "plan failed");
            }
            run_terminal(plan, cx, &mut map, error)
        }
    }
}

fn plan_error(node: &CompiledNode, kind: ErrorKind, message: impl Into<String>) -> PlanError {
    PlanError { kind, node: node.name.clone(), message: message.into() }
}

async fn run_chain(plan: &ExecPlan, cx: &Ctx, map: &mut RequestMap, body: &Bytes) -> Result<Reply, PlanError> {
    for &id in &plan.chain {
        let Some(node) = plan.compiled.node(id) else {
            return Err(PlanError {
                kind: ErrorKind::Internal,
                node: Arc::from("?"),
                message: format!("node id {id} out of range"),
            });
        };
        if cx.is_cancelled() {
            return Err(plan_error(node, ErrorKind::Cancelled, "request cancelled"));
        }
        if Instant::now() >= cx.deadline() {
            return Err(plan_error(node, ErrorKind::Deadline, "deadline passed before the node started"));
        }
        let span = tracing::info_span!("node", node = %node.name, module = %node.module, kind = node.kind().as_str());
        let slots = &node.slots;
        match &node.imp {
            NodeImpl::Decode(m) => {
                span.in_scope(|| m.dyn_decode(cx, body, map, slots))
                    .map_err(|e| PlanError::from_node(node.name.clone(), &e))?;
            }
            NodeImpl::Module(m) => {
                let call = tokio::time::timeout_at(cx.deadline(), m.dyn_call(cx, map, slots));
                let outcome = tokio::select! {
                    biased;
                    () = cx.cancellation().cancelled() => None,
                    r = call.instrument(span) => Some(r),
                };
                match outcome {
                    Some(Ok(Ok(()))) => {}
                    Some(Ok(Err(e))) => return Err(PlanError::from_node(node.name.clone(), &e)),
                    Some(Err(_elapsed)) => {
                        return Err(plan_error(
                            node,
                            ErrorKind::Deadline,
                            "deadline passed while the node was running",
                        ));
                    }
                    None => return Err(plan_error(node, ErrorKind::Cancelled, "request cancelled")),
                }
            }
            NodeImpl::Encode(m) => {
                return span
                    .in_scope(|| m.dyn_encode(cx, map, slots))
                    .map_err(|e| PlanError::from_node(node.name.clone(), &e));
            }
            other => {
                return Err(plan_error(
                    node,
                    ErrorKind::Internal,
                    format!("{} nodes are not executed yet (phase 2)", other.kind()),
                ));
            }
        }
    }
    Err(PlanError {
        kind: ErrorKind::Internal,
        node: plan.compiled.name.clone(),
        message: "the plan ended without reaching an encode node".into(),
    })
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
    span.in_scope(|| terminal.dyn_respond(cx, map, &node.slots, &error)).map_err(|e| EngineError::TerminalFailed {
        error,
        terminal: node.name.clone(),
        message: e.message().to_owned(),
    })
}
