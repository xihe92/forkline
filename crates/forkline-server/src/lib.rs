//! The thin server around the Forkline engine.
//!
//! Everything here is glue over existing crates: axum for HTTP, tonic for
//! gRPC, tracing-subscriber for logs, clap for the CLI. A binary registers its
//! modules and hands the registry to [`cli::main_with`]:
//!
//! ```ignore
//! fn main() -> std::process::ExitCode {
//!     let registry = Registry::builder() /* .decode::<...>(...) */ .build().expect("registry");
//!     forkline_server::cli::main_with(registry)
//! }
//! ```

#![forbid(unsafe_code)]

#[cfg(feature = "jemalloc")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

pub mod admin;
pub mod admission;
mod allow;
pub mod cli;
pub mod config;
pub mod grpc;
pub mod http;
mod http_serve;
mod status;
pub mod tcp;
pub mod telemetry;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use forkline_engine::Engine;
use prometheus_client::registry::Registry as MetricsRegistry;
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub use admin::Reloader;
pub use config::ServerConfig;

use crate::admission::{Admission, Gate};
use crate::config::TcpListenerConfig;

/// The server could not start or stopped with an error.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ServerError {
    /// Server configuration is invalid.
    #[error("server config: {0}")]
    Config(String),
    /// A listener could not bind.
    #[error("cannot bind {addr}: {source}")]
    Bind {
        /// Address.
        addr: SocketAddr,
        /// Underlying error.
        source: std::io::Error,
    },
    /// A listener failed while serving.
    #[error("{listener} listener failed: {message}")]
    Serve {
        /// `http`, `grpc`, `tcp` or `admin`.
        listener: &'static str,
        /// What went wrong.
        message: String,
    },
}

/// A listener that is bound and ready to serve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Bound {
    /// HTTP listener and its local address.
    Http(SocketAddr),
    /// gRPC listener and its local address.
    Grpc(SocketAddr),
    /// Length-delimited TCP listener and its local address.
    Tcp(SocketAddr),
    /// Admin listener and its local address.
    Admin(SocketAddr),
}

/// Optional pieces a server binary supplies.
#[derive(Clone, Default)]
pub struct Hooks {
    /// Called by `POST /admin/reload` (and the CLI's SIGHUP and file watcher).
    pub reloader: Option<Reloader>,
    /// Rendered at `GET /metrics` (see [`telemetry::metrics_registry`]).
    pub metrics: Option<Arc<MetricsRegistry>>,
}

impl std::fmt::Debug for Hooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hooks")
            .field("reloader", &self.reloader.is_some())
            .field("metrics", &self.metrics.is_some())
            .finish()
    }
}

/// All listeners bound; call [`BoundServer::run`] to serve.
#[derive(Debug)]
pub struct BoundServer {
    http: Vec<(TcpListener, axum::Router, http_serve::Timeouts)>,
    grpc: Vec<(TcpListener, grpc::GrpcPlans, http_serve::Timeouts)>,
    tcp: Vec<(TcpListener, TcpListenerConfig, Gate)>,
    admin: Option<(TcpListener, axum::Router)>,
    bound: Vec<Bound>,
    engine: Arc<Engine>,
    admission: Option<Arc<Admission>>,
    ready: Arc<AtomicBool>,
    hard_stop: CancellationToken,
    drain_timeout: Duration,
}

impl BoundServer {
    /// Check the config against the engine's plans and bind every listener.
    pub async fn bind(config: &ServerConfig, engine: Arc<Engine>) -> Result<Self, ServerError> {
        Self::bind_with(config, engine, Hooks::default()).await
    }

    /// Like [`bind`](Self::bind), with a reloader and a metrics handle for the admin listener.
    pub async fn bind_with(config: &ServerConfig, engine: Arc<Engine>, hooks: Hooks) -> Result<Self, ServerError> {
        config.validate(&engine).map_err(ServerError::Config)?;
        // From here on, a reload that drops a plan a listener serves is rejected.
        engine.require_plans(config.served_plans());
        let admission = config.admission.as_ref().map(|a| Arc::new(Admission::new(a)));
        if let Some(a) = &config.admission {
            engine.set_max_queue(a.max_queue_ms.map(Duration::from_millis));
        }
        let ready = Arc::new(AtomicBool::new(true));
        let hard_stop = CancellationToken::new();
        let mut server = BoundServer {
            http: Vec::new(),
            grpc: Vec::new(),
            tcp: Vec::new(),
            admin: None,
            bound: Vec::new(),
            engine: engine.clone(),
            admission: admission.clone(),
            ready: ready.clone(),
            hard_stop: hard_stop.clone(),
            drain_timeout: Duration::from_millis(config.shutdown.drain_timeout_ms),
        };
        for listener in &config.http {
            let tcp = bind(listener.addr).await?;
            let addr = local_addr(&tcp, listener.addr)?;
            server.bound.push(Bound::Http(addr));
            let gate = Gate::new(admission.clone(), format!("http:{addr}"), hard_stop.clone());
            let router = http::gated_router(engine.clone(), &listener.routes, listener.max_body_bytes, gate);
            server.http.push((tcp, router, listener.timeouts()));
        }
        for listener in &config.grpc {
            let tcp = bind(listener.addr).await?;
            let addr = local_addr(&tcp, listener.addr)?;
            server.bound.push(Bound::Grpc(addr));
            let gate = Gate::new(admission.clone(), format!("grpc:{addr}"), hard_stop.clone());
            server.grpc.push((
                tcp,
                grpc::GrpcPlans::gated(engine.clone(), &listener.methods, gate),
                listener.timeouts(),
            ));
        }
        for listener in &config.tcp {
            let tcp = bind(listener.addr).await?;
            let addr = local_addr(&tcp, listener.addr)?;
            server.bound.push(Bound::Tcp(addr));
            let gate = Gate::new(admission.clone(), format!("tcp:{addr}"), hard_stop.clone());
            server.tcp.push((tcp, listener.clone(), gate));
        }
        if let Some(admin) = &config.admin {
            let tcp = bind(admin.addr).await?;
            server.bound.push(Bound::Admin(local_addr(&tcp, admin.addr)?));
            let state = admin::AdminState {
                engine,
                reloader: hooks.reloader,
                metrics: hooks.metrics,
                ready,
                allow: Arc::new(admin::parse_allow(&admin.allow)),
            };
            server.admin = Some((tcp, admin::router(state)));
        }
        Ok(server)
    }

