//! `forkline-plan check FILE...`: check plan graphs in CI.
//!
//! This binary contains no modules, so it checks the graph rules only. A
//! server binary built with its modules can run the full check, including
//! module kinds, key types and module config (`<server> check`).

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use forkline_plan::{PlansFile, check_structure};

#[derive(Parser)]
#[command(name = "forkline-plan", version, about = "Forkline plan tools")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check the graph rules of one or more plans files.
    Check {
        /// Plans files to check.
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
}

fn main() -> ExitCode {
    let Cli { command: Command::Check { files } } = Cli::parse();
    let mut failed = false;
    for path in &files {
        let file = match PlansFile::load(path) {
            Ok(file) => file,
            Err(e) => {
                eprintln!("error: {e}");
                failed = true;
                continue;
            }
        };
        let mut problems = 0;
        for (name, plan) in &file.plans {
            for d in check_structure(name, plan).1 {
                eprintln!("error: {}: {d}", path.display());
                problems += 1;
            }
        }
        if problems == 0 {
            println!("ok: {} ({} plans)", path.display(), file.plans.len());
        } else {
            failed = true;
        }
    }
    if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}
