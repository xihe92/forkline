//! Any text as `services.toml` or `server.toml`: parsing and building
//! services never panic.
#![no_main]

use std::sync::LazyLock;

use forkline_services::{ServiceKinds, ServicesFile};
use libfuzzer_sys::fuzz_target;

// gRPC channels are built lazily but need a runtime context.
static RUNTIME: LazyLock<tokio::runtime::Runtime> =
    LazyLock::new(|| tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap());

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else { return };
    if let Ok(file) = ServicesFile::from_toml_str(text) {
        let _guard = RUNTIME.enter();
        let _ = ServiceKinds::default().build(&file);
    }
    let _ = forkline_server::ServerConfig::from_toml_str(text);
});
