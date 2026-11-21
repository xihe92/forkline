//! Core types for the Forkline plan engine.
//!
//! A *module* is a Rust type that implements one node-kind trait ([`Decode`],
//! [`Module`], [`Branch`], [`MultiBranch`], [`Join`], [`Encode`] or
//! [`Terminal`]) and declares the request-map keys it touches with an
//! `interface!` contract. Modules are registered by name in a [`Registry`];
//! plans (in `forkline-plan`) wire registered modules into a graph, and the
//! engine (in `forkline-engine`) runs that graph for every request.

#![forbid(unsafe_code)]

pub mod contract;
pub mod ctx;
pub mod error;
pub mod map;
pub mod node;
pub mod registry;
pub mod reply;

pub use contract::{Access, Empty, Interface, KeySpec};
pub use ctx::{Ctx, Protocol, RequestEnv, RequestId};
pub use error::{BuildError, ErrorKind, NodeError, PlanError};
pub use fixedbitset::FixedBitSet;
pub use map::{RequestMap, Slot};
pub use node::{Branch, Build, Decode, Encode, Join, JoinStep, Module, MultiBranch, NoConfig, Terminal};
pub use registry::{NodeImpl, NodeKind, Registry, RegistryBuilder, RegistryEntry, RegistryError};
pub use reply::{Reply, Status};

/// Items the `interface!` macro expands to. Not part of the public API.
#[doc(hidden)]
pub mod __rt {
    pub use crate::contract::{Access, Interface, KeySpec};
    pub use crate::map::{RequestMap, Slot};

    /// `TypeId::of::<T>` as a function pointer, so contracts can live in a `const`.
    pub fn type_id_of<T: 'static>() -> core::any::TypeId {
        core::any::TypeId::of::<T>()
    }

    /// `type_name::<T>` as a function pointer.
    pub fn type_name_of<T: ?Sized>() -> &'static str {
        core::any::type_name::<T>()
    }

    /// Compile-time check that a contract key type can live in the request map.
    pub const fn assert_map_value<T: Clone + Send + Sync + 'static>() {}

    /// Slot for contract position `index`, or `None` if the binding is short.
    #[inline]
    pub fn slot_at(slots: &[Slot], index: usize) -> Option<Slot> {
        slots.get(index).copied()
    }
}
