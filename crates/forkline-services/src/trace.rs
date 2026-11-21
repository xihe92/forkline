//! Trace-context forwarding and per-service call timing.

use std::sync::LazyLock;
use std::time::Duration;

use forkline_core::Ctx;
use http::HeaderMap;
use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::histogram::Histogram;
use prometheus_client::registry::Registry;

/// Headers carrying W3C trace context.
const TRACE_HEADERS: [&str; 2] = ["traceparent", "tracestate"];

/// Copy the inbound W3C trace-context headers onto an outbound request,
/// unless the module already set them.
pub(crate) fn forward(cx: &Ctx, headers: &mut HeaderMap) {
    for name in TRACE_HEADERS {
        if headers.contains_key(name) {
            continue;
        }
        if let Some(value) = cx.env().metadata.get(name) {
            headers.insert(name, value.clone());
        }
    }
}

/// Histogram buckets for call durations, in seconds: 0.5 ms to 10 s.
const BUCKETS: [f64; 14] = [0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0];

fn histogram() -> Histogram {
    Histogram::new(BUCKETS)
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct CallLabels {
    service: String,
    outcome: &'static str,
}

type CallHistograms = Family<CallLabels, Histogram, fn() -> Histogram>;

static CALLS: LazyLock<CallHistograms> = LazyLock::new(|| Family::new_with_constructor(histogram as fn() -> Histogram));

/// Add the service-call metrics to `registry`.
pub fn register(registry: &mut Registry) {
    registry.register(
        "forkline_service_call_duration_seconds",
        "Service call duration by service and outcome, in seconds",
        CALLS.clone(),
    );
}

/// `forkline_service_call_duration_seconds{service, outcome}` handles, bound once per client.
#[derive(Clone, Debug)]
pub(crate) struct CallTimings {
    ok: Histogram,
    error: Histogram,
}

impl CallTimings {
    pub(crate) fn new(service: &str) -> Self {
        let get = |outcome| CALLS.get_or_create(&CallLabels { service: service.to_owned(), outcome }).clone();
        Self { ok: get("ok"), error: get("error") }
    }

    pub(crate) fn record(&self, ok: bool, elapsed: Duration) {
        if ok { &self.ok } else { &self.error }.observe(elapsed.as_secs_f64());
    }
}
