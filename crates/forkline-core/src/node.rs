//! Node-kind traits.
//!
//! Every module implements [`Build`] (how it is constructed from its config)
//! and exactly one node-kind trait. Only [`Module`] is async: decode, branch,
//! join, encode and terminal steps work on data already in the map, so they
//! are plain functions and never hold the request's task across an await.
//!
//! Forks are not a trait. Any non-branch node with two or more children is a
//! fork.

use std::future::Future;
use std::pin::Pin;

use bytes::Bytes;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::contract::Interface;
use crate::ctx::Ctx;
use crate::error::{BuildError, NodeError, PlanError};
use crate::map::{RequestMap, Slot};
use crate::reply::Reply;

/// How a module is constructed from the `config` table of its plan node.
pub trait Build: Sized + Send + Sync + 'static {
    /// The module's config type. Use [`NoConfig`] when there is none.
    type Config: DeserializeOwned;

    /// Build the module. Called once per plan load, never per request.
    fn build(config: Self::Config) -> Result<Self, BuildError>;
}

/// Config type for modules that take no configuration. Rejects unknown keys.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoConfig {}

/// Turns the raw request body into map values. Plan root only.
pub trait Decode: Build {
    /// The keys this node reads and writes.
    type Io<'a>: Interface<'a>;

    /// Decode `body` into the map.
    fn decode(&self, cx: &Ctx, body: &Bytes, io: Self::Io<'_>) -> Result<(), NodeError>;
}

/// A processing step. One trait covers both quick and I/O-bound work.
///
/// CPU-heavy work belongs in [`Ctx::blocking`].
pub trait Module: Build {
    /// The keys this node reads and writes.
    type Io<'a>: Interface<'a>;

    /// Run the step.
    fn call<'a>(&'a self, cx: &'a Ctx, io: Self::Io<'a>) -> impl Future<Output = Result<(), NodeError>> + Send + 'a;
}

/// Picks exactly one labelled child to run.
pub trait Branch: Build {
    /// The keys this node reads and writes.
    type Io<'a>: Interface<'a>;

    /// Return the label of the child to run.
    fn choose(&self, cx: &Ctx, io: Self::Io<'_>) -> Result<&'static str, NodeError>;
}

/// Picks any number of labelled children to run concurrently.
pub trait MultiBranch: Build {
    /// The keys this node reads and writes.
    type Io<'a>: Interface<'a>;

    /// Return the labels of the children to run.
    fn choose(&self, cx: &Ctx, io: Self::Io<'_>) -> Result<Vec<&'static str>, NodeError>;
}

/// What a join wants after seeing one more path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinStep {
    /// Keep waiting for the remaining paths.
    Wait,
    /// Continue now; paths still running are cancelled.
    Done,
}

/// Merges the paths of a fork back together.
///
/// Called once per arriving path, serially, so it needs no locks.
pub trait Join: Build {
    /// The keys this node reads and writes.
    type Io<'a>: Interface<'a>;

    /// Fold one finished path into the merged map.
    fn on_path(&self, cx: &Ctx, path: Self::Io<'_>, merged: Self::Io<'_>) -> Result<JoinStep, NodeError>;
}

/// Turns map values into the reply.
pub trait Encode: Build {
    /// The keys this node reads and writes.
    type Io<'a>: Interface<'a>;

    /// Produce the reply.
    fn encode(&self, cx: &Ctx, io: Self::Io<'_>) -> Result<Reply, NodeError>;
}

/// Produces the reply when the plan fails.
pub trait Terminal: Build {
    /// The keys this node reads and writes.
    type Io<'a>: Interface<'a>;

    /// Produce the error reply.
    fn respond(&self, cx: &Ctx, io: Self::Io<'_>, error: &PlanError) -> Result<Reply, NodeError>;
}

/// A boxed, sendable future.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Object-safe forms of the node traits, used by the engine.
///
/// Blanket-implemented for every type implementing the matching trait.
pub mod erased {
    use super::*;

    /// Object-safe [`Decode`].
    pub trait DynDecode: Send + Sync {
        /// See [`Decode::decode`].
        fn dyn_decode(&self, cx: &Ctx, body: &Bytes, map: &mut RequestMap, slots: &[Slot]) -> Result<(), NodeError>;
    }

