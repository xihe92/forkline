//! Registry-aware checks and compilation.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use bytes::Bytes;
use forkline_core::{
    Branch, Build, BuildError, Ctx, Decode, Empty, Encode, Join, JoinStep, Module, NoConfig, NodeError, PlanError,
    Registry, Reply, Terminal,
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

struct Pick;
impl Build for Pick {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Branch for Pick {
    type Io<'a> = Empty<'a>;
    fn choose(&self, _: &Ctx, _: Empty<'_>) -> Result<&'static str, NodeError> {
        Ok("x")
    }
}

fn registry() -> Registry {
    Registry::builder()
        .decode::<Dec>("Dec")
        .branch::<Pick>("Pick")
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

interface! {
    #[forkline(crate = forkline_core)]
    pub struct ReadsIo {
        in svc_response: String,
    }
}

interface! {
    #[forkline(crate = forkline_core)]
    pub struct UnsetIo {
        unset svc_response: String,
    }
}

struct Reads;
impl Build for Reads {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Reads)
    }
}
impl Module for Reads {
    type Io<'a> = ReadsIo<'a>;
    async fn call(&self, _: &Ctx, _: ReadsIo<'_>) -> Result<(), NodeError> {
        Ok(())
    }
}

struct Clear;
impl Build for Clear {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Clear)
    }
}
impl Module for Clear {
    type Io<'a> = UnsetIo<'a>;
    async fn call(&self, _: &Ctx, mut io: UnsetIo<'_>) -> Result<(), NodeError> {
        io.unset_svc_response();
        Ok(())
    }
}

fn flow_registry() -> Registry {
    Registry::builder()
        .decode::<Dec>("Dec")
        .module::<Work>("Work")
        .module::<Reads>("Reads")
        .module::<Clear>("Clear")
        .join::<J>("J")
        .encode::<Enc>("Enc")
        .terminal::<Err0>("Err")
        .build()
        .unwrap()
}

fn flow_errors(links: &str, nodes: &str) -> Vec<String> {
    let text = format!(
        "[plans.p]\nroot = \"decode\"\nterminal = \"error\"\nlinks = [{links}]\n[plans.p.nodes]\n\
         decode = {{ module = \"Dec\" }}\nencode = {{ module = \"Enc\" }}\nerror = {{ module = \"Err\" }}\n{nodes}"
    );
    let file = PlansFile::from_toml_str(&text).unwrap();
    match compile_all(&file, &flow_registry()) {
        Ok(_) => Vec::new(),
        Err(d) => d.iter().map(ToString::to_string).collect(),
    }
}

#[test]
fn reads_need_an_upstream_write() {
    // `encode` reads svc_response, but nothing writes it.
    let e = flow_errors(r#""decode -> encode""#, "");
    assert_eq!(
        e,
        vec![
            "plan `p`, node `encode`: reads `svc_response`, but it is not written upstream on every path \
             (declare it `opt` if it may be absent)"
        ]
    );
    // Written upstream: fine.
    assert!(flow_errors(r#""decode -> w", "w -> encode""#, r#"w = { module = "Work" }"#).is_empty());
}

#[test]
fn an_unset_removes_a_key_from_what_is_available() {
    let e = flow_errors(
        r#""decode -> w", "w -> c", "c -> encode""#,
        "w = { module = \"Work\" }\nc = { module = \"Clear\" }",
    );
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(e[0].contains("node `encode`: reads `svc_response`"), "{e:?}");
}

#[test]
fn reads_before_the_write_on_the_same_path_are_caught() {
    let e = flow_errors(
        r#""decode -> r", "r -> w", "w -> encode""#,
        "r = { module = \"Reads\" }\nw = { module = \"Work\" }",
    );
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(e[0].contains("node `r`: reads `svc_response`"), "{e:?}");
}

#[test]
fn a_join_sees_the_union_of_its_paths() {
    // One fork path writes svc_response, the other does not; after the join it is available.
    let e = flow_errors(
        r#""decode -> w", "decode -> r0", "w -> j", "r0 -> j", "j -> encode""#,
        "w = { module = \"Work\" }\nr0 = { module = \"Clear\" }\nj = { module = \"J\" }",
    );
    assert!(e.is_empty(), "{e:?}");
}

fn assert_reported(text: &str, needle: &str) {
    let e = errors(text);
    assert!(e.iter().any(|m| m.contains(needle)), "expected {needle:?} in {e:?}");
}

#[test]
fn each_module_kind_must_sit_where_the_plan_allows_it() {
    let work = r#"work   = { module = "Work", config = { suffix = "!" } }"#;
    assert_reported(
        &CHAIN.replace(work, r#"work = { module = "Dec" }"#),
        "node `work`: a decode module can only be the root",
    );
    assert_reported(
        &CHAIN.replace(work, r#"work = { module = "Err" }"#),
        "node `work`: a terminal module can only be the plan's terminal",
    );
    assert_reported(
        &CHAIN.replace(r#"error  = { module = "Err" }"#, r#"error = { module = "Work" }"#),
        "node `error`: the terminal must be a terminal module, `Work` is a module",
    );
    assert_reported(
        &CHAIN.replace(r#"encode = { module = "Enc" }"#, r#"encode = { module = "Work" }"#),
        "plan `echo`: the plan has no encode node",
    );
}

#[test]
fn labelled_links_belong_to_branches_and_branches_need_labels() {
    assert_reported(
        &CHAIN.replace(r#""decode -> work""#, r#""decode -> work : x""#),
        "node `decode`: has labelled links, so it must be a branch or multibranch module, `Dec` is a decode",
    );
    let work = r#"work   = { module = "Work", config = { suffix = "!" } }"#;
    assert_reported(
        &CHAIN.replace(work, r#"work = { module = "Pick" }"#),
        "node `work`: links out of a branch must carry labels",
    );
    // A branch at the end of a detached path has nowhere to go.
    let text = CHAIN
        .replace(r#""work -> encode""#, r#""work -> encode", "decode -> pick""#)
        .replace(r#"error  = { module = "Err" }"#, "error = { module = \"Err\" }\npick = { module = \"Pick\" }");
    assert_reported(&text, "node `pick`: a branch needs at least one labelled link");
}

#[test]
fn plans_files_that_cannot_be_read_name_the_path() {
    let err = PlansFile::load("/nonexistent/plans.toml").unwrap_err().to_string();
    assert!(err.contains("/nonexistent/plans.toml"), "{err}");
}
