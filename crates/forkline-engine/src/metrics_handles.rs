//! Engine metrics on the official Prometheus client (`prometheus-client`).
//!
//! The metric families are process-wide statics: recording works whether or
//! not anything exports them, and a server adds them to its registry with
//! [`register`]. Label values are bound once per plan load, so recording on
//! the request path is a few atomic operations with no label lookups.

use std::sync::LazyLock;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use forkline_core::{ErrorKind, Reply, Status};
use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::Histogram;
use prometheus_client::registry::Registry;

use crate::EngineError;

/// Histogram buckets for durations, in seconds: 0.5 ms to 10 s.
pub const DURATION_BUCKETS: [f64; 14] =
    [0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0];

fn duration_histogram() -> Histogram {
    Histogram::new(DURATION_BUCKETS)
}

type Histograms<L> = Family<L, Histogram, fn() -> Histogram>;

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct RequestLabels {
    plan: String,
    outcome: &'static str,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct PlanLabels {
    plan: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct NodeLabels {
    plan: String,
    node: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct NodeErrorLabels {
    plan: String,
    node: String,
    kind: &'static str,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct ConflictLabels {
    plan: String,
    node: String,
    key: String,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct ResultLabels {
    result: &'static str,
}

struct EngineMetrics {
    requests: Family<RequestLabels, Counter>,
    request_duration: Histograms<PlanLabels>,
    node_duration: Histograms<NodeLabels>,
    node_errors: Family<NodeErrorLabels, Counter>,
    node_slow_polls: Family<NodeLabels, Counter>,
    merge_conflicts: Family<ConflictLabels, Counter>,
    reloads: Family<ResultLabels, Counter>,
    snapshot_version: Gauge,
}

static METRICS: LazyLock<EngineMetrics> = LazyLock::new(|| EngineMetrics {
    requests: Family::default(),
    request_duration: Family::new_with_constructor(duration_histogram as fn() -> Histogram),
    node_duration: Family::new_with_constructor(duration_histogram as fn() -> Histogram),
    node_errors: Family::default(),
    node_slow_polls: Family::default(),
    merge_conflicts: Family::default(),
    reloads: Family::default(),
    snapshot_version: Gauge::default(),
});

/// Add the engine's metrics to `registry`.
pub fn register(registry: &mut Registry) {
    let m = &*METRICS;
    registry.register("forkline_requests", "Requests by plan and outcome", m.requests.clone());
    registry.register(
        "forkline_request_duration_seconds",
        "Request duration by plan, in seconds",
        m.request_duration.clone(),
    );
    registry.register(
        "forkline_node_duration_seconds",
        "Node duration by plan and node, in seconds",
        m.node_duration.clone(),
    );
    registry.register("forkline_node_errors", "Node failures by plan, node and kind", m.node_errors.clone());
    registry.register(
        "forkline_node_slow_polls",
        "Polls that held a worker thread longer than the slow-poll threshold, by plan and node",
        m.node_slow_polls.clone(),
    );
    registry.register(
        "forkline_merge_conflicts",
        "Keys changed on several paths and not resolved by the join",
        m.merge_conflicts.clone(),
    );
    registry.register("forkline_reloads", "Snapshot loads by result", m.reloads.clone());
    registry.register(
        "forkline_snapshot_version",
        "Version of the snapshot serving requests",
        m.snapshot_version.clone(),
    );
}

pub(crate) fn node_error(plan: &str, node: &str, kind: ErrorKind) {
    METRICS
        .node_errors
        .get_or_create(&NodeErrorLabels { plan: plan.to_owned(), node: node.to_owned(), kind: kind.as_str() })
        .inc();
}

pub(crate) fn merge_conflict(plan: &str, node: &str, key: &str) {
    METRICS
        .merge_conflicts
        .get_or_create(&ConflictLabels { plan: plan.to_owned(), node: node.to_owned(), key: key.to_owned() })
        .inc();
}

pub(crate) fn reload(result: &'static str) {
    METRICS.reloads.get_or_create(&ResultLabels { result }).inc();
}

pub(crate) fn snapshot_version(version: u64) {
    METRICS.snapshot_version.set(i64::try_from(version).unwrap_or(i64::MAX));
}

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

/// Handles for one plan, bound to its label values.
#[derive(Debug)]
pub(crate) struct PlanMetrics {
    requests: Vec<Counter>,
    duration: Histogram,
    nodes: Vec<Histogram>,
    slow_polls: Vec<Counter>,
    /// When each node last logged a slow poll (ms since an epoch; 0: never).
    last_warned: Vec<AtomicU64>,
}

impl PlanMetrics {
    pub(crate) fn new(plan: &str, nodes: &[&str]) -> Self {
        let m = &*METRICS;
        let requests = Outcome::ALL
            .iter()
            .map(|o| m.requests.get_or_create(&RequestLabels { plan: plan.to_owned(), outcome: o.label() }).clone())
            .collect();
        let duration = m.request_duration.get_or_create(&PlanLabels { plan: plan.to_owned() }).clone();
        let label = |n: &&str| NodeLabels { plan: plan.to_owned(), node: (*n).to_owned() };
        let slow_polls = nodes.iter().map(|n| m.node_slow_polls.get_or_create(&label(n)).clone()).collect();
        let last_warned = nodes.iter().map(|_| AtomicU64::new(0)).collect();
        let nodes = nodes.iter().map(|n| m.node_duration.get_or_create(&label(n)).clone()).collect();
        Self { requests, duration, nodes, slow_polls, last_warned }
    }

    pub(crate) fn slow_poll(&self, id: usize) {
        if let Some(c) = self.slow_polls.get(id) {
            c.inc();
        }
    }

    pub(crate) fn last_warned(&self, id: usize) -> Option<&AtomicU64> {
        self.last_warned.get(id)
    }

    pub(crate) fn record(&self, outcome: Outcome, elapsed: Duration) {
        if let Some(c) = Outcome::ALL.iter().position(|o| *o == outcome).and_then(|i| self.requests.get(i)) {
            c.inc();
        }
        if outcome != Outcome::Shed {
            self.duration.observe(elapsed.as_secs_f64());
        }
    }

    pub(crate) fn node(&self, id: usize, elapsed: Duration) {
        if let Some(h) = self.nodes.get(id) {
            h.observe(elapsed.as_secs_f64());
        }
    }
}
