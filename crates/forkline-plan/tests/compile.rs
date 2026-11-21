//! Registry-aware checks and compilation.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use bytes::Bytes;
use forkline_core::{
    Build, BuildError, Ctx, Decode, Encode, Join, JoinStep, Module, NoConfig, NodeError, PlanError, Registry, Reply,
    Terminal,
};
use forkline_macros::interface;
use forkline_plan::{PlansFile, compile, compile_all};
use serde::Deserialize;

interface! {
    #[forkline(crate = forkline_core)]
    pub struct DecodeIo {
        out payload: Bytes,
    }
}

interface! {
    #[forkline(crate = forkline_core)]
    pub struct WorkIo {
        in payload: Bytes,
        out svc_response: String,
    }
}

interface! {
    #[forkline(crate = forkline_core)]
    pub struct NumberIo {
        out svc_response: i64,
    }
}

interface! {
    #[forkline(crate = forkline_core)]
    pub struct EncodeIo {
        in svc_response: String,
    }
}

struct Dec;
impl Build for Dec {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Dec)
    }
}
impl Decode for Dec {
    type Io<'a> = DecodeIo<'a>;
    fn decode(&self, _: &Ctx, body: &Bytes, mut io: DecodeIo<'_>) -> Result<(), NodeError> {
        io.set_payload(body.clone());
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkConfig {
    #[serde(default)]
    suffix: String,
}

struct Work {
    _suffix: String,
}
impl Build for Work {
    type Config = WorkConfig;
    fn build(c: WorkConfig) -> Result<Self, BuildError> {
        if c.suffix.len() > 8 {
            return Err(BuildError::msg("suffix longer than 8 bytes"));
        }
        Ok(Work { _suffix: c.suffix })
    }
}
impl Module for Work {
    type Io<'a> = WorkIo<'a>;
    async fn call(&self, _: &Ctx, mut io: WorkIo<'_>) -> Result<(), NodeError> {
        io.set_svc_response(String::new());
        Ok(())
    }
}

struct Number;
impl Build for Number {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Number)
    }
}
impl Module for Number {
    type Io<'a> = NumberIo<'a>;
    async fn call(&self, _: &Ctx, mut io: NumberIo<'_>) -> Result<(), NodeError> {
        io.set_svc_response(1);
        Ok(())
    }
}

struct Enc;
impl Build for Enc {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Enc)
    }
}
impl Encode for Enc {
    type Io<'a> = EncodeIo<'a>;
    fn encode(&self, _: &Ctx, io: EncodeIo<'_>) -> Result<Reply, NodeError> {
        Ok(Reply::ok(io.svc_response().cloned().unwrap_or_default()))
    }
}

struct Err0;
impl Build for Err0 {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Err0)
    }
}
impl Terminal for Err0 {
    type Io<'a> = forkline_core::Empty<'a>;
    fn respond(&self, _: &Ctx, _: Self::Io<'_>, e: &PlanError) -> Result<Reply, NodeError> {
        Ok(Reply::error(e.kind, e.to_string()))
    }
}

struct J;
impl Build for J {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(J)
    }
}
impl Join for J {
    type Io<'a> = forkline_core::Empty<'a>;
    fn on_path(&self, _: &Ctx, _: Self::Io<'_>, _: Self::Io<'_>) -> Result<JoinStep, NodeError> {
        Ok(JoinStep::Wait)
    }
}

fn registry() -> Registry {
    Registry::builder()
        .decode::<Dec>("Dec")
        .module::<Work>("Work")
        .module::<Number>("Number")
        .join::<J>("J")
        .encode::<Enc>("Enc")
        .terminal::<Err0>("Err")
        .build()
        .unwrap()
}

fn errors(text: &str) -> Vec<String> {
    let file = PlansFile::from_toml_str(text).unwrap();
    match compile_all(&file, &registry()) {
        Ok(_) => Vec::new(),
        Err(d) => d.iter().map(ToString::to_string).collect(),
    }
}

const CHAIN: &str = r#"
    [plans.echo]
    root = "decode"
    terminal = "error"
    time_budget_ms = 50
    links = ["decode -> work", "work -> encode"]
    [plans.echo.nodes]
    decode = { module = "Dec" }
    work   = { module = "Work", config = { suffix = "!" } }
    encode = { module = "Enc" }
    error  = { module = "Err" }
"#;

