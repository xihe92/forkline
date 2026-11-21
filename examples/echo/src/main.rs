//! `forkline-echo serve --config examples/echo/server.toml --plans examples/echo/plans.toml`

#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    match forkline_echo::registry() {
        Ok(registry) => forkline_server::cli::main_with(registry),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
