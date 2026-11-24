//! Modules shared by the fuzz targets: every node kind, several key types and
//! configs, so generated plans exercise every validation path.
#![allow(missing_docs, clippy::unwrap_used)]

use bytes::Bytes;
use forkline_core::{
    Branch, Build, BuildError, Ctx, Decode, Empty, Encode, Join, JoinStep, Module, MultiBranch, NoConfig, NodeError,
    PlanError, Registry, Reply, Terminal,
};
use forkline_macros::interface;
use serde::Deserialize;

interface! {
    #[forkline(crate = forkline_core)]
    pub struct DecIo { out a: String, out body: Bytes }
}
interface! {
    #[forkline(crate = forkline_core)]
    pub struct M1Io { in a: String, out b: i64 }
}
interface! {
    #[forkline(crate = forkline_core)]
    pub struct M2Io { opt b: i64, inout c: String, unset a: String }
}
interface! {
    #[forkline(crate = forkline_core)]
    pub struct EncIo { opt a: String, opt b: i64, opt body: Bytes }
}

macro_rules! simple {
    ($name:ident) => {
        pub struct $name;
        impl Build for $name {
            type Config = NoConfig;
            fn build(_: NoConfig) -> Result<Self, BuildError> {
                Ok(Self)
            }
        }
    };
}

simple!(Dec);
impl Decode for Dec {
    type Io<'a> = DecIo<'a>;
    fn decode(&self, _: &Ctx, body: &Bytes, mut io: DecIo<'_>) -> Result<(), NodeError> {
        io.set_body(body.clone());
        io.set_a(String::from_utf8_lossy(body).into_owned());
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct M1Config {
    pub factor: i64,
    #[serde(default)]
    pub name: String,
}
pub struct M1(i64);
impl Build for M1 {
    type Config = M1Config;
    fn build(c: M1Config) -> Result<Self, BuildError> {
        if c.factor == 0 { Err(BuildError::msg("factor must not be 0")) } else { Ok(Self(c.factor)) }
    }
}
impl Module for M1 {
    type Io<'a> = M1Io<'a>;
    async fn call(&self, _: &Ctx, mut io: M1Io<'_>) -> Result<(), NodeError> {
        let n = io.a().map_or(0, |a| a.len() as i64);
        io.set_b(n.wrapping_mul(self.0));
        Ok(())
    }
}

simple!(M2);
impl Module for M2 {
    type Io<'a> = M2Io<'a>;
    async fn call(&self, _: &Ctx, mut io: M2Io<'_>) -> Result<(), NodeError> {
        let b = io.b().copied().unwrap_or_default();
        io.set_c(format!("{b}"));
        io.unset_a();
        Ok(())
    }
}

simple!(Br);
impl Branch for Br {
    type Io<'a> = Empty<'a>;
    fn choose(&self, _: &Ctx, _: Empty<'_>) -> Result<&'static str, NodeError> {
        Ok("x")
    }
}

simple!(Mb);
impl MultiBranch for Mb {
    type Io<'a> = Empty<'a>;
    fn choose(&self, _: &Ctx, _: Empty<'_>) -> Result<Vec<&'static str>, NodeError> {
        Ok(vec!["x", "y"])
    }
}

simple!(J);
impl Join for J {
    type Io<'a> = Empty<'a>;
    fn on_path(&self, _: &Ctx, _: Empty<'_>, _: Empty<'_>) -> Result<JoinStep, NodeError> {
        Ok(JoinStep::Wait)
    }
}

simple!(Enc);
impl Encode for Enc {
    type Io<'a> = EncIo<'a>;
    fn encode(&self, _: &Ctx, io: EncIo<'_>) -> Result<Reply, NodeError> {
        Ok(Reply::ok(io.body().cloned().unwrap_or_default()))
    }
}

simple!(Term);
impl Terminal for Term {
    type Io<'a> = Empty<'a>;
    fn respond(&self, _: &Ctx, _: Empty<'_>, e: &PlanError) -> Result<Reply, NodeError> {
        Ok(Reply::error(e.kind, e.message.clone()))
    }
}

pub fn registry() -> Registry {
    Registry::builder()
        .decode::<Dec>("Dec")
        .module::<M1>("M1")
        .module::<M2>("M2")
        .branch::<Br>("Br")
        .multi_branch::<Mb>("Mb")
        .join::<J>("J")
        .encode::<Enc>("Enc")
        .terminal::<Term>("Term")
        .build()
        .unwrap()
}

/// The HTTP/2 client connection preface.
pub const H2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// Check that `out` is a sequence of complete HTTP/2 frames, the first a
/// SETTINGS frame, as a server must send.
pub fn assert_h2_frames(out: &[u8]) {
    let mut rest = out;
    let mut first = true;
    while !rest.is_empty() {
        assert!(rest.len() >= 9, "truncated HTTP/2 frame header: {rest:?}");
        let len = usize::from(rest[0]) << 16 | usize::from(rest[1]) << 8 | usize::from(rest[2]);
        let kind = rest[3];
        assert!(kind <= 9, "unknown HTTP/2 frame type {kind}");
        if first {
            assert_eq!(kind, 4, "the server's first frame must be SETTINGS");
            first = false;
        }
        assert!(rest.len() >= 9 + len, "truncated HTTP/2 frame: {len} bytes announced, {} left", rest.len() - 9);
        rest = &rest[9 + len..];
    }
}

/// Fail if a task is still alive on the current runtime a second after the
/// connection closed (the leak check the server's tests run).
pub async fn assert_no_task_left() {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        let alive = tokio::runtime::Handle::current().metrics().num_alive_tasks();
        if alive == 0 {
            return;
        }
        assert!(tokio::time::Instant::now() < deadline, "{alive} task(s) still alive after the connection closed");
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}

/// One plan `p` over the test modules, for the connection targets.
pub fn engine() -> std::sync::Arc<forkline_engine::Engine> {
    let plans = forkline_plan::PlansFile::from_toml_str(
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
    .expect("plan");
    std::sync::Arc::new(forkline_engine::Engine::new(std::sync::Arc::new(registry()), &plans).expect("engine"))
}
