//! Any byte stream into the length-delimited TCP adapter: it never panics,
//! always closes, and every response it writes is a well-formed frame with a
//! known status byte.
#![no_main]

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use forkline_engine::Engine;
use forkline_plan::PlansFile;
use libfuzzer_sys::fuzz_target;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

static RUNTIME: LazyLock<tokio::runtime::Runtime> =
    LazyLock::new(|| tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap());

static ENGINE: LazyLock<Arc<Engine>> = LazyLock::new(|| {
    let plans = PlansFile::from_toml_str(
        r#"
        [plans.p]
        root = "decode"
        terminal = "error"
        time_budget_ms = 1000
        links = ["decode -> m", "m -> encode"]
        [plans.p.nodes]
        decode = { module = "Dec" }
        m      = { module = "M1", config = { factor = 3 } }
        encode = { module = "Enc" }
        error  = { module = "Term" }
        "#,
    )
    .unwrap();
    Arc::new(Engine::new(Arc::new(forkline_fuzz::registry()), &plans).unwrap())
});

fuzz_target!(|data: &[u8]| {
    RUNTIME.block_on(async {
        let (mut client, server) = tokio::io::duplex(1 << 16);
        let task = tokio::spawn(forkline_server::tcp::serve_stream(server, ENGINE.clone(), "p", 4096, 8));
        let _ = client.write_all(data).await;
        let _ = client.shutdown().await;
        let mut out = Vec::new();
        let read = tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut out)).await;
        assert!(read.is_ok(), "the connection did not close");
        task.await.unwrap();
        // Every response is a complete frame starting with a known status byte.
        let mut rest = &out[..];
        while !rest.is_empty() {
            assert!(rest.len() >= 5, "truncated frame");
            let len = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
            assert!(rest.len() >= 4 + len && len >= 1, "bad frame length");
            assert!(rest[4] <= 4, "unknown status byte {}", rest[4]);
            rest = &rest[4 + len..];
        }
    });
});
