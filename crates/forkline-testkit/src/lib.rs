//! Run one Forkline module in a test: typed inputs, mock services, no server.
//!
//! ```ignore
//! use forkline_testkit::{Harness, inputs};
//!
//! // A module, with a mock service it resolves at build time.
//! let services = ServiceRegistry::new().with("horoscope", HttpClient::from_service("horoscope", mock));
//! let out = Harness::<HoroscopeLookup>::from_config("service = \"horoscope\"", services)?
//!     .inputs(inputs! { sign: String::from("leo") })
//!     .run_module()
//!     .await?;
//! assert_eq!(out.get::<String>("svc_response").map(String::as_str), Some("bold"));
//!
//! // A join, with the merge rules applied.
//! let out = Harness::new(SumJoin)
//!     .fork_base(inputs! { a: 1i64, e: 1i64, f: 1i64 })
//!     .path(inputs! { a: 2i64, e: 2i64, f: 2i64 })
//!     .path(inputs! { a: 3i64, e: 3i64, g: 1i64 })
//!     .on_conflict(OnConflict::Drop)
//!     .run_join()?;
//! assert_eq!(out.conflicts, ["e"]);
//! ```
//!
//! Keys are named as in the module's contract. Keys outside the contract
//! are allowed too (a join merges keys it never declares), and are typed by
//! the value given. Async tests can use tokio's paused clock: the harness
//! builds its deadline from `tokio::time::Instant` when the module runs.

#![forbid(unsafe_code)]

use std::any::{Any, TypeId};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use forkline_core::{
    Branch, Build, BuildContext, BuildError, Ctx, CtxParts, Decode, Encode, Interface, Join, JoinStep, KeySpec, Module,
    MultiBranch, NodeError, PlanError, Reply, RequestEnv, RequestId, RequestMap, ServiceRegistry, Slot, Terminal,
};
use forkline_engine::merge::merge_paths;
use forkline_plan::OnConflict;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Build an [`Inputs`]: `inputs! { sign: String::from("leo"), count: 3u64 }`.
#[macro_export]
macro_rules! inputs {
    ($($key:ident : $value:expr),* $(,)?) => {
        $crate::Inputs::new() $(.set(stringify!($key), $value))*
    };
}

type Value = Arc<dyn Any + Send + Sync>;

/// Named, typed values to put in the request map.
#[derive(Clone, Default)]
pub struct Inputs {
    values: Vec<(String, TypeId, &'static str, Value)>,
}

impl Inputs {
    /// No values.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add (or replace) a value.
    pub fn set<T: Clone + Send + Sync + 'static>(mut self, key: &str, value: T) -> Self {
        self.values.retain(|(k, ..)| k != key);
        self.values.push((key.to_owned(), TypeId::of::<T>(), std::any::type_name::<T>(), Arc::new(value)));
        self
    }
}

impl fmt::Debug for Inputs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.values.iter().map(|(k, _, t, _)| format!("{k}: {t}"))).finish()
    }
}

/// Why a harness run did not produce a result.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TestError {
    /// A value's type does not match the contract or an earlier value.
    #[error("key `{key}` is `{expected}`, but the test gave a `{given}`")]
    WrongType {
        /// Key.
        key: String,
        /// Type in the contract (or first seen).
        expected: &'static str,
        /// Type the test gave.
        given: &'static str,
    },
    /// The module could not be built.
    #[error("build failed: {0}")]
    Build(#[from] BuildError),
    /// The config string is not valid TOML.
    #[error("config: {0}")]
    Config(String),
    /// The module returned an error.
    #[error("{kind}: {message}", kind = .0.kind(), message = .0.message())]
    Node(NodeError),
}

impl From<NodeError> for TestError {
    fn from(e: NodeError) -> Self {
        TestError::Node(e)
    }
}

