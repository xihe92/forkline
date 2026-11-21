//! ```sh
//! cargo run -p forkline-horoscope --bin horoscope-backend &
//! cargo run -p forkline-horoscope -- serve --config examples/horoscope/server.toml \
//!     --plans examples/horoscope/plans.toml --services examples/horoscope/services.toml
//! curl -d '{"want":"horoscope","sign":"leo"}' http://127.0.0.1:8080/query
//! ```

#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    match forkline_horoscope::registry() {
        Ok(registry) => forkline_server::cli::main_with(registry),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
