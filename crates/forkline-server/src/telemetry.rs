//! Logs, traces and metrics setup.
//!
//! * Logs: `tracing-subscriber`, text or JSON, filtered by `filter` (or `RUST_LOG`).
//! * Traces (feature `otlp`): spans exported over OTLP/gRPC to `otlp_endpoint`;
//!   inbound W3C `traceparent` becomes each request's parent.
//! * tokio-console (feature `console`, built with `--cfg tokio_unstable`).
//! * Metrics: a registry on the official Prometheus client (`prometheus-client`).

use http::HeaderMap;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry};

use crate::config::{LogFormat, TelemetryConfig};

type BoxedLayer = Box<dyn Layer<Registry> + Send + Sync>;

fn filter(config: &TelemetryConfig) -> EnvFilter {
    EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&config.filter))
        .unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Keeps telemetry exporters alive; call [`shutdown`](Self::shutdown) to flush.
#[must_use = "dropping the guard without shutdown may lose buffered spans"]
#[derive(Debug, Default)]
pub struct TelemetryGuard {
    #[cfg(feature = "otlp")]
    provider: Option<opentelemetry_sdk::trace::SdkTracerProvider>,
}

impl TelemetryGuard {
    /// Flush and stop exporters.
    pub fn shutdown(self) {
        #[cfg(feature = "otlp")]
        if let Some(Err(e)) = self.provider.map(|p| p.shutdown()) {
            eprintln!("warning: trace exporter shutdown: {e}");
        }
    }
}

/// Install the global subscriber. `RUST_LOG` overrides the configured filter.
/// Does nothing if a subscriber is already installed (for example in tests).
pub fn init(config: &TelemetryConfig) -> TelemetryGuard {
    let mut layers: Vec<BoxedLayer> = Vec::new();
    match config.log_format {
        LogFormat::Json => layers.push(
            tracing_subscriber::fmt::layer()
                .json()
                .with_current_span(true)
                .with_span_list(true)
                .with_filter(filter(config))
                .boxed(),
        ),
        LogFormat::Text => layers.push(tracing_subscriber::fmt::layer().with_filter(filter(config)).boxed()),
    }
    #[allow(unused_mut)]
    let mut guard = TelemetryGuard::default();
    #[cfg(feature = "otlp")]
    {
        guard.provider = otlp::layer(config, &mut layers);
    }
    #[cfg(feature = "console")]
    layers.push(console_subscriber::ConsoleLayer::builder().with_default_env().spawn().boxed());
    if config.otlp_endpoint.is_some() && !cfg!(feature = "otlp") {
        eprintln!("warning: telemetry.otlp_endpoint is set but this binary was built without the `otlp` feature");
    }
    if tracing_subscriber::registry().with(layers).try_init().is_err() {
        tracing::debug!("a tracing subscriber was already installed");
    }
    guard
}

/// A span for one inbound request. With the `otlp` feature, an inbound W3C
/// `traceparent` header becomes its parent, so the request joins the
/// caller's trace.
pub fn inbound_span(protocol: &'static str, headers: &HeaderMap) -> tracing::Span {
    let span = tracing::info_span!("inbound", protocol);
    #[cfg(feature = "otlp")]
    otlp::continue_trace(&span, headers);
    #[cfg(not(feature = "otlp"))]
    let _ = headers;
    span
}

#[cfg(feature = "otlp")]
mod otlp {
    use http::HeaderMap;
    use opentelemetry::propagation::Extractor;
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::propagation::TraceContextPropagator;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    use tracing_subscriber::Layer;

    use super::{BoxedLayer, filter};
    use crate::config::TelemetryConfig;

    pub(super) fn layer(config: &TelemetryConfig, layers: &mut Vec<BoxedLayer>) -> Option<SdkTracerProvider> {
        let endpoint = config.otlp_endpoint.as_ref()?;
        let exporter =
            match opentelemetry_otlp::SpanExporter::builder().with_tonic().with_endpoint(endpoint.clone()).build() {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("warning: OTLP export disabled: {e}");
                    return None;
                }
            };
        let resource = Resource::builder().with_service_name(config.service_name.clone()).build();
        let provider = SdkTracerProvider::builder().with_batch_exporter(exporter).with_resource(resource).build();
        let tracer = provider.tracer("forkline");
        opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());
        opentelemetry::global::set_tracer_provider(provider.clone());
        layers.push(tracing_opentelemetry::layer().with_tracer(tracer).with_filter(filter(config)).boxed());
        Some(provider)
    }

    struct Headers<'a>(&'a HeaderMap);

    impl Extractor for Headers<'_> {
        fn get(&self, key: &str) -> Option<&str> {
            self.0.get(key).and_then(|v| v.to_str().ok())
        }

        fn keys(&self) -> Vec<&str> {
            self.0.keys().map(http::HeaderName::as_str).collect()
        }
    }

    pub(super) fn continue_trace(span: &tracing::Span, headers: &HeaderMap) {
        let cx = opentelemetry::global::get_text_map_propagator(|p| p.extract(&Headers(headers)));
        let _ = span.set_parent(cx);
    }
}

/// A Prometheus registry holding every Forkline metric: engine, service
/// clients, listeners and thread pools. `GET /metrics` on the admin listener renders it.
pub fn metrics_registry() -> std::sync::Arc<prometheus_client::registry::Registry> {
    let mut registry = prometheus_client::registry::Registry::default();
    forkline_engine::metrics::register(&mut registry);
    forkline_services::metrics::register(&mut registry);
    crate::admission::register(&mut registry);
    forkline_core::pool::register_metrics(&mut registry);
    crate::task_polls::register(&mut registry);
    #[cfg(feature = "jemalloc")]
    crate::allocator::register(&mut registry);
    std::sync::Arc::new(registry)
}

#[cfg(all(test, feature = "otlp"))]
mod tests {
    use opentelemetry::trace::{TraceContextExt, TracerProvider as _};
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn an_inbound_traceparent_becomes_the_parent() {
        opentelemetry::global::set_text_map_propagator(opentelemetry_sdk::propagation::TraceContextPropagator::new());
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
        let subscriber =
            tracing_subscriber::registry().with(tracing_opentelemetry::layer().with_tracer(provider.tracer("test")));
        tracing::subscriber::with_default(subscriber, || {
            let mut headers = http::HeaderMap::new();
            headers.insert(
                "traceparent",
                http::HeaderValue::from_static("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"),
            );
            let span = super::inbound_span("http", &headers);
            let cx = span.context();
            assert_eq!(cx.span().span_context().trace_id().to_string(), "4bf92f3577b34da6a3ce929d0e0e4736");
        });
    }
}
