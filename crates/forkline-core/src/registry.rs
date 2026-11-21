//! The module registry: named factories for every module a binary contains.
//!
//! Modules are compiled into the binary and registered explicitly:
//!
//! ```ignore
//! let registry = Registry::builder()
//!     .decode::<RequestDecoder>("RequestDecoder")
//!     .module::<HoroscopeLookup>("HoroscopeLookup")
//!     .join::<ConcatJoin>("ConcatJoin")
//!     .encode::<ResponseEncoder>("ResponseEncoder")
//!     .terminal::<ErrorEncoder>("ErrorEncoder")
//!     .build()?;
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use serde::de::DeserializeOwned;

use crate::contract::{Interface, KeySpec};
use crate::error::BuildError;
use crate::node::erased::{DynBranch, DynDecode, DynEncode, DynJoin, DynModule, DynMultiBranch, DynTerminal};
use crate::node::{Branch, Build, BuildContext, Decode, Encode, Join, Module, MultiBranch, Terminal};

/// Which node-kind trait a module implements.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NodeKind {
    /// [`Decode`]
    Decode,
    /// [`Module`]
    Module,
    /// [`Branch`]
    Branch,
    /// [`MultiBranch`]
    MultiBranch,
    /// [`Join`]
    Join,
    /// [`Encode`]
    Encode,
    /// [`Terminal`]
    Terminal,
}

impl NodeKind {
    /// Lowercase name for logs and spans.
    pub fn as_str(self) -> &'static str {
        match self {
            NodeKind::Decode => "decode",
            NodeKind::Module => "module",
            NodeKind::Branch => "branch",
            NodeKind::MultiBranch => "multibranch",
            NodeKind::Join => "join",
            NodeKind::Encode => "encode",
            NodeKind::Terminal => "terminal",
        }
    }

    /// True for the kinds whose outgoing links carry labels.
    pub fn is_branching(self) -> bool {
        matches!(self, NodeKind::Branch | NodeKind::MultiBranch)
    }
}

impl fmt::Display for NodeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A built module, type-erased.
#[derive(Clone)]
pub enum NodeImpl {
    /// A [`Decode`] module.
    Decode(Arc<dyn DynDecode>),
    /// A [`Module`].
    Module(Arc<dyn DynModule>),
    /// A [`Branch`] module.
    Branch(Arc<dyn DynBranch>),
    /// A [`MultiBranch`] module.
    MultiBranch(Arc<dyn DynMultiBranch>),
    /// A [`Join`] module.
    Join(Arc<dyn DynJoin>),
    /// An [`Encode`] module.
    Encode(Arc<dyn DynEncode>),
    /// A [`Terminal`] module.
    Terminal(Arc<dyn DynTerminal>),
}

impl NodeImpl {
    /// The node kind.
    pub fn kind(&self) -> NodeKind {
        match self {
            NodeImpl::Decode(_) => NodeKind::Decode,
            NodeImpl::Module(_) => NodeKind::Module,
            NodeImpl::Branch(_) => NodeKind::Branch,
            NodeImpl::MultiBranch(_) => NodeKind::MultiBranch,
            NodeImpl::Join(_) => NodeKind::Join,
            NodeImpl::Encode(_) => NodeKind::Encode,
            NodeImpl::Terminal(_) => NodeKind::Terminal,
        }
    }
}

impl fmt::Debug for NodeImpl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeImpl::{}", self.kind())
    }
}

type Factory = Box<dyn Fn(Option<&toml::Value>, &BuildContext<'_>) -> Result<NodeImpl, BuildError> + Send + Sync>;

/// One registered module.
pub struct RegistryEntry {
    name: String,
    kind: NodeKind,
    contract: &'static [KeySpec],
    type_name: &'static str,
    factory: Factory,
}

impl RegistryEntry {
    /// The name plans use to refer to the module.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The node kind.
    pub fn kind(&self) -> NodeKind {
        self.kind
    }

    /// The module's data contract.
    pub fn contract(&self) -> &'static [KeySpec] {
        self.contract
    }

    /// The Rust type behind the name.
    pub fn type_name(&self) -> &'static str {
        self.type_name
    }

    /// Build an instance from a node's `config` table (`None` = empty table).
    pub fn build(&self, config: Option<&toml::Value>, cx: &BuildContext<'_>) -> Result<NodeImpl, BuildError> {
        (self.factory)(config, cx)
    }
}

