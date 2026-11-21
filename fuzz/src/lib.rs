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
