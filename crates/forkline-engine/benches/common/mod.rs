//! Modules and plans shared by the benchmarks.
#![allow(dead_code, missing_docs, unreachable_pub, clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use bytes::Bytes;
use forkline_core::{
    Build, BuildError, Ctx, Decode, Empty, Encode, Join, JoinStep, Module, NoConfig, NodeError, PlanError, Registry,
    Reply, Terminal,
};
use forkline_engine::Engine;
use forkline_macros::interface;
use forkline_plan::PlansFile;

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

pub struct In;
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
pub struct Step;
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

/// Touches nothing; for parallel paths.
pub struct Noop;
impl Build for Noop {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Module for Noop {
    type Io<'a> = Empty<'a>;
    async fn call(&self, _: &Ctx, _: Empty<'_>) -> Result<(), NodeError> {
        Ok(())
    }
}

pub struct Pass;
impl Build for Pass {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Join for Pass {
    type Io<'a> = Empty<'a>;
    fn on_path(&self, _: &Ctx, _: Empty<'_>, _: Empty<'_>) -> Result<JoinStep, NodeError> {
        Ok(JoinStep::Wait)
    }
}

pub struct Out;
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

pub struct Fail;
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

fn plan(links: &[String], nodes: &str) -> String {
    format!(
        "[plans.p]\nroot = \"decode\"\nterminal = \"error\"\ntime_budget_ms = 1000\nlinks = [{}]\n[plans.p.nodes]\n\
         decode = {{ module = \"In\" }}\nencode = {{ module = \"Out\" }}\nerror = {{ module = \"Fail\" }}\n{nodes}",
        links.iter().map(|l| format!("\"{l}\"")).collect::<Vec<_>>().join(", ")
    )
}

/// decode, `nodes - 2` steps, encode.
pub fn chain(nodes: usize) -> String {
    let mut links = vec![];
    let mut decl = String::new();
    let mut prev = "decode".to_string();
    for i in 0..nodes - 2 {
        let name = format!("s{i}");
        links.push(format!("{prev} -> {name}"));
        decl.push_str(&format!("{name} = {{ module = \"Step\" }}\n"));
        prev = name;
    }
    links.push(format!("{prev} -> encode"));
    plan(&links, &decl)
}

/// decode forks into `width` parallel no-op paths, a join, encode.
pub fn fan(width: usize) -> String {
    let mut links = vec![];
    let mut decl = String::from("join = { module = \"Pass\" }\n");
    for i in 0..width {
        links.push(format!("decode -> p{i}"));
        links.push(format!("p{i} -> join"));
        decl.push_str(&format!("p{i} = {{ module = \"Noop\" }}\n"));
    }
    links.push("join -> encode".into());
    plan(&links, &decl)
}

/// The plan's target shape: 7 nodes with one fork and one join.
/// decode -> s0 -> {a, b} -> join -> s1 -> encode
pub fn seven_with_fork() -> String {
    let links: Vec<String> =
        ["decode -> s0", "s0 -> a", "s0 -> b", "a -> join", "b -> join", "join -> s1", "s1 -> encode"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
    plan(
        &links,
        "s0 = { module = \"Step\" }\na = { module = \"Noop\" }\nb = { module = \"Noop\" }\n\
         join = { module = \"Pass\" }\ns1 = { module = \"Step\" }\n",
    )
}

pub fn registry() -> Arc<Registry> {
    Arc::new(
        Registry::builder()
            .decode::<In>("In")
            .module::<Step>("Step")
            .module::<Noop>("Noop")
            .join::<Pass>("Pass")
            .encode::<Out>("Out")
            .terminal::<Fail>("Fail")
            .build()
            .unwrap(),
    )
}

pub fn engine(text: &str) -> Engine {
    Engine::new(registry(), &PlansFile::from_toml_str(text).unwrap()).unwrap()
}
