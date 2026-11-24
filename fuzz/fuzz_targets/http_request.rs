//! Any bytes into an HTTP listener connection (HTTP/1.1, or h2c when the
//! input starts with the HTTP/2 preface), over an in-memory stream: the server
//! never panics, always closes the connection, answers with well-formed
//! HTTP/1 status lines or HTTP/2 frames, and leaves no task behind.
#![no_main]

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use forkline_engine::Engine;
use forkline_fuzz::{H2_PREFACE, assert_h2_frames, assert_no_task_left};
use libfuzzer_sys::fuzz_target;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

static RUNTIME: LazyLock<tokio::runtime::Runtime> =
    LazyLock::new(|| tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap());
static ENGINE: LazyLock<Arc<Engine>> = LazyLock::new(forkline_fuzz::engine);
static ROUTES: LazyLock<BTreeMap<String, String>> = LazyLock::new(|| BTreeMap::from([("/p".into(), "p".into())]));

fuzz_target!(|data: &[u8]| {
    RUNTIME.block_on(async {
        let (mut client, server) = tokio::io::duplex(1 << 20);
        let task = tokio::spawn(forkline_server::http::serve_stream(server, ENGINE.clone(), &ROUTES));
        let _ = client.write_all(data).await;
        let _ = client.shutdown().await;
        let mut out = Vec::new();
        let read = tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut out)).await;
        assert!(read.is_ok(), "the connection did not close");
        drop(client);
        task.await.unwrap();
        if data.starts_with(H2_PREFACE) {
            assert_h2_frames(&out);
        } else if !out.is_empty() {
            // HTTP/1.0 requests are answered in HTTP/1.0.
            assert!(
                out.starts_with(b"HTTP/1.1 ") || out.starts_with(b"HTTP/1.0 "),
                "not an HTTP/1 response: {:?}",
                String::from_utf8_lossy(&out)
            );
            let code = &out[9..12.min(out.len())];
            assert!(code.len() == 3 && code.iter().all(u8::is_ascii_digit), "bad status line");
        }
        assert_no_task_left().await;
    });
});
