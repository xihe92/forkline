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
use forkline_engine::{Engine, LoadInput};
use forkline_plan::PlansFile;
use forkline_services::{ServiceKinds, ServicesFile};
use tokio_util::sync::CancellationToken;

use crate::{Bound, BoundServer, Hooks, Reloader, ServerConfig, cancel_on_signal, telemetry};

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

    /// Hash of the config files, for spans and the admin API.
    pub fn config_hash(plans: &Path, services: Option<&Path>) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for path in std::iter::once(plans).chain(services) {
            std::fs::read(path).unwrap_or_default().hash(&mut h);
        }
        h.finish()
    }

    /// Re-read the plans and services files and swap a new snapshot into `engine`.
    pub fn reload(&self, engine: &Engine, plans: &Path, services: Option<&Path>) -> Result<u64, String> {
        let file = PlansFile::load(plans).map_err(|e| e.to_string())?;
        let registry = self.load_services(services)?;
        let config_hash = Self::config_hash(plans, services);
        engine.load(LoadInput { plans: &file, services: Arc::new(registry), config_hash }).map_err(|d| d.to_string())
    }

    /// Parse the command line and run it.
    pub fn run(self) -> ExitCode {
        match Cli::parse().command {
            Command::Check { plans, services } => self.check(&plans, services.as_deref()),
            Command::Serve { config, plans, services } => Arc::new(self).serve(&config, &plans, services),
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

    fn serve(self: Arc<Self>, config: &Path, plans: &Path, services: Option<PathBuf>) -> ExitCode {
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
        let plans = plans.to_path_buf();
        runtime.block_on(async move {
            telemetry::init(&config.telemetry);
            // The recorder must exist before the engine so plan metric handles bind to it.
            let metrics = match telemetry::install_metrics() {
                Ok(handle) => Some(handle),
                Err(e) => {
                    tracing::warn!(error = %e, "metrics disabled");
                    None
                }
            };
            let engine = match self.build_engine(&config, &plans, services.as_deref()) {
                Ok(e) => Arc::new(e),
                Err(e) => {
                    for line in e.lines() {
                        tracing::error!("{line}");
                    }
                    return ExitCode::FAILURE;
                }
            };
            let reloader: Reloader = {
                let (app, engine, plans, services) = (self.clone(), engine.clone(), plans.clone(), services.clone());
                Arc::new(move || app.reload(&engine, &plans, services.as_deref()))
            };
            let hooks = Hooks { reloader: Some(reloader.clone()), metrics };
            let server = match BoundServer::bind_with(&config, engine, hooks).await {
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
                    Bound::Tcp(addr) => tracing::info!(%addr, "tcp listening"),
                    Bound::Admin(addr) => tracing::info!(%addr, "admin listening"),
                }
            }
            let shutdown = CancellationToken::new();
            tokio::spawn(cancel_on_signal(shutdown.clone()));
            tokio::spawn(reload_on_sighup(reloader.clone(), shutdown.clone()));
            if config.reload.watch_interval_ms > 0 {
                let every = std::time::Duration::from_millis(config.reload.watch_interval_ms);
                tokio::spawn(watch_files(reloader, plans.clone(), services.clone(), every, shutdown.clone()));
            }
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

    fn build_engine(&self, config: &ServerConfig, plans: &Path, services: Option<&Path>) -> Result<Engine, String> {
        let file = PlansFile::load(plans).map_err(|e| e.to_string())?;
        let registry = self.load_services(services)?;
        let max_queue = config.admission.and_then(|a| a.max_queue_ms).map(std::time::Duration::from_millis);
        Engine::builder(self.registry.clone())
            .history(config.reload.history)
            .max_queue(max_queue)
            .build(LoadInput {
                plans: &file,
                services: Arc::new(registry),
                config_hash: Self::config_hash(plans, services),
            })
            .map_err(|d| d.to_string())
    }
}

async fn run_reload(reloader: Reloader, trigger: &'static str) {
    match tokio::task::spawn_blocking(move || reloader()).await {
        Ok(Ok(version)) => tracing::info!(version, trigger, "reloaded"),
        Ok(Err(e)) => {
            for line in e.lines() {
                tracing::error!(trigger, "reload rejected: {line}");
            }
        }
        Err(e) => tracing::error!(trigger, error = %e, "reload task failed"),
    }
}

/// Reload on SIGHUP until shutdown.
async fn reload_on_sighup(reloader: Reloader, shutdown: CancellationToken) {
    #[cfg(unix)]
    {
        let mut hup = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "cannot listen for SIGHUP");
                return;
            }
        };
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                got = hup.recv() => {
                    if got.is_none() {
                        return;
                    }
                    run_reload(reloader.clone(), "sighup").await;
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (reloader, shutdown);
    }
}

/// Poll the config files and reload when their contents change.
async fn watch_files(
    reloader: Reloader,
    plans: PathBuf,
    services: Option<PathBuf>,
    every: std::time::Duration,
    shutdown: CancellationToken,
) {
    let mut last = App::config_hash(&plans, services.as_deref());
    let mut ticker = tokio::time::interval(every);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            _ = ticker.tick() => {
                let now = App::config_hash(&plans, services.as_deref());
                if now != last {
                    last = now;
                    run_reload(reloader.clone(), "file change").await;
                }
            }
        }
    }
}
