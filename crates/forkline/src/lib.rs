//! Forkline: a plan engine that runs a configured graph of modules for every
//! request.
//!
//! This crate re-exports everything a module author needs:
//!
//! * the core types ([`Ctx`], [`RequestMap`], [`Reply`], the node-kind traits
//!   and [`Registry`]),
//! * the [`interface!`] macro for data contracts,
//! * [`plan`] (configuration, validation, compilation) and [`engine`].
//!
//! ```
//! use forkline::{Build, BuildError, Ctx, Module, NoConfig, NodeError};
//!
//! forkline::interface! {
//!     pub struct GreetIo {
//!         in  name: String,
//!         out greeting: String,
//!     }
//! }
//!
//! struct Greet;
//!
//! impl Build for Greet {
//!     type Config = NoConfig;
//!     fn build(_: NoConfig) -> Result<Self, BuildError> {
//!         Ok(Greet)
//!     }
//! }
//!
//! impl Module for Greet {
//!     type Io<'a> = GreetIo<'a>;
//!     async fn call(&self, _cx: &Ctx, mut io: GreetIo<'_>) -> Result<(), NodeError> {
//!         let name = io.name().cloned().unwrap_or_default();
//!         io.set_greeting(format!("hello, {name}"));
//!         Ok(())
//!     }
//! }
//! ```

#![forbid(unsafe_code)]

pub use forkline_core::*;
pub use forkline_engine as engine;
pub use forkline_macros::interface;
pub use forkline_plan as plan;