/// Key names and types for one harness run: the contract first, then extras.
#[derive(Clone, Debug, Default)]
struct Keys {
    names: Vec<String>,
    types: Vec<(TypeId, &'static str)>,
}

impl Keys {
    fn from_contract(contract: &[KeySpec]) -> Self {
        Self {
            names: contract.iter().map(|k| k.name.to_owned()).collect(),
            types: contract.iter().map(|k| ((k.type_id)(), (k.type_name)())).collect(),
        }
    }

    fn slot(&mut self, key: &str, type_id: TypeId, type_name: &'static str) -> Result<Slot, TestError> {
        if let Some(i) = self.names.iter().position(|n| n == key) {
            let (expected_id, expected) = self.types[i];
            if expected_id != type_id {
                return Err(TestError::WrongType { key: key.to_owned(), expected, given: type_name });
            }
            return Ok(Slot::try_from(i).unwrap_or(Slot::MAX));
        }
        self.names.push(key.to_owned());
        self.types.push((type_id, type_name));
        Ok(Slot::try_from(self.names.len() - 1).unwrap_or(Slot::MAX))
    }
}

/// The request map after a run, read by key name.
#[derive(Clone, Debug)]
pub struct Outputs {
    map: RequestMap,
    names: Vec<String>,
}

impl Outputs {
    fn slot(&self, key: &str) -> Option<Slot> {
        self.names.iter().position(|n| n == key).and_then(|i| Slot::try_from(i).ok())
    }

    /// The value of `key`, if present and of type `T`.
    pub fn get<T: 'static>(&self, key: &str) -> Option<&T> {
        self.map.get::<T>(self.slot(key)?)
    }

    /// True when `key` holds a value.
    pub fn contains(&self, key: &str) -> bool {
        self.slot(key).is_some_and(|s| self.map.contains(s))
    }

    /// Keys holding a value, sorted.
    pub fn present(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self
            .names
            .iter()
            .enumerate()
            .filter(|(i, _)| Slot::try_from(*i).is_ok_and(|s| self.map.contains(s)))
            .map(|(_, n)| n.as_str())
            .collect();
        v.sort_unstable();
        v
    }

    /// Keys the run wrote or removed, sorted.
    pub fn changed(&self) -> Vec<&str> {
        let mut v: Vec<&str> =
            self.map.dirty_slots().filter_map(|s| self.names.get(s as usize)).map(String::as_str).collect();
        v.sort_unstable();
        v
    }
}

/// The result of [`Harness::run_join`].
#[derive(Clone, Debug)]
pub struct JoinOutcome {
    /// The merged map.
    pub outputs: Outputs,
    /// Keys changed on several paths and not written by the join, sorted.
    pub conflicts: Vec<String>,
    /// Paths the join saw before it returned `Done` (all of them otherwise).
    pub paths_seen: usize,
}

/// Runs one module.
pub struct Harness<M> {
    module: M,
    services: ServiceRegistry,
    budget: Duration,
    env: RequestEnv,
    inputs: Inputs,
    base: Inputs,
    paths: Vec<Inputs>,
    on_conflict: OnConflict,
}

impl<M> fmt::Debug for Harness<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Harness").field("module", &std::any::type_name::<M>()).field("inputs", &self.inputs).finish()
    }
}

impl<M: Build> Harness<M> {
    /// A harness around an already built module.
    pub fn new(module: M) -> Self {
        Self {
            module,
            services: ServiceRegistry::new(),
            budget: Duration::from_secs(1),
            env: RequestEnv::in_process(),
            inputs: Inputs::new(),
            base: Inputs::new(),
            paths: Vec::new(),
            on_conflict: OnConflict::Error,
        }
    }

    /// Build the module from a TOML config body (the contents of the node's
    /// `config` table, e.g. `service = "horoscope"`), resolving services from
    /// `services` as plan loading would.
    pub fn from_config(config: &str, services: ServiceRegistry) -> Result<Self, TestError> {
        let table: toml::Table = toml::from_str(config).map_err(|e| TestError::Config(e.to_string()))?;
        let config: M::Config =
            toml::Value::Table(table).try_into().map_err(|e: toml::de::Error| TestError::Config(e.to_string()))?;
        let module = M::build_with(config, &BuildContext::new("test", "node", &services))?;
        let mut harness = Self::new(module);
        harness.services = services;
        Ok(harness)
    }

