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

pub mod cli;
pub mod config;
pub mod grpc;
pub mod http;
mod status;
pub mod telemetry;

use std::net::SocketAddr;
use std::sync::Arc;

use forkline_engine::Engine;
use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tokio_stream::wrappers::TcpListenerStream;
use tokio_util::sync::CancellationToken;

pub use config::ServerConfig;

/// The server could not start or stopped with an error.
#[derive(Debug, thiserror::Error)]
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
        /// `http` or `grpc`.
        listener: &'static str,
        /// What went wrong.
        message: String,
    },
}

/// A listener that is bound and ready to serve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    /// HTTP listener and its local address.
    Http(SocketAddr),
    /// gRPC listener and its local address.
    Grpc(SocketAddr),
}

/// All listeners bound; call [`BoundServer::run`] to serve.
#[derive(Debug)]
pub struct BoundServer {
    http: Vec<(TcpListener, axum::Router)>,
    grpc: Vec<(TcpListener, grpc::GrpcPlans)>,
    bound: Vec<Bound>,
}

impl BoundServer {
    /// Check the config against the engine's plans and bind every listener.
    pub async fn bind(config: &ServerConfig, engine: Arc<Engine>) -> Result<Self, ServerError> {
        config.validate(&engine).map_err(ServerError::Config)?;
        let mut server = BoundServer { http: Vec::new(), grpc: Vec::new(), bound: Vec::new() };
        for listener in &config.http {
            let tcp = bind(listener.addr).await?;
            server.bound.push(Bound::Http(local_addr(&tcp, listener.addr)?));
            server.http.push((tcp, http::router(engine.clone(), &listener.routes)));
        }
        for listener in &config.grpc {
            let tcp = bind(listener.addr).await?;
            server.bound.push(Bound::Grpc(local_addr(&tcp, listener.addr)?));
            server.grpc.push((tcp, grpc::GrpcPlans::new(engine.clone(), &listener.methods)));
        }
        Ok(server)
    }

    /// The bound listeners, in config order (HTTP first, then gRPC).
    pub fn bound(&self) -> &[Bound] {
        &self.bound
    }

    /// Serve until `shutdown` is cancelled, then stop accepting and let
    /// in-flight requests finish.
    pub async fn run(self, shutdown: CancellationToken) -> Result<(), ServerError> {
        let mut tasks: JoinSet<Result<(), ServerError>> = JoinSet::new();
        for (tcp, router) in self.http {
            let stop = shutdown.clone();
            tasks.spawn(async move {
                axum::serve(tcp, router.into_make_service_with_connect_info::<SocketAddr>())
                    .with_graceful_shutdown(stop.cancelled_owned())
                    .await
                    .map_err(|e| ServerError::Serve { listener: "http", message: e.to_string() })
            });
        }
        for (tcp, plans) in self.grpc {
            let stop = shutdown.clone();
            tasks.spawn(async move {
                tonic::transport::Server::builder()
                    .add_service(plans)
                    .serve_with_incoming_shutdown(TcpListenerStream::new(tcp), stop.cancelled_owned())
                    .await
                    .map_err(|e| ServerError::Serve { listener: "grpc", message: e.to_string() })
            });
        }
        let mut result = Ok(());
        while let Some(joined) = tasks.join_next().await {
            let outcome =
                joined.unwrap_or_else(|e| Err(ServerError::Serve { listener: "task", message: e.to_string() }));
            if let Err(e) = outcome {
                tracing::error!(error = %e, "listener stopped");
                shutdown.cancel();
                if result.is_ok() {
                    result = Err(e);
                }
            }
        }
        result
    }
}

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
