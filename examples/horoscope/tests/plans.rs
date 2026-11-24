//! The shipped config, plans and services pass every check `serve` runs at
//! startup, so a broken plan fails `cargo test` instead of the server's start
//! or reload. The backends need not be running: clients connect lazily.

#![allow(missing_docs)]

use std::path::Path;

use forkline_server::cli::App;

#[test]
fn shipped_plans_pass_every_check() {
    let registry = forkline_horoscope::registry().expect("registry");
    App::new(registry).assert_files_valid(
        Some(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/server.toml"))),
        concat!(env!("CARGO_MANIFEST_DIR"), "/plans.toml"),
        Some(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/services.toml"))),
    );
}