    /// The bound listeners, in config order (HTTP, gRPC, TCP, then admin).
    pub fn bound(&self) -> &[Bound] {
        &self.bound
    }

    /// The admission controller, if configured.
    pub fn admission(&self) -> Option<&Arc<Admission>> {
        self.admission.as_ref()
    }

    /// Serve until `shutdown` is cancelled. Then: readiness turns false,
    /// listeners stop accepting, in-flight requests get up to the drain
    /// timeout to finish (the rest are cancelled), detached paths get what is
    /// left of it, and the admin listener stops last.
    pub async fn run(self, shutdown: CancellationToken) -> Result<(), ServerError> {
        let mut listeners: JoinSet<Result<(), ServerError>> = JoinSet::new();
        for (tcp, router, timeouts) in self.http {
            let stop = shutdown.clone();
            listeners.spawn(async move {
                http_serve::serve(tcp, router, http_serve::Versions::Both, timeouts, stop).await;
                Ok(())
            });
        }
        for (tcp, plans, timeouts) in self.grpc {
            let stop = shutdown.clone();
            listeners.spawn(async move {
                http_serve::serve(tcp, plans, http_serve::Versions::Http2, timeouts, stop).await;
                Ok(())
            });
        }
        for (tcp, config, gate) in self.tcp {
            let (engine, stop) = (self.engine.clone(), shutdown.clone());
            listeners.spawn(async move {
                tcp::serve(tcp, engine, gate, config, stop)
                    .await
                    .map_err(|e| ServerError::Serve { listener: "tcp", message: e.to_string() })
            });
        }
        let admin_stop = CancellationToken::new();
        let admin_task = self.admin.map(|(tcp, router)| {
            let stop = admin_stop.clone();
            tokio::spawn(http_serve::serve(tcp, router, http_serve::Versions::Both, ADMIN_TIMEOUTS, stop))
        });

        // Serve until asked to stop, or until a listener fails.
        let mut result = Ok(());
        tokio::select! {
            () = shutdown.cancelled() => {}
            Some(joined) = listeners.join_next() => {
                let outcome = joined.unwrap_or_else(|e| Err(ServerError::Serve { listener: "task", message: e.to_string() }));
                if let Err(e) = outcome {
                    tracing::error!(error = %e, "listener stopped");
                    result = Err(e);
                }
                shutdown.cancel();
            }
        }

        // Drain.
        self.ready.store(false, Ordering::Release);
        let started = Instant::now();
        let drained = tokio::time::timeout(self.drain_timeout, async {
            while let Some(joined) = listeners.join_next().await {
                if let Ok(Err(e)) = joined {
                    tracing::error!(error = %e, "listener stopped with an error while draining");
                }
            }
        })
        .await;
        if drained.is_err() {
            // Cut off the requests still running (they answer "shutting down"),
            // give the listeners a moment to close their connections, then abort.
            tracing::warn!("drain timeout; cancelling the requests still running");
            self.hard_stop.cancel();
            let closed =
                tokio::time::timeout(Duration::from_secs(1), async { while listeners.join_next().await.is_some() {} })
                    .await;
            if closed.is_err() {
                listeners.abort_all();
                while listeners.join_next().await.is_some() {}
            }
        }
        let left = self.drain_timeout.saturating_sub(started.elapsed());
        let pending = self.engine.detached_running();
        if pending > 0 {
            tracing::info!(requests = pending, "waiting for detached paths");
            if tokio::time::timeout(left, self.engine.wait_detached()).await.is_err() {
                tracing::warn!(requests = self.engine.detached_running(), "drain timeout; detached paths abandoned");
            }
        }
        admin_stop.cancel();
        if let Some(task) = admin_task {
            let _ = task.await;
        }
        result
    }
}

/// The admin listener's timeouts: the HTTP listeners' defaults.
const ADMIN_TIMEOUTS: http_serve::Timeouts = http_serve::Timeouts {
    header_read: Duration::from_millis(config::DEFAULT_HTTP_TIMEOUT_MS),
    idle: Duration::from_millis(config::DEFAULT_HTTP_TIMEOUT_MS),
};

async fn bind(addr: SocketAddr) -> Result<TcpListener, ServerError> {
    TcpListener::bind(addr).await.map_err(|source| ServerError::Bind { addr, source })
}

fn local_addr(tcp: &TcpListener, addr: SocketAddr) -> Result<SocketAddr, ServerError> {
    tcp.local_addr().map_err(|source| ServerError::Bind { addr, source })
}

/// Cancel `token` on Ctrl-C or SIGTERM.
pub async fn cancel_on_signal(token: CancellationToken) {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(term) => term,
            Err(e) => {
                tracing::warn!(error = %e, "cannot listen for SIGTERM; Ctrl-C only");
                let _ = ctrl_c.await;
                token.cancel();
                return;
            }
        };
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = ctrl_c.await;
    }
    tracing::info!("shutdown requested; draining");
    token.cancel();
}