#[test]
fn a_valid_chain_compiles_with_shared_slots() {
    let file = PlansFile::from_toml_str(CHAIN).unwrap();
    let plan = compile("echo", &file.plans["echo"], &registry()).unwrap();
    assert_eq!(plan.keys.len(), 2, "payload and svc_response");
    let payload = plan.keys.slot("payload").unwrap();
    let resp = plan.keys.slot("svc_response").unwrap();
    let decode = &plan.nodes[plan.root];
    assert_eq!(&*decode.name, "decode");
    assert_eq!(&*decode.slots, &[payload]);
    let work = plan.nodes.iter().find(|n| &*n.name == "work").unwrap();
    assert_eq!(&*work.slots, &[payload, resp]);
    assert_eq!(plan.time_budget.as_millis(), 50);
}

#[test]
fn unknown_modules_and_wrong_positions_are_reported() {
    let text =
        CHAIN.replace(r#"work   = { module = "Work", config = { suffix = "!" } }"#, r#"work = { module = "Nope" }"#);
    assert_eq!(errors(&text), vec!["plan `echo`, node `work`: module `Nope` is not registered"]);

    let text = CHAIN.replace(r#"decode = { module = "Dec" }"#, r#"decode = { module = "Enc" }"#);
    let e = errors(&text);
    assert!(e.iter().any(|m| m.contains("the root must be a decode module")), "{e:?}");
    assert!(e.iter().any(|m| m.contains("more than one encode node")), "{e:?}");
}

#[test]
fn module_config_errors_are_reported() {
    let text = CHAIN.replace(r#"suffix = "!""#, r#"suffix = "much too long""#);
    assert_eq!(errors(&text), vec!["plan `echo`, node `work`: module `Work`: suffix longer than 8 bytes"]);

    let text = CHAIN.replace(r#"suffix = "!""#, r#"colour = "red""#);
    let e = errors(&text);
    assert!(e.len() == 1 && e[0].contains("invalid config") && e[0].contains("colour"), "{e:?}");
}

#[test]
fn key_type_conflicts_are_reported() {
    let text = r#"
        [plans.p]
        root = "decode"
        terminal = "error"
        links = ["decode -> a", "a -> b", "b -> encode"]
        [plans.p.nodes]
        decode = { module = "Dec" }
        a      = { module = "Work" }
        b      = { module = "Number" }
        encode = { module = "Enc" }
        error  = { module = "Err" }
    "#;
    let e = errors(text);
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(e[0].contains("key `svc_response` is `i64` here but `alloc::string::String` elsewhere"), "{e:?}");

    // A rename resolves it.
    let fixed = text.replace(
        r#"b      = { module = "Number" }"#,
        r#"b = { module = "Number", rename = { svc_response = "count" } }"#,
    );
    assert_eq!(errors(&fixed), Vec::<String>::new());
}

#[test]
fn bad_renames_are_reported() {
    let text = CHAIN.replace(
        r#"work   = { module = "Work", config = { suffix = "!" } }"#,
        r#"work = { module = "Work", rename = { nope = "x", svc_response = "payload" } }"#,
    );
    let e = errors(&text);
    assert!(e.iter().any(|m| m.contains("rename of `nope`")), "{e:?}");
    assert!(e.iter().any(|m| m.contains("both map to plan key `payload`")), "{e:?}");
}

#[test]
fn joins_need_two_parents_and_multi_parent_nodes_must_be_joins() {
    let text = r#"
        [plans.p]
        root = "decode"
        terminal = "error"
        links = ["decode -> a", "decode -> b", "a -> encode", "b -> encode"]
        [plans.p.nodes]
        decode = { module = "Dec" }
        a      = { module = "Work" }
        b      = { module = "Work" }
        encode = { module = "Enc" }
        error  = { module = "Err" }
    "#;
    let e = errors(text);
    assert!(e.iter().any(|m| m.contains("node `encode`: has 2 incoming links, so it must be a join")), "{e:?}");

    let text = CHAIN
        .replace(r#""work -> encode""#, r#""work -> j", "j -> encode""#)
        .replace(r#"encode = { module = "Enc" }"#, "encode = { module = \"Enc\" }\nj = { module = \"J\" }");
    let e = errors(&text);
    assert!(e.iter().any(|m| m.contains("a join needs at least two incoming links")), "{e:?}");
}

#[test]
fn spawn_only_on_forks() {
    let text = CHAIN.replace(r#"suffix = "!" }"#, r#"suffix = "!" }, spawn = true"#);
    let e = errors(&text);
    assert!(e.iter().any(|m| m.contains("`spawn = true` only applies to a fork")), "{e:?}");
}

#[test]
fn duplicate_registration_fails() {
    let err = Registry::builder().decode::<Dec>("X").module::<Work>("X").build().unwrap_err();
    assert!(err.to_string().contains('X'));
}