    /// Make a service available to `cx.service(..)` at request time.
    pub fn service<T: Clone + Send + Sync + 'static>(mut self, name: &str, value: T) -> Self {
        self.services.insert(name, value);
        self
    }

    /// Set one input value.
    pub fn input<T: Clone + Send + Sync + 'static>(mut self, key: &str, value: T) -> Self {
        self.inputs = self.inputs.set(key, value);
        self
    }

    /// Set input values.
    pub fn inputs(mut self, inputs: Inputs) -> Self {
        for (k, id, name, v) in inputs.values {
            self.inputs.values.retain(|(key, ..)| *key != k);
            self.inputs.values.push((k, id, name, v));
        }
        self
    }

    /// The request's time budget (default 1 s).
    pub fn budget(mut self, budget: Duration) -> Self {
        self.budget = budget;
        self
    }

    /// The request environment (protocol, peer, metadata).
    pub fn env(mut self, env: RequestEnv) -> Self {
        self.env = env;
        self
    }

    /// The map at the fork, for [`run_join`](Self::run_join).
    pub fn fork_base(mut self, base: Inputs) -> Self {
        self.base = base;
        self
    }

    /// One arriving path's changes relative to the fork base.
    pub fn path(mut self, changes: Inputs) -> Self {
        self.paths.push(changes);
        self
    }

    /// The merge-conflict policy (default `Error`, as in plans).
    pub fn on_conflict(mut self, policy: OnConflict) -> Self {
        self.on_conflict = policy;
        self
    }

    fn cx(&self) -> Ctx {
        Ctx::from_parts(CtxParts {
            request_id: RequestId(1),
            plan: Arc::from("test"),
            span: tracing::Span::none(),
            deadline: Instant::now() + self.budget,
            cancel: CancellationToken::new(),
            env: self.env.clone(),
            services: Arc::new(self.services.clone()),
            runner: None,
            depth: 0,
        })
    }

    fn fill(keys: &mut Keys, map: &mut RequestMap, inputs: &Inputs) -> Result<(), TestError> {
        for (key, type_id, type_name, value) in &inputs.values {
            let slot = keys.slot(key, *type_id, type_name)?;
            map.set_raw(slot, Some(value.clone()));
        }
        Ok(())
    }

    /// Lay out keys and build the input map (with no slots marked written).
    fn prepare(&self, contract: &[KeySpec]) -> Result<(Keys, RequestMap), TestError> {
        let mut keys = Keys::from_contract(contract);
        for (key, type_id, type_name, _) in &self.inputs.values {
            keys.slot(key, *type_id, type_name)?;
        }
        let mut map = RequestMap::with_slots(keys.names.len());
        Self::fill(&mut keys, &mut map, &self.inputs)?;
        map.clear_dirty();
        Ok((keys, map))
    }

    fn slots(n: usize) -> Vec<Slot> {
        (0..n).filter_map(|i| Slot::try_from(i).ok()).collect()
    }
}

macro_rules! contract_of {
    ($m:ty) => {
        <<$m>::Io<'static> as Interface<'static>>::CONTRACT
    };
}

impl<M: Module> Harness<M> {
    /// Run a [`Module`] once.
    pub async fn run_module(self) -> Result<Outputs, TestError> {
        let contract = contract_of!(M);
        let (keys, mut map) = self.prepare(contract)?;
        let slots = Self::slots(contract.len());
        let cx = self.cx();
        let call = self.module.call(&cx, M::Io::bind(&mut map, &slots));
        tokio::time::timeout_at(cx.deadline(), call).await.map_err(|_| {
            NodeError::new(forkline_core::ErrorKind::Deadline, "deadline passed while the node was running")
        })??;
        Ok(Outputs { map, names: keys.names })
    }
}