    impl<T: Decode> DynDecode for T {
        fn dyn_decode(&self, cx: &Ctx, body: &Bytes, map: &mut RequestMap, slots: &[Slot]) -> Result<(), NodeError> {
            Decode::decode(self, cx, body, T::Io::bind(map, slots))
        }
    }

    /// Object-safe [`Module`].
    pub trait DynModule: Send + Sync {
        /// See [`Module::call`].
        fn dyn_call<'a>(
            &'a self,
            cx: &'a Ctx,
            map: &'a mut RequestMap,
            slots: &'a [Slot],
        ) -> BoxFuture<'a, Result<(), NodeError>>;
    }

    impl<T: Module> DynModule for T {
        fn dyn_call<'a>(
            &'a self,
            cx: &'a Ctx,
            map: &'a mut RequestMap,
            slots: &'a [Slot],
        ) -> BoxFuture<'a, Result<(), NodeError>> {
            let io = <T::Io<'a> as Interface<'a>>::bind(map, slots);
            Box::pin(Module::call(self, cx, io))
        }
    }

    /// Object-safe [`Branch`].
    pub trait DynBranch: Send + Sync {
        /// See [`Branch::choose`].
        fn dyn_choose(&self, cx: &Ctx, map: &mut RequestMap, slots: &[Slot]) -> Result<&'static str, NodeError>;
    }

    impl<T: Branch> DynBranch for T {
        fn dyn_choose(&self, cx: &Ctx, map: &mut RequestMap, slots: &[Slot]) -> Result<&'static str, NodeError> {
            Branch::choose(self, cx, T::Io::bind(map, slots))
        }
    }

    /// Object-safe [`MultiBranch`].
    pub trait DynMultiBranch: Send + Sync {
        /// See [`MultiBranch::choose`].
        fn dyn_choose(&self, cx: &Ctx, map: &mut RequestMap, slots: &[Slot]) -> Result<Vec<&'static str>, NodeError>;
    }

    impl<T: MultiBranch> DynMultiBranch for T {
        fn dyn_choose(&self, cx: &Ctx, map: &mut RequestMap, slots: &[Slot]) -> Result<Vec<&'static str>, NodeError> {
            MultiBranch::choose(self, cx, T::Io::bind(map, slots))
        }
    }

    /// Object-safe [`Join`].
    pub trait DynJoin: Send + Sync {
        /// See [`Join::on_path`].
        fn dyn_on_path(
            &self,
            cx: &Ctx,
            path: &mut RequestMap,
            merged: &mut RequestMap,
            slots: &[Slot],
        ) -> Result<JoinStep, NodeError>;
    }

    impl<T: Join> DynJoin for T {
        fn dyn_on_path(
            &self,
            cx: &Ctx,
            path: &mut RequestMap,
            merged: &mut RequestMap,
            slots: &[Slot],
        ) -> Result<JoinStep, NodeError> {
            Join::on_path(self, cx, T::Io::bind(path, slots), T::Io::bind(merged, slots))
        }
    }

    /// Object-safe [`Encode`].
    pub trait DynEncode: Send + Sync {
        /// See [`Encode::encode`].
        fn dyn_encode(&self, cx: &Ctx, map: &mut RequestMap, slots: &[Slot]) -> Result<Reply, NodeError>;
    }

    impl<T: Encode> DynEncode for T {
        fn dyn_encode(&self, cx: &Ctx, map: &mut RequestMap, slots: &[Slot]) -> Result<Reply, NodeError> {
            Encode::encode(self, cx, T::Io::bind(map, slots))
        }
    }

    /// Object-safe [`Terminal`].
    pub trait DynTerminal: Send + Sync {
        /// See [`Terminal::respond`].
        fn dyn_respond(
            &self,
            cx: &Ctx,
            map: &mut RequestMap,
            slots: &[Slot],
            error: &PlanError,
        ) -> Result<Reply, NodeError>;
    }

    impl<T: Terminal> DynTerminal for T {
        fn dyn_respond(
            &self,
            cx: &Ctx,
            map: &mut RequestMap,
            slots: &[Slot],
            error: &PlanError,
        ) -> Result<Reply, NodeError> {
            Terminal::respond(self, cx, T::Io::bind(map, slots), error)
        }
    }
}
