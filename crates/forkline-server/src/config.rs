//! `server.toml`: listeners, runtime and telemetry.
//!
//! ```toml
//! [runtime]
//! worker_threads = 8          # default: one per core
//!
//! [[http]]
//! addr = "0.0.0.0:8080"
//! routes = { "/echo" = "echo" }   # POST /echo runs plan `echo`
//!
//! [[grpc]]
//! addr = "0.0.0.0:50051"
//! methods = { Echo = "echo" }     # /forkline.v1.Plans/Echo runs plan `echo`
//!
//! [telemetry]
//! log_format = "json"             # or "text"
//! filter = "info"                 # RUST_LOG overrides it
//! ```

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;

use forkline_engine::Engine;
use serde::Deserialize;

/// The whole server config.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// Tokio runtime settings.
    #[serde(default)]
    pub runtime: RuntimeConfig,
    /// HTTP listeners.
    #[serde(default)]
    pub http: Vec<HttpListener>,
    /// gRPC listeners.
    #[serde(default)]
    pub grpc: Vec<GrpcListener>,
    /// Logging.
    #[serde(default)]
    pub telemetry: TelemetryConfig,
}

/// Tokio runtime settings.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    /// Worker threads; default one per core.
    pub worker_threads: Option<usize>,
    /// Upper bound on the blocking pool used by `Ctx::blocking`.
    pub max_blocking_threads: Option<usize>,
}

/// One HTTP listener.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpListener {
    /// Address to bind.
    pub addr: SocketAddr,
    /// Path to plan name. Each path accepts POST.
    pub routes: BTreeMap<String, String>,
}

/// One gRPC listener serving `forkline.v1.Plans`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrpcListener {
    /// Address to bind.
    pub addr: SocketAddr,
    /// Method name to plan name: `/forkline.v1.Plans/<method>`.
    pub methods: BTreeMap<String, String>,
}

/// Log output format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// Human-readable lines.
    #[default]
    Text,
    /// One JSON object per line.
    Json,
}

/// Logging settings.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryConfig {
    /// Output format.
    #[serde(default)]
    pub log_format: LogFormat,
    /// `tracing` filter directive; `RUST_LOG` overrides it.
    #[serde(default = "default_filter")]
    pub filter: String,
}

fn default_filter() -> String {
    "info".into()
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self { log_format: LogFormat::default(), filter: default_filter() }
    }
}

impl ServerConfig {
    /// Parse a config from a string.
    pub fn from_toml_str(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    /// Read and parse a config file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Self::from_toml_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Check that every route and method names a loaded plan.
    pub fn validate(&self, engine: &Engine) -> Result<(), String> {
        let mut problems = Vec::new();
        if self.http.is_empty() && self.grpc.is_empty() {
            problems.push("no listeners configured".to_owned());
        }
        for l in &self.http {
            for (path, plan) in &l.routes {
                if !path.starts_with('/') {
                    problems.push(format!("http {}: route `{path}` must start with `/`", l.addr));
                }
                if !engine.has_plan(plan) {
                    problems.push(format!("http {}: route `{path}` names unknown plan `{plan}`", l.addr));
                }
            }
        }
        for l in &self.grpc {
            for (method, plan) in &l.methods {
                if method.is_empty() || method.contains('/') {
                    problems.push(format!("grpc {}: `{method}` is not a valid method name", l.addr));
                }
                if !engine.has_plan(plan) {
                    problems.push(format!("grpc {}: method `{method}` names unknown plan `{plan}`", l.addr));
                }
            }
        }
        if problems.is_empty() { Ok(()) } else { Err(problems.join("; ")) }
    }
}
