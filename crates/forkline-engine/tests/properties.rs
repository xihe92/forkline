//! Property tests: merge rules against a reference model, and scheduling
//! invariants on randomly generated series-parallel plans with branches.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, missing_docs)]

use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use bytes::Bytes;
use forkline_core::{
    Branch, Build, BuildError, Ctx, Decode, Empty, Encode, ErrorKind, Join, JoinStep, Module, NoConfig, NodeError,
    PlanError, Registry, Reply, RequestMap, Status, Terminal,
};
use forkline_engine::merge::merge_paths;
use forkline_engine::{Engine, Request};
use forkline_plan::{OnConflict, PlansFile};
use proptest::collection::vec;
use proptest::prelude::*;
use serde::Deserialize;

// ---------------------------------------------------------------- merge model

#[derive(Clone, Copy, Debug)]
enum Change {
    Keep,
    Set(i64),
    Unset,
}

fn change() -> impl Strategy<Value = Change> {
    prop_oneof![4 => Just(Change::Keep), 2 => (0i64..4).prop_map(Change::Set), 1 => Just(Change::Unset)]
}

fn apply(map: &mut RequestMap, slot: u32, c: Change) {
    match c {
        Change::Keep => {}
        Change::Set(v) => map.set(slot, v),
        Change::Unset => map.unset(slot),
    }
}

fn result_of(base: Option<i64>, c: Change) -> Option<i64> {
    match c {
        Change::Keep => base,
        Change::Set(v) => Some(v),
        Change::Unset => None,
    }
}

type MergeCase = (Vec<Option<i64>>, Vec<Vec<Change>>, Vec<Change>, bool);

fn merge_case() -> impl Strategy<Value = MergeCase> {
    (1usize..8).prop_flat_map(|k| {
        (vec(proptest::option::of(0i64..4), k), vec(vec(change(), k), 1..5), vec(change(), k), any::<bool>())
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, ..ProptestConfig::default() })]

    #[test]
    fn merge_matches_the_reference_model((base, paths, join, drop) in merge_case()) {
        let k = base.len();
        let mut base_map = RequestMap::with_slots(k);
        for (slot, v) in base.iter().enumerate() {
            if let Some(v) = v {
                base_map.set(slot as u32, *v);
            }
        }
        let base_map = base_map.clean_copy();
        let arrivals: Vec<RequestMap> = paths
            .iter()
            .map(|changes| {
                let mut m = base_map.clean_copy();
                for (slot, c) in changes.iter().enumerate() {
                    apply(&mut m, slot as u32, *c);
                }
                m
            })
            .collect();
        let mut merged = base_map.clean_copy();
        for (slot, c) in join.iter().enumerate() {
            apply(&mut merged, slot as u32, *c);
        }
        let policy = if drop { OnConflict::Drop } else { OnConflict::Error };
        let conflicts = merge_paths(&mut merged, &arrivals, policy);

        let mut expected_conflicts = Vec::new();
        for slot in 0..k {
            let s = slot as u32;
            let writers: Vec<Change> =
                paths.iter().map(|p| p[slot]).filter(|c| !matches!(c, Change::Keep)).collect();
            let expected = if !matches!(join[slot], Change::Keep) {
                Some(result_of(base[slot], join[slot]))
            } else {
                match writers.as_slice() {
                    [] => Some(base[slot]),
                    [only] => Some(result_of(base[slot], *only)),
                    _ => {
                        expected_conflicts.push(s);
                        drop.then_some(None)
                    }
                }
            };
            if let Some(expected) = expected {
                prop_assert_eq!(merged.get::<i64>(s).copied(), expected, "slot {}", slot);
            }
            let changed = !matches!(join[slot], Change::Keep) || !writers.is_empty();
            if drop || !expected_conflicts.contains(&s) {
                prop_assert_eq!(merged.is_dirty(s), changed, "dirty slot {}", slot);
            }
        }
        prop_assert_eq!(conflicts, expected_conflicts);
    }
}

// ---------------------------------------------------------- scheduling model

static STARTED: LazyLock<Mutex<Vec<u32>>> = LazyLock::new(Default::default);
static FINISHED: LazyLock<Mutex<Vec<u32>>> = LazyLock::new(Default::default);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StepConfig {
    id: u32,
    delay_ms: u64,
    fail: bool,
}

