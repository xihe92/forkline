//! Logging setup over `tracing-subscriber`.

use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::config::{LogFormat, TelemetryConfig};

/// Install the global subscriber. `RUST_LOG` overrides the configured filter.
/// Does nothing if a subscriber is already installed (for example in tests).
pub fn init(config: &TelemetryConfig) {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&config.filter))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let registry = tracing_subscriber::registry().with(filter);
    let result = match config.log_format {
        LogFormat::Json => registry
            .with(tracing_subscriber::fmt::layer().json().with_current_span(true).with_span_list(true))
            .try_init(),
        LogFormat::Text => registry.with(tracing_subscriber::fmt::layer()).try_init(),
    };
    if result.is_err() {
        tracing::debug!("a tracing subscriber was already installed");
    }
}

/// Install the Prometheus recorder for the `metrics` facade and return the
/// handle `/metrics` renders. Durations use histogram buckets from 0.5 ms to 10 s.
pub fn install_metrics() -> Result<metrics_exporter_prometheus::PrometheusHandle, String> {
    use metrics_exporter_prometheus::{Matcher, PrometheusBuilder};
    const BUCKETS: &[f64] = &[0.0005, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0];
    PrometheusBuilder::new()
        .set_buckets_for_metric(Matcher::Suffix("_seconds".into()), BUCKETS)
        .map_err(|e| e.to_string())?
        .install_recorder()
        .map_err(|e| e.to_string())
}
