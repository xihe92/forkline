//! The command line every Forkline server binary shares.
//!
//! * `serve --config server.toml --plans plans.toml [--services services.toml]`
//! * `check --plans plans.toml [--services services.toml]`: every plan check,
//!   with this binary's modules and services
//!
//! ```ignore
//! fn main() -> std::process::ExitCode {
//!     let registry = Registry::builder() /* ... */ .build().expect("registry");
//!     forkline_server::cli::App::new(registry)
//!         .service_kinds(ServiceKinds::default().register("redis", build_redis))
//!         .run()
//! }
//! ```

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use forkline_core::{Registry, ServiceRegistry};
use forkline_engine::Engine;
use forkline_plan::PlansFile;
use forkline_services::{ServiceKinds, ServicesFile};
use tokio_util::sync::CancellationToken;

use crate::{Bound, BoundServer, ServerConfig, cancel_on_signal, telemetry};

#[derive(Parser)]
#[command(version, about = "A Forkline plan server")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the configured listeners.
    Serve {
        /// Server config (listeners, runtime, telemetry).
        #[arg(long, default_value = "server.toml")]
        config: PathBuf,
        /// Plans file.
        #[arg(long, default_value = "plans.toml")]
        plans: PathBuf,
        /// Services file.
        #[arg(long)]
        services: Option<PathBuf>,
    },
    /// Validate a plans file against this binary's modules and services.
    Check {
        /// Plans file.
        #[arg(long, default_value = "plans.toml")]
        plans: PathBuf,
        /// Services file.
        #[arg(long)]
        services: Option<PathBuf>,
    },
}

/// Parse the command line and run it with `registry` and the built-in service kinds.
pub fn main_with(registry: Registry) -> ExitCode {
    App::new(registry).run()
}

/// A server binary: modules, service kinds and services added in code.
#[derive(Debug)]
pub struct App {
    registry: Arc<Registry>,
    kinds: ServiceKinds,
    services: ServiceRegistry,
}

impl App {
    /// An app with these modules, the built-in service kinds and no services.
    pub fn new(registry: Registry) -> Self {
        Self { registry: Arc::new(registry), kinds: ServiceKinds::default(), services: ServiceRegistry::new() }
    }

    /// Replace the service kinds `services.toml` may use.
    pub fn service_kinds(mut self, kinds: ServiceKinds) -> Self {
        self.kinds = kinds;
        self
    }

    /// Add a service built in code; `services.toml` entries of the same name replace it.
    pub fn service<T: Clone + Send + Sync + 'static>(mut self, name: impl Into<String>, value: T) -> Self {
        self.services.insert(name, value);
        self
    }

    /// Load the plans (and services) into an engine.
    pub fn load_engine(&self, plans: &Path, services: Option<&Path>) -> Result<Engine, String> {
        let file = PlansFile::load(plans).map_err(|e| e.to_string())?;
        let services = self.load_services(services)?;
        Engine::with_services(self.registry.clone(), &file, Arc::new(services)).map_err(|d| d.to_string())
    }

    /// Build the service registry: code-registered services plus the file's.
    pub fn load_services(&self, services: Option<&Path>) -> Result<ServiceRegistry, String> {
        let mut registry = self.services.clone();
        if let Some(path) = services {
            let file = ServicesFile::load(path)?;
            self.kinds.build_into(&file, &mut registry).map_err(|v| v.join("\n"))?;
        }
        Ok(registry)
    }

    /// Parse the command line and run it.
    pub fn run(self) -> ExitCode {
        match Cli::parse().command {
            Command::Check { plans, services } => self.check(&plans, services.as_deref()),
            Command::Serve { config, plans, services } => self.serve(&config, &plans, services.as_deref()),
        }
    }

    fn check(&self, plans: &Path, services: Option<&Path>) -> ExitCode {
        // Some clients (gRPC channels) need a runtime to be built.
        let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("error: cannot start the runtime: {e}");
                return ExitCode::FAILURE;
            }
        };
        match runtime.block_on(async { self.load_engine(plans, services) }) {
            Ok(engine) => {
                let snapshot = engine.snapshot();
                println!(
                    "ok: {} ({} plans, {} services)",
                    plans.display(),
                    snapshot.plans.len(),
                    snapshot.services.len()
                );
                ExitCode::SUCCESS
            }
            Err(e) => {
                for line in e.lines() {
                    eprintln!("error: {line}");
                }
                ExitCode::FAILURE
            }
        }
    }

    fn serve(&self, config: &Path, plans: &Path, services: Option<&Path>) -> ExitCode {
        let config = match ServerConfig::load(config) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        };
        let mut builder = tokio::runtime::Builder::new_multi_thread();
        builder.enable_all();
        if let Some(n) = config.runtime.worker_threads {
            builder.worker_threads(n);
        }
        if let Some(n) = config.runtime.max_blocking_threads {
            builder.max_blocking_threads(n);
        }
        let runtime = match builder.build() {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("error: cannot start the runtime: {e}");
                return ExitCode::FAILURE;
            }
        };
        runtime.block_on(async move {
            telemetry::init(&config.telemetry);
            let engine = match self.load_engine(plans, services) {
                Ok(e) => Arc::new(e),
                Err(e) => {
                    for line in e.lines() {
                        tracing::error!("{line}");
                    }
                    return ExitCode::FAILURE;
                }
            };
            let server = match BoundServer::bind(&config, engine).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(error = %e, "cannot start");
                    return ExitCode::FAILURE;
                }
            };
            for b in server.bound() {
                match b {
                    Bound::Http(addr) => tracing::info!(%addr, "http listening"),
                    Bound::Grpc(addr) => tracing::info!(%addr, "grpc listening"),
                }
            }
            let shutdown = CancellationToken::new();
            tokio::spawn(cancel_on_signal(shutdown.clone()));
            match server.run(shutdown).await {
                Ok(()) => {
                    tracing::info!("stopped");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    tracing::error!(error = %e, "server failed");
                    ExitCode::FAILURE
                }
            }
        })
    }
}