/// Records when it starts and finishes; may sleep or fail.
struct Step(StepConfig);
impl Build for Step {
    type Config = StepConfig;
    fn build(c: StepConfig) -> Result<Self, BuildError> {
        Ok(Self(c))
    }
}
impl Module for Step {
    type Io<'a> = Empty<'a>;
    async fn call(&self, _: &Ctx, _: Empty<'_>) -> Result<(), NodeError> {
        STARTED.lock().unwrap().push(self.0.id);
        tokio::time::sleep(Duration::from_millis(self.0.delay_ms)).await;
        if self.0.fail {
            return Err(NodeError::msg("injected failure"));
        }
        FINISHED.lock().unwrap().push(self.0.id);
        Ok(())
    }
}

struct Start;
impl Build for Start {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Decode for Start {
    type Io<'a> = Empty<'a>;
    fn decode(&self, _: &Ctx, _: &Bytes, _: Empty<'_>) -> Result<(), NodeError> {
        Ok(())
    }
}

struct Done;
impl Build for Done {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Encode for Done {
    type Io<'a> = Empty<'a>;
    fn encode(&self, _: &Ctx, _: Empty<'_>) -> Result<Reply, NodeError> {
        Ok(Reply::ok("done"))
    }
}

struct Wait;
impl Build for Wait {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Join for Wait {
    type Io<'a> = Empty<'a>;
    fn on_path(&self, _: &Ctx, _: Empty<'_>, _: Empty<'_>) -> Result<JoinStep, NodeError> {
        Ok(JoinStep::Wait)
    }
}

#[derive(Deserialize)]
struct PickConfig {
    label: String,
}
struct Pick(&'static str);
impl Build for Pick {
    type Config = PickConfig;
    fn build(c: PickConfig) -> Result<Self, BuildError> {
        Ok(Self(Box::leak(c.label.into_boxed_str())))
    }
}
impl Branch for Pick {
    type Io<'a> = Empty<'a>;
    fn choose(&self, _: &Ctx, _: Empty<'_>) -> Result<&'static str, NodeError> {
        Ok(self.0)
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
        Ok(Reply::error(e.kind, e.message.clone()))
    }
}

fn registry() -> Arc<Registry> {
    Arc::new(
        Registry::builder()
            .decode::<Start>("Start")
            .module::<Step>("Step")
            .encode::<Done>("Done")
            .join::<Wait>("Wait")
            .branch::<Pick>("Pick")
            .terminal::<Fail>("Fail")
            .build()
            .unwrap(),
    )
}

/// A series-parallel plan shape with exclusive choices.
#[derive(Clone, Debug)]
enum Sp {
    Leaf { delay: u64, fail: bool },
    Seq(Box<Sp>, Box<Sp>),
    Par(Vec<Sp>),
    Choice(Vec<Sp>, usize),
}

fn shape() -> impl Strategy<Value = Sp> {
    let leaf = (0u64..6, proptest::bool::weighted(0.04)).prop_map(|(delay, fail)| Sp::Leaf { delay, fail });
    leaf.prop_recursive(4, 32, 3, |inner| {
        prop_oneof![
            (inner.clone(), inner.clone()).prop_map(|(a, b)| Sp::Seq(Box::new(a), Box::new(b))),
            vec(inner.clone(), 2..=3).prop_map(Sp::Par),
            (vec(inner, 2..=3), any::<proptest::sample::Index>()).prop_map(|(v, i)| {
                let chosen = i.index(v.len());
                Sp::Choice(v, chosen)
            }),
        ]
    })
}

#[derive(Default)]
struct Builder {
    links: Vec<String>,
    nodes: Vec<String>,
    next: u32,
    /// Step ids that must run, and whether a failure is on an active path.
    expected: Vec<u32>,
    fails: bool,
}

impl Builder {
    fn link(&mut self, from: &str, to: &str, label: Option<&str>) {
        match label {
            Some(l) => self.links.push(format!("\"{from} -> {to} : {l}\"")),
            None => self.links.push(format!("\"{from} -> {to}\"")),
        }
    }

    fn step(&mut self, delay: u64, fail: bool, active: bool) -> String {
        let id = self.next;
        self.next += 1;
        let name = format!("s{id}");
        self.nodes.push(format!(
            "{name} = {{ module = \"Step\", config = {{ id = {id}, delay_ms = {delay}, fail = {fail} }} }}"
        ));
        if active {
            self.expected.push(id);
            self.fails |= fail;
        }
        name
    }

    fn fresh(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}{}", self.next)
    }

    /// Add `sp` after `entry`; return the exit node.
    fn build(&mut self, sp: &Sp, entry: &str, label: Option<&str>, active: bool) -> String {
        match sp {
            Sp::Leaf { delay, fail } => {
                let name = self.step(*delay, *fail, active);
                self.link(entry, &name, label);
                name
            }
            Sp::Seq(a, b) => {
                let mid = self.build(a, entry, label, active);
                self.build(b, &mid, None, active)
            }
            Sp::Par(parts) => {
                let fork = self.step(0, false, active);
                self.link(entry, &fork, label);
                let exits: Vec<String> = parts.iter().map(|p| self.build(p, &fork, None, active)).collect();
                let join = self.fresh("j");
                self.nodes.push(format!("{join} = {{ module = \"Wait\" }}"));
                for exit in exits {
                    self.link(&exit, &join, None);
                }
                join
            }
            Sp::Choice(options, chosen) => {
                let branch = self.fresh("b");
                let picked = format!("c{chosen}");
                self.nodes.push(format!("{branch} = {{ module = \"Pick\", config = {{ label = \"{picked}\" }} }}"));
                self.link(entry, &branch, label);
                let exits: Vec<String> = options
                    .iter()
                    .enumerate()
                    .map(|(i, o)| self.build(o, &branch, Some(&format!("c{i}")), active && i == *chosen))
                    .collect();
                let join = self.fresh("j");
                self.nodes.push(format!("{join} = {{ module = \"Wait\" }}"));
                for exit in exits {
                    self.link(&exit, &join, None);
                }
                join
            }
        }
    }
}

fn plan_text(sp: &Sp) -> (String, Vec<u32>, bool) {
    let mut b = Builder::default();
    let exit = b.build(sp, "decode", None, true);
    b.link(&exit, "encode", None);
    let text = format!(
        "[plans.p]\nroot = \"decode\"\nterminal = \"error\"\ntime_budget_ms = 10000\nlinks = [{}]\n[plans.p.nodes]\n\
         decode = {{ module = \"Start\" }}\nencode = {{ module = \"Done\" }}\nerror = {{ module = \"Fail\" }}\n{}\n",
        b.links.join(", "),
        b.nodes.join("\n")
    );
    let mut expected = b.expected;
    expected.sort_unstable();
    (text, expected, b.fails)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

    /// Without failures every node on an active path runs exactly once and no
    /// node on an unchosen branch runs. With a failure the request fails, no
    /// node runs twice, and nothing starts after the reply.
    #[test]
    fn scheduling_invariants(sp in shape()) {
        let (text, expected, fails) = plan_text(&sp);
        let plans = PlansFile::from_toml_str(&text).unwrap();
        let engine = match Engine::new(registry(), &plans) {
            Ok(e) => e,
            Err(d) => panic!("generated plan rejected: {d}\n{text}"),
        };
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().start_paused(true).build().unwrap();
        STARTED.lock().unwrap().clear();
        FINISHED.lock().unwrap().clear();

        let reply = rt.block_on(async { engine.call("p", Request::in_process("")).await }).unwrap();
        let started_at_reply = STARTED.lock().unwrap().clone();
        rt.block_on(async { tokio::time::sleep(Duration::from_secs(60)).await });
        prop_assert_eq!(rt.metrics().num_alive_tasks(), 0, "a task outlived its request\n{}", text);
        let started = STARTED.lock().unwrap().clone();
        let mut finished = FINISHED.lock().unwrap().clone();
        finished.sort_unstable();

        let mut unique = started.clone();
        unique.sort_unstable();
        unique.dedup();
        prop_assert_eq!(unique.len(), started.len(), "a node ran twice: {:?}\n{}", started, text);
        prop_assert!(started.iter().all(|id| expected.contains(id)), "an unchosen node ran\n{}", text);
        prop_assert_eq!(&started_at_reply, &started, "a node started after the reply\n{}", text);

        if fails {
            prop_assert_eq!(reply.status, Status::Error(ErrorKind::Module), "\n{}", text);
        } else {
            prop_assert_eq!(reply.status, Status::Ok, "{:?}\n{}", reply.body, text);
            prop_assert_eq!(finished, expected, "\n{}", text);
        }
    }
}
