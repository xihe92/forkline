//! Any text as a plans file: parsing, graph checks and full compilation
//! (module kinds, key types, renames, configs, data flow) never panic, and
//! whatever compiles also loads into an engine.
#![no_main]

use std::sync::{Arc, LazyLock};

use forkline_core::Registry;
use forkline_plan::{PlansFile, check_structure, compile_all};
use libfuzzer_sys::fuzz_target;

static REGISTRY: LazyLock<Arc<Registry>> = LazyLock::new(|| Arc::new(forkline_fuzz::registry()));

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else { return };
    let Ok(file) = PlansFile::from_toml_str(text) else { return };
    for (name, plan) in &file.plans {
        let _ = check_structure(name, plan);
    }
    if compile_all(&file, &REGISTRY).is_ok() {
        let engine = forkline_engine::Engine::new(REGISTRY.clone(), &file);
        assert!(engine.is_ok(), "compiled but did not load: {:?}", engine.err());
    }
});
