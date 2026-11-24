//! Any HTTP/2 frames into a gRPC listener connection, over an in-memory
//! stream: the input follows the client preface, so the fuzzer works on
//! frames. The server never panics, always closes the connection, answers
//! with complete HTTP/2 frames starting with SETTINGS, and leaves no task
//! behind.
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
static METHODS: LazyLock<BTreeMap<String, String>> = LazyLock::new(|| BTreeMap::from([("P".into(), "p".into())]));

fuzz_target!(|data: &[u8]| {
    RUNTIME.block_on(async {
        let (mut client, server) = tokio::io::duplex(1 << 20);
        let task = tokio::spawn(forkline_server::grpc::serve_stream(server, ENGINE.clone(), &METHODS));
        let _ = client.write_all(H2_PREFACE).await;
        let _ = client.write_all(data).await;
        let _ = client.shutdown().await;
        let mut out = Vec::new();
        let read = tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut out)).await;
        assert!(read.is_ok(), "the connection did not close");
        drop(client);
        task.await.unwrap();
        assert_h2_frames(&out);
        assert_no_task_left().await;
    });
});
