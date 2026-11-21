//! Tail latency of the engine alone (modules do no work, no network).
//!
//! `cargo bench -p forkline-engine --bench latency`
//!
//! Runs each plan shape sequentially on a current-thread runtime, times every
//! request, and prints percentiles. The proposed target is engine overhead
//! under 50 µs at p99 for a 7-node plan with one fork and one join.
#![allow(missing_docs, unreachable_pub, clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::{Duration, Instant};

use bytes::Bytes;
use forkline_engine::Request;

const WARMUP: usize = 20_000;
const SAMPLES: usize = 200_000;

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn measure(name: &str, plan: &str) {
    let engine = common::engine(plan);
    let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
    let body = Bytes::from_static(b"hello");
    let mut samples = Vec::with_capacity(SAMPLES);
    rt.block_on(async {
        for _ in 0..WARMUP {
            engine.call("p", Request::in_process(body.clone())).await.unwrap();
        }
        for _ in 0..SAMPLES {
            let started = Instant::now();
            let reply = engine.call("p", Request::in_process(body.clone())).await.unwrap();
            samples.push(started.elapsed());
            std::hint::black_box(reply);
        }
    });
    samples.sort_unstable();
    let us = |d: Duration| d.as_secs_f64() * 1e6;
    println!(
        "{name:<26} p50 {:>6.2} µs   p90 {:>6.2} µs   p99 {:>6.2} µs   p99.9 {:>6.2} µs   max {:>8.2} µs",
        us(percentile(&samples, 50.0)),
        us(percentile(&samples, 90.0)),
        us(percentile(&samples, 99.0)),
        us(percentile(&samples, 99.9)),
        us(*samples.last().unwrap()),
    );
}

fn main() {
    // `cargo bench` passes `--bench`; nothing else to parse.
    println!("{SAMPLES} sequential requests per shape after {WARMUP} warm-up requests");
    measure("linear, 7 nodes", &common::chain(7));
    measure("fork/join, 7 nodes", &common::seven_with_fork());
    measure("fan-out 8 + join", &common::fan(8));
}
