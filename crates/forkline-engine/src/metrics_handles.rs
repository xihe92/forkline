//! Metric handles created once per plan load, so recording on the request
//! path is a few atomic operations with no label lookups.
//!
//! Handles bind to the global `metrics` recorder that is installed when the
//! plan is loaded; with no recorder they do nothing.

use std::time::Duration;

use forkline_core::{ErrorKind, Reply, Status};
use metrics::{Counter, Histogram};

use crate::EngineError;

/// How a request ended, for `forkline_requests_total{outcome}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Ok,
    Error,
    BadRequest,
    Deadline,
    Shed,
}

impl Outcome {
    const ALL: [Outcome; 5] = [Outcome::Ok, Outcome::Error, Outcome::BadRequest, Outcome::Deadline, Outcome::Shed];

    fn label(self) -> &'static str {
        match self {
            Outcome::Ok => "ok",
            Outcome::Error => "error",
            Outcome::BadRequest => "bad_request",
            Outcome::Deadline => "deadline",
            Outcome::Shed => "shed",
        }
    }

    pub(crate) fn of(result: &Result<Reply, EngineError>) -> Self {
        match result {
            Ok(reply) => match reply.status {
                Status::Ok => Outcome::Ok,
                Status::Error(ErrorKind::Deadline) => Outcome::Deadline,
                Status::Error(ErrorKind::BadRequest) => Outcome::BadRequest,
                Status::Error(_) => Outcome::Error,
            },
            Err(EngineError::Shed { .. }) => Outcome::Shed,
            Err(_) => Outcome::Error,
        }
    }
}

/// Handles for one plan.
#[derive(Debug)]
pub(crate) struct PlanMetrics {
    requests: Vec<Counter>,
    duration: Histogram,
    nodes: Vec<Histogram>,
}

impl PlanMetrics {
    pub(crate) fn new(plan: &str, nodes: &[&str]) -> Self {
        let requests = Outcome::ALL
            .iter()
            .map(|o| metrics::counter!("forkline_requests_total", "plan" => plan.to_owned(), "outcome" => o.label()))
            .collect();
        let duration = metrics::histogram!("forkline_request_duration_seconds", "plan" => plan.to_owned());
        let nodes = nodes
            .iter()
            .map(|n| {
                metrics::histogram!("forkline_node_duration_seconds", "plan" => plan.to_owned(), "node" => (*n).to_owned())
            })
            .collect();
        Self { requests, duration, nodes }
    }

    pub(crate) fn record(&self, outcome: Outcome, elapsed: Duration) {
        if let Some(c) = Outcome::ALL.iter().position(|o| *o == outcome).and_then(|i| self.requests.get(i)) {
            c.increment(1);
        }
        if outcome != Outcome::Shed {
            self.duration.record(elapsed.as_secs_f64());
        }
    }

    pub(crate) fn node(&self, id: usize, elapsed: Duration) {
        if let Some(h) = self.nodes.get(id) {
            h.record(elapsed.as_secs_f64());
        }
    }
}