impl fmt::Debug for RegistryEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegistryEntry")
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("type", &self.type_name)
            .field("contract", &self.contract)
            .finish()
    }
}

/// The modules a binary contains, by name.
#[derive(Debug)]
pub struct Registry {
    entries: BTreeMap<String, RegistryEntry>,
}

impl Registry {
    /// Start registering modules.
    pub fn builder() -> RegistryBuilder {
        RegistryBuilder::default()
    }

    /// Look a module up by name.
    pub fn get(&self, name: &str) -> Option<&RegistryEntry> {
        self.entries.get(name)
    }

    /// All registered modules, sorted by name.
    pub fn entries(&self) -> impl Iterator<Item = &RegistryEntry> {
        self.entries.values()
    }
}

/// A registration mistake found by [`RegistryBuilder::build`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RegistryError {
    /// The same name was registered more than once.
    #[error("module names registered more than once: {}", .0.iter().cloned().collect::<Vec<_>>().join(", "))]
    Duplicate(BTreeSet<String>),
}

/// Builder for [`Registry`].
#[derive(Default)]
pub struct RegistryBuilder {
    entries: BTreeMap<String, RegistryEntry>,
    duplicates: BTreeSet<String>,
}

impl fmt::Debug for RegistryBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegistryBuilder").field("entries", &self.entries.keys().collect::<Vec<_>>()).finish()
    }
}

fn parse_config<C: DeserializeOwned>(config: Option<&toml::Value>) -> Result<C, BuildError> {
    let value = config.cloned().unwrap_or_else(|| toml::Value::Table(toml::Table::new()));
    value.try_into::<C>().map_err(|e| BuildError::Config(e.to_string()))
}

macro_rules! register_fn {
    ($(#[$doc:meta])* $fn_name:ident, $trait_:ident, $variant:ident) => {
        $(#[$doc])*
        pub fn $fn_name<T: $trait_>(self, name: impl Into<String>) -> Self {
            self.insert::<T>(
                name.into(),
                NodeKind::$variant,
                <T::Io<'static> as Interface<'static>>::CONTRACT,
                Box::new(|config, cx| {
                    let module = T::build_with(parse_config::<T::Config>(config)?, cx)?;
                    Ok(NodeImpl::$variant(Arc::new(module)))
                }),
            )
        }
    };
}

impl RegistryBuilder {
    fn insert<T: Build>(
        mut self,
        name: String,
        kind: NodeKind,
        contract: &'static [KeySpec],
        factory: Factory,
    ) -> Self {
        let entry =
            RegistryEntry { name: name.clone(), kind, contract, type_name: std::any::type_name::<T>(), factory };
        if self.entries.insert(name.clone(), entry).is_some() {
            self.duplicates.insert(name);
        }
        self
    }

    register_fn!(
        /// Register a [`Decode`] module.
        decode,
        Decode,
        Decode
    );
    register_fn!(
        /// Register a [`Module`].
        module,
        Module,
        Module
    );
    register_fn!(
        /// Register a [`Branch`] module.
        branch,
        Branch,
        Branch
    );
    register_fn!(
        /// Register a [`MultiBranch`] module.
        multi_branch,
        MultiBranch,
        MultiBranch
    );
    register_fn!(
        /// Register a [`Join`] module.
        join,
        Join,
        Join
    );
    register_fn!(
        /// Register an [`Encode`] module.
        encode,
        Encode,
        Encode
    );
    register_fn!(
        /// Register a [`Terminal`] module.
        terminal,
        Terminal,
        Terminal
    );

    /// Finish registration. Fails if any name was registered twice.
    pub fn build(self) -> Result<Registry, RegistryError> {
        if !self.duplicates.is_empty() {
            return Err(RegistryError::Duplicate(self.duplicates));
        }
        Ok(Registry { entries: self.entries })
    }
}
