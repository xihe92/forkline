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
