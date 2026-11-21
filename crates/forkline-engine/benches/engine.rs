//! Phase 1 baseline: request-map cost, per-plan engine overhead, and the cost
//! of dynamic dispatch (one boxed future per async node).
#![allow(missing_docs, unreachable_pub, clippy::unwrap_used, clippy::expect_used)]

use std::hint::black_box;
use std::sync::Arc;

use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use forkline_core::node::erased::DynModule;
use forkline_core::{
    Build, BuildError, Ctx, Decode, Empty, Encode, Interface, Module, NoConfig, NodeError, PlanError, Registry, Reply,
    RequestEnv, RequestId, RequestMap, Terminal,
};
use forkline_engine::{Engine, Request};
use forkline_macros::interface;
use forkline_plan::PlansFile;
use tokio_util::sync::CancellationToken;

interface! {
    #[forkline(crate = forkline_core)]
    pub struct InIo { out payload: Bytes }
}
interface! {
    #[forkline(crate = forkline_core)]
    pub struct StepIo { in payload: Bytes, inout count: u64 }
}
interface! {
    #[forkline(crate = forkline_core)]
    pub struct OutIo { in payload: Bytes }
}

struct In;
impl Build for In {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Decode for In {
    type Io<'a> = InIo<'a>;
    fn decode(&self, _: &Ctx, body: &Bytes, mut io: InIo<'_>) -> Result<(), NodeError> {
        io.set_payload(body.clone());
        Ok(())
    }
}

/// Reads the payload and bumps a counter: the cheapest realistic module.
struct Step;
impl Build for Step {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Module for Step {
    type Io<'a> = StepIo<'a>;
    async fn call(&self, _: &Ctx, mut io: StepIo<'_>) -> Result<(), NodeError> {
        let len = io.payload().map_or(0, Bytes::len) as u64;
        match io.count_mut() {
            Some(c) => *c += len,
            None => io.set_count(len),
        }
        Ok(())
    }
}

struct Out;
impl Build for Out {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Encode for Out {
    type Io<'a> = OutIo<'a>;
    fn encode(&self, _: &Ctx, io: OutIo<'_>) -> Result<Reply, NodeError> {
        Ok(Reply::ok(io.payload().cloned().unwrap_or_default()))
    }
}

struct Fail;
impl Build for Fail {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Terminal for Fail {
    type Io<'a> = Empty<'a>;
    fn respond(&self, _: &Ctx, _: Empty<'_>, e: &PlanError) -> Result<Reply, NodeError> {
        Ok(Reply::error(e.kind, e.to_string()))
    }
}

/// A chain of `nodes` nodes: decode, `nodes - 2` steps, encode.
fn chain_engine(nodes: usize) -> Engine {
    let steps = nodes - 2;
    let mut links = vec![];
    let mut decl = String::new();
    let mut prev = "decode".to_string();
    for i in 0..steps {
        let name = format!("s{i}");
        links.push(format!("\"{prev} -> {name}\""));
        decl.push_str(&format!("{name} = {{ module = \"Step\" }}\n"));
        prev = name;
    }
    links.push(format!("\"{prev} -> encode\""));
    let text = format!(
        "[plans.p]\nroot = \"decode\"\nterminal = \"error\"\ntime_budget_ms = 1000\nlinks = [{}]\n[plans.p.nodes]\n\
         decode = {{ module = \"In\" }}\nencode = {{ module = \"Out\" }}\nerror = {{ module = \"Fail\" }}\n{decl}",
        links.join(", ")
    );
    let registry = Registry::builder()
        .decode::<In>("In")
        .module::<Step>("Step")
        .encode::<Out>("Out")
        .terminal::<Fail>("Fail")
        .build()
        .unwrap();
    Engine::new(Arc::new(registry), &PlansFile::from_toml_str(&text).unwrap()).unwrap()
}

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

fn bench_engine(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
    let mut g = c.benchmark_group("engine_linear");
    for nodes in [3usize, 7, 15] {
        let engine = chain_engine(nodes);
        let body = Bytes::from_static(b"hello");
        g.bench_with_input(BenchmarkId::from_parameter(nodes), &nodes, |b, _| {
            b.to_async(&rt).iter(|| async {
                let reply = engine.call("p", Request::in_process(body.clone())).await.unwrap();
                black_box(reply)
            });
        });
    }
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
