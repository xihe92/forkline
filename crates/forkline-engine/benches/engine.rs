//! Micro-benchmarks: request-map cost, engine overhead per plan shape, fork
//! cost, and the cost of dynamic dispatch (one boxed future per async node).
#![allow(missing_docs, unreachable_pub, clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::hint::black_box;
use std::sync::Arc;

use bytes::Bytes;
use common::{Step, StepIo};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use forkline_core::node::erased::DynModule;
use forkline_core::{Ctx, Interface, Module, RequestEnv, RequestId, RequestMap};
use forkline_engine::{Engine, Request};
use tokio_util::sync::CancellationToken;

fn bench_map(c: &mut Criterion) {
    let mut g = c.benchmark_group("map");
    g.bench_function("set_get_8", |b| {
        b.iter(|| {
            let mut m = RequestMap::with_slots(8);
            for s in 0..8u32 {
                m.set(s, u64::from(s));
            }
            let mut sum = 0;
            for s in 0..8u32 {
                sum += m.get::<u64>(s).copied().unwrap_or(0);
            }
            black_box(sum)
        });
    });
    g.bench_function("clone_32", |b| {
        let mut m = RequestMap::with_slots(32);
        for s in 0..32u32 {
            m.set(s, String::from("value"));
        }
        b.iter(|| black_box(m.clone()));
    });
    g.finish();
}

fn run(engine: &Engine, rt: &tokio::runtime::Runtime, b: &mut criterion::Bencher<'_>) {
    let body = Bytes::from_static(b"hello");
    b.to_async(rt).iter(|| async {
        let reply = engine.call("p", Request::in_process(body.clone())).await.unwrap();
        black_box(reply)
    });
}

fn bench_engine(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();

    let mut g = c.benchmark_group("engine_linear");
    for nodes in [3usize, 7, 15] {
        let engine = common::engine(&common::chain(nodes));
        g.bench_with_input(BenchmarkId::from_parameter(nodes), &nodes, |b, _| run(&engine, &rt, b));
    }
    g.finish();

    let mut g = c.benchmark_group("engine_fork_join");
    for width in [2usize, 4, 8] {
        let engine = common::engine(&common::fan(width));
        g.bench_with_input(BenchmarkId::new("fan", width), &width, |b, _| run(&engine, &rt, b));
    }
    let engine = common::engine(&common::seven_with_fork());
    g.bench_function("seven_nodes_one_fork", |b| run(&engine, &rt, b));
    g.finish();
}

fn bench_dispatch(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
    let cx = Ctx::new(
        RequestId(1),
        Arc::from("bench"),
        tracing::Span::none(),
        tokio::time::Instant::now() + std::time::Duration::from_secs(3600),
        CancellationToken::new(),
        RequestEnv::in_process(),
    );
    let slots: [u32; 2] = [0, 1];
    let step = Step;
    let dyn_step: Arc<dyn DynModule> = Arc::new(Step);
    let mut g = c.benchmark_group("dispatch_10_calls");
    g.bench_function("static", |b| {
        b.to_async(&rt).iter(|| async {
            let mut map = RequestMap::with_slots(2);
            map.set(0, Bytes::from_static(b"hello"));
            for _ in 0..10 {
                Module::call(&step, &cx, StepIo::bind(&mut map, &slots)).await.unwrap();
            }
            black_box(map)
        });
    });
    g.bench_function("dyn_boxed", |b| {
        b.to_async(&rt).iter(|| async {
            let mut map = RequestMap::with_slots(2);
            map.set(0, Bytes::from_static(b"hello"));
            for _ in 0..10 {
                dyn_step.dyn_call(&cx, &mut map, &slots).await.unwrap();
            }
            black_box(map)
        });
    });
    g.finish();
}

criterion_group!(benches, bench_map, bench_engine, bench_dispatch);
criterion_main!(benches);