impl<M: Decode> Harness<M> {
    /// Run a [`Decode`] module on `body`.
    pub fn run_decode(self, body: impl Into<Bytes>) -> Result<Outputs, TestError> {
        let contract = contract_of!(M);
        let (keys, mut map) = self.prepare(contract)?;
        let slots = Self::slots(contract.len());
        self.module.decode(&self.cx(), &body.into(), M::Io::bind(&mut map, &slots))?;
        Ok(Outputs { map, names: keys.names })
    }
}

impl<M: Branch> Harness<M> {
    /// Run a [`Branch`] module and return the label it picked.
    pub fn run_branch(self) -> Result<&'static str, TestError> {
        let contract = contract_of!(M);
        let (_, mut map) = self.prepare(contract)?;
        let slots = Self::slots(contract.len());
        Ok(self.module.choose(&self.cx(), M::Io::bind(&mut map, &slots))?)
    }
}

impl<M: MultiBranch> Harness<M> {
    /// Run a [`MultiBranch`] module and return the labels it picked.
    pub fn run_multi_branch(self) -> Result<Vec<&'static str>, TestError> {
        let contract = contract_of!(M);
        let (_, mut map) = self.prepare(contract)?;
        let slots = Self::slots(contract.len());
        Ok(self.module.choose(&self.cx(), M::Io::bind(&mut map, &slots))?)
    }
}

impl<M: Encode> Harness<M> {
    /// Run an [`Encode`] module.
    pub fn run_encode(self) -> Result<Reply, TestError> {
        let contract = contract_of!(M);
        let (_, mut map) = self.prepare(contract)?;
        let slots = Self::slots(contract.len());
        Ok(self.module.encode(&self.cx(), M::Io::bind(&mut map, &slots))?)
    }
}

impl<M: Terminal> Harness<M> {
    /// Run a [`Terminal`] module for `error`.
    pub fn run_terminal(self, error: &PlanError) -> Result<Reply, TestError> {
        let contract = contract_of!(M);
        let (_, mut map) = self.prepare(contract)?;
        let slots = Self::slots(contract.len());
        Ok(self.module.respond(&self.cx(), M::Io::bind(&mut map, &slots), error)?)
    }
}

impl<M: Join> Harness<M> {
    /// Run a [`Join`] over the configured paths, in order, then apply the
    /// merge rules exactly as the engine does.
    pub fn run_join(self) -> Result<JoinOutcome, TestError> {
        let contract = contract_of!(M);
        let mut keys = Keys::from_contract(contract);
        for inputs in std::iter::once(&self.base).chain(&self.paths) {
            for (key, type_id, type_name, _) in &inputs.values {
                keys.slot(key, *type_id, type_name)?;
            }
        }
        let n = keys.names.len();
        let mut base = RequestMap::with_slots(n);
        Self::fill(&mut keys, &mut base, &self.base)?;
        let base = base.clean_copy();
        let slots = Self::slots(contract.len());
        let cx = self.cx();
        let mut merged = base.clean_copy();
        let mut arrivals = Vec::with_capacity(self.paths.len());
        for changes in &self.paths {
            let mut path = base.clean_copy();
            Self::fill(&mut keys, &mut path, changes)?;
            let step = self.module.on_path(&cx, M::Io::bind(&mut path, &slots), M::Io::bind(&mut merged, &slots))?;
            arrivals.push(path);
            if step == JoinStep::Done {
                break;
            }
        }
        let paths_seen = arrivals.len();
        let conflicts = merge_paths(&mut merged, &arrivals, self.on_conflict)
            .into_iter()
            .filter_map(|s| keys.names.get(s as usize).cloned())
            .collect();
        Ok(JoinOutcome { outputs: Outputs { map: merged, names: keys.names }, conflicts, paths_seen })
    }
}
