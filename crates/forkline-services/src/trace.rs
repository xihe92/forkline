//! Trace-context forwarding and per-service call timing.

use std::time::Duration;

use forkline_core::Ctx;
use http::HeaderMap;
use metrics::Histogram;

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

/// `forkline_service_call_duration_seconds{service, outcome}` handles.
#[derive(Clone, Debug)]
pub(crate) struct CallTimings {
    ok: Histogram,
    error: Histogram,
}

impl CallTimings {
    pub(crate) fn new(service: &str) -> Self {
        let name = "forkline_service_call_duration_seconds";
        Self {
            ok: metrics::histogram!(name, "service" => service.to_owned(), "outcome" => "ok"),
            error: metrics::histogram!(name, "service" => service.to_owned(), "outcome" => "error"),
        }
    }

    pub(crate) fn record(&self, ok: bool, elapsed: Duration) {
        if ok { &self.ok } else { &self.error }.record(elapsed.as_secs_f64());
    }
}
