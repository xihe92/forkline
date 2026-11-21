//! Named services for Forkline modules.
//!
//! A service is a cloneable client registered by name in a
//! [`ServiceRegistry`]. This crate builds those clients from
//! `services.toml`, each wrapped in a tower layer stack:
//!
//! ```toml
//! [services.horoscope]
//! kind = "http"                         # or "grpc", or a kind registered in code
//! url = "http://horoscope.internal:8080/"
//! timeout_ms = 40                       # per attempt; the request deadline also applies
//! retries = 1                           # retry budget: 20% of requests by default
//! concurrency_limit = 64
//! load_shed = true                      # refuse at the limit instead of queueing
//! breaker = { failures = 5, open_ms = 2000 }
//! ```
//!
//! Layers, outermost first: concurrency limit (and shedding), circuit
//! breaker, retry, per-attempt timeout, then the transport. Timeout and retry
//! are tower's; the limit and breaker are small layers here (tower's limit is
//! not `Sync`, which shared clients need, and tower has no breaker).

#![forbid(unsafe_code)]

mod breaker;
mod config;
mod error;
pub mod grpc;
pub mod http;
mod limit;
mod stack;

pub use breaker::BreakerConfig;
pub use config::{ServiceConfig, ServiceKinds, ServicesFile};
pub use error::{ServiceError, ServiceErrorKind};
pub use forkline_core::ServiceRegistry;
pub use grpc::{BytesCodec, GrpcClient};
pub use http::{HttpClient, HttpRequest, HttpResponse};
pub use stack::{StackConfig, layered};
