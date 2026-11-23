//! `server.toml`: listeners, runtime and telemetry.
//!
//! ```toml
//! [runtime]
//! worker_threads = 8          # default: one per core
//!
//! [[http]]
//! addr = "0.0.0.0:8080"
//! routes = { "/echo" = "echo" }   # GET or POST /echo runs plan `echo`
//! max_body_bytes = 2097152        # larger bodies get 413 (default 2 MiB)
//!
//! [[grpc]]
//! addr = "0.0.0.0:50051"
//! methods = { Echo = "echo" }     # /forkline.v1.Plans/Echo runs plan `echo`
//!
//! [telemetry]
//! log_format = "json"             # or "text"
//! filter = "info"                 # RUST_LOG overrides it
//!
//! [[tcp]]
//! addr = "0.0.0.0:7000"
//! plan = "echo"                   # length-delimited frames, one request each
//!
//! [admin]
//! addr = "127.0.0.1:6789"         # /healthz /readyz /metrics /admin/{snapshot,reload,rollback}
//! allow = ["127.0.0.1/32", "::1/128"]
//!
//! [admission]
//! red = 800                       # refuse new requests at 800 in flight...
//! green = 500                     # ...until back down to 500
//! max_queue_ms = 50               # shed requests that waited longer to start
//!
//! [reload]
//! history = 5                     # snapshots kept for rollback
//! watch_interval_ms = 0           # >0: poll config files and reload on change
//!
//! [shutdown]
//! drain_timeout_ms = 10000        # in-flight requests, then detached paths
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
    /// Shutdown behaviour.
    #[serde(default)]
    pub shutdown: ShutdownConfig,
    /// Length-delimited TCP listeners.
    #[serde(default)]
    pub tcp: Vec<TcpListenerConfig>,
    /// Admin listener (health, metrics, reload, rollback).
    pub admin: Option<AdminConfig>,
    /// Admission control shared by all listeners.
    pub admission: Option<AdmissionConfig>,
    /// Reload behaviour.
    #[serde(default)]
    pub reload: ReloadConfig,
}

/// One length-delimited TCP listener: every frame is a request to `plan`.
///
/// Frames carry a 4-byte big-endian length prefix. A response frame is one
/// status byte followed by the reply body: 0 ok, 1 bad request, 2 failed,
/// 3 deadline, 4 overloaded (refused or shed).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TcpListenerConfig {
    /// Address to bind.
    pub addr: SocketAddr,
    /// Plan every frame runs.
    pub plan: String,
    /// Largest accepted frame; a larger one closes the connection.
    #[serde(default = "default_max_frame")]
    pub max_frame_bytes: usize,
    /// Requests one connection may have in flight (responses keep request order).
    #[serde(default = "default_pipeline")]
    pub max_in_flight_per_connection: usize,
}

fn default_max_frame() -> usize {
    1 << 20
}

fn default_pipeline() -> usize {
    32
}

/// The admin listener.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminConfig {
    /// Address to bind; keep it off the public interface.
    pub addr: SocketAddr,
    /// Networks allowed to call it (default: loopback only).
    #[serde(default = "default_allow")]
    pub allow: Vec<String>,
}

fn default_allow() -> Vec<String> {
    vec!["127.0.0.1/32".into(), "::1/128".into()]
}

/// Admission control: refuse new work above `red` in-flight requests until
/// the count falls to `green`.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmissionConfig {
    /// In-flight requests at which new requests are refused.
    pub red: usize,
    /// In-flight requests at which admission resumes.
    pub green: usize,
    /// Shed requests that waited longer than this before starting, in milliseconds.
    pub max_queue_ms: Option<u64>,
}

/// Reload behaviour.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReloadConfig {
    /// Previous snapshots kept for rollback.
    #[serde(default = "default_history")]
    pub history: usize,
    /// Poll the plans and services files this often and reload on change (0: off).
    #[serde(default)]
    pub watch_interval_ms: u64,
}

fn default_history() -> usize {
    5
}

impl Default for ReloadConfig {
    fn default() -> Self {
        Self { history: default_history(), watch_interval_ms: 0 }
    }
}

/// Shutdown behaviour.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShutdownConfig {
    /// How long to wait for in-flight requests and then detached paths after
    /// shutdown starts; whatever is left is cancelled.
    #[serde(default = "default_drain_timeout_ms")]
    pub drain_timeout_ms: u64,
}

fn default_drain_timeout_ms() -> u64 {
    10_000
}

impl Default for ShutdownConfig {
    fn default() -> Self {
        Self { drain_timeout_ms: default_drain_timeout_ms() }
    }
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
    /// Path to plan name. Each path accepts GET and POST.
    pub routes: BTreeMap<String, String>,
    /// Largest accepted request body; a larger one gets 413 (default 2 MiB).
    #[serde(default = "default_max_body")]
    pub max_body_bytes: usize,
}

/// Default request body limit for HTTP listeners: 2 MiB.
pub const DEFAULT_MAX_BODY_BYTES: usize = 2 << 20;

fn default_max_body() -> usize {
    DEFAULT_MAX_BODY_BYTES
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
    /// OTLP/gRPC collector endpoint for traces (needs the `otlp` feature),
    /// e.g. `http://otel-collector:4317`.
    #[serde(default)]
    pub otlp_endpoint: Option<String>,
    /// Service name reported with traces.
    #[serde(default = "default_service_name")]
    pub service_name: String,
}

fn default_service_name() -> String {
    "forkline".into()
}

fn default_filter() -> String {
    "info".into()
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            log_format: LogFormat::default(),
            filter: default_filter(),
            otlp_endpoint: None,
            service_name: default_service_name(),
        }
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

    /// Every plan a route, method or TCP listener serves.
    pub fn served_plans(&self) -> std::collections::BTreeSet<String> {
        let http = self.http.iter().flat_map(|l| l.routes.values());
        let grpc = self.grpc.iter().flat_map(|l| l.methods.values());
        let tcp = self.tcp.iter().map(|l| &l.plan);
        http.chain(grpc).chain(tcp).cloned().collect()
    }

    /// Check that every route and method names a loaded plan.
    pub fn validate(&self, engine: &Engine) -> Result<(), String> {
        let mut problems = Vec::new();
        if self.http.is_empty() && self.grpc.is_empty() && self.tcp.is_empty() {
            problems.push("no listeners configured".to_owned());
        }
        for l in &self.tcp {
            if !engine.has_plan(&l.plan) {
                problems.push(format!("tcp {}: unknown plan `{}`", l.addr, l.plan));
            }
            if l.max_frame_bytes == 0 || l.max_in_flight_per_connection == 0 {
                problems.push(format!("tcp {}: max_frame_bytes and max_in_flight_per_connection must be > 0", l.addr));
            }
        }
        if let Some(a) = self.admission.as_ref().filter(|a| a.red == 0 || a.green >= a.red) {
            problems.push(format!("admission: need 0 < green < red (green = {}, red = {})", a.green, a.red));
        }
        if let Some(admin) = &self.admin {
            for net in &admin.allow {
                if net.parse::<crate::allow::Network>().is_err() {
                    problems.push(format!("admin: `{net}` is not an IP address or network"));
                }
            }
        }
        for l in &self.http {
            if l.max_body_bytes == 0 {
                problems.push(format!("http {}: max_body_bytes must be > 0", l.addr));
            }
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
