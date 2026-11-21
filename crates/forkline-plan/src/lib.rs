//! Plan configuration, validation and compilation.
//!
//! Plans are TOML. Each one is checked and compiled once when it is loaded;
//! a plan that fails any check is rejected as a whole, with every problem
//! reported at once.
//!
//! * [`check_structure`] runs the graph rules alone and needs no modules. The
//!   `forkline-plan check` CLI uses it.
//! * [`compile`] runs every check against a [`Registry`](forkline_core::Registry),
//!   builds the modules and returns a [`CompiledPlan`] ready to execute.

#![forbid(unsafe_code)]

mod compile;
mod config;
mod diagnostic;
mod graph;

pub use compile::{CompiledNode, CompiledPlan, Edge, KeyInfo, KeyTable, NodeId, compile, compile_all};
pub use config::{ConfigError, Link, NodeConfig, OnConflict, PlanConfig, PlansFile, parse_link};
pub use diagnostic::{Diagnostic, Diagnostics};
pub use graph::{PlanGraph, check_structure};
