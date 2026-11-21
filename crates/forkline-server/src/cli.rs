//! The command line every Forkline server binary shares.
//!
//! * `serve --config server.toml --plans plans.toml`
//! * `check --plans plans.toml`: every plan check, with this binary's modules

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use forkline_core::Registry;
use forkline_engine::Engine;
use forkline_plan::PlansFile;
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
    },
    /// Validate a plans file against this binary's modules.
    Check {
        /// Plans file.
        #[arg(long, default_value = "plans.toml")]
        plans: PathBuf,
    },
}

/// Parse the command line and run it with `registry`.
pub fn main_with(registry: Registry) -> ExitCode {
    let registry = Arc::new(registry);
    match Cli::parse().command {
        Command::Check { plans } => check(registry, &plans),
        Command::Serve { config, plans } => serve(registry, &config, &plans),
    }
}

fn load_engine(registry: Arc<Registry>, plans: &PathBuf) -> Result<Engine, String> {
    let file = PlansFile::load(plans).map_err(|e| e.to_string())?;
    Engine::new(registry, &file).map_err(|d| d.to_string())
}

fn check(registry: Arc<Registry>, plans: &PathBuf) -> ExitCode {
    match load_engine(registry, plans) {
        Ok(engine) => {
            println!("ok: {} ({} plans)", plans.display(), engine.snapshot().plans.len());
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

fn serve(registry: Arc<Registry>, config: &PathBuf, plans: &PathBuf) -> ExitCode {
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
        let engine = match load_engine(registry, plans) {
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
