//! Data contracts: which request-map keys a module reads and writes.
//!
//! The `interface!` macro generates a typed accessor struct and implements
//! [`Interface`] for it. The plan compiler reads [`Interface::CONTRACT`] to
//! intern keys, check types across modules, and bind each node to its slots.

use std::any::TypeId;
use std::fmt;
use std::marker::PhantomData;

use crate::map::{RequestMap, Slot};

/// How a module uses a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Access {
    /// Read only.
    In,
    /// Written only.
    Out,
    /// Read and written.
    InOut,
    /// Removed from the map.
    Unset,
}

impl Access {
    /// True when the module may read the key.
    pub fn reads(self) -> bool {
        matches!(self, Access::In | Access::InOut)
    }

    /// True when the module may change the key.
    pub fn writes(self) -> bool {
        matches!(self, Access::Out | Access::InOut | Access::Unset)
    }
}

impl fmt::Display for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Access::In => "in",
            Access::Out => "out",
            Access::InOut => "inout",
            Access::Unset => "unset",
        })
    }
}

/// One key in a module's contract.
#[derive(Clone, Copy)]
pub struct KeySpec {
    /// The key's local name; plans may rename it.
    pub name: &'static str,
    /// How the module uses it.
    pub access: Access,
    /// The value type's `TypeId`.
    pub type_id: fn() -> TypeId,
    /// The value type's name, for error messages.
    pub type_name: fn() -> &'static str,
}

impl fmt::Debug for KeySpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}: {}", self.access, self.name, (self.type_name)())
    }
}

/// A typed view of the request map, bound to one node's slots.
///
/// Implemented by the `interface!` macro. `slots[i]` is the map slot for
/// `CONTRACT[i]`.
pub trait Interface<'a>: Sized + Send {
    /// The keys this view reads and writes, in binding order.
    const CONTRACT: &'static [KeySpec];

    /// Bind the view to a map.
    fn bind(map: &'a mut RequestMap, slots: &'a [Slot]) -> Self;
}

/// The interface for a module that touches no keys.
#[derive(Debug)]
pub struct Empty<'a>(PhantomData<&'a mut RequestMap>);

impl<'a> Interface<'a> for Empty<'a> {
    const CONTRACT: &'static [KeySpec] = &[];

    fn bind(_map: &'a mut RequestMap, _slots: &'a [Slot]) -> Self {
        Empty(PhantomData)
    }
}
