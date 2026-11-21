//! Error types.

use std::error::Error as StdError;
use std::fmt;
use std::sync::Arc;

/// Why a plan did not finish normally.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// The request itself was invalid (maps to HTTP 400 / INVALID_ARGUMENT).
    BadRequest,
    /// A module returned an error.
    Module,
    /// The request's deadline passed.
    Deadline,
    /// The request was cancelled.
    Cancelled,
    /// A module panicked.
    Panic,
    /// Paths changed the same key and the plan's `on_conflict` is `error`.
    MergeConflict,
    /// The engine hit a state that validation should have ruled out.
    Internal,
}

impl ErrorKind {
    /// Short lowercase name, used in logs and metric labels.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::BadRequest => "bad_request",
            ErrorKind::Module => "module",
            ErrorKind::Deadline => "deadline",
            ErrorKind::Cancelled => "cancelled",
            ErrorKind::Panic => "panic",
            ErrorKind::MergeConflict => "merge_conflict",
            ErrorKind::Internal => "internal",
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An error returned by a module.
///
/// Any `std::error::Error` converts into it with `?`. Like `anyhow::Error`,
/// it does not itself implement `std::error::Error`, which is what makes that
/// blanket conversion possible.
pub struct NodeError {
    kind: ErrorKind,
    message: String,
    source: Option<Box<dyn StdError + Send + Sync>>,
}

impl NodeError {
    /// A module failure with a message.
    pub fn msg(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Module, message)
    }

    /// The request was malformed; terminals usually answer 400 / INVALID_ARGUMENT.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::BadRequest, message)
    }

    /// An engine-internal failure.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }

    /// An error of a given kind.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), source: None }
    }

    /// The error's kind.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// The error's message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The underlying error, if any.
    pub fn source(&self) -> Option<&(dyn StdError + Send + Sync + 'static)> {
        self.source.as_deref()
    }
}

impl<E: StdError + Send + Sync + 'static> From<E> for NodeError {
    fn from(err: E) -> Self {
        Self { kind: ErrorKind::Module, message: err.to_string(), source: Some(Box::new(err)) }
    }
}

impl fmt::Debug for NodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NodeError")
            .field("kind", &self.kind)
            .field("message", &self.message)
            .field("source", &self.source)
            .finish()
    }
}

impl fmt::Display for NodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// The error a terminal node receives: what failed, where, and why.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{kind} at node `{node}`: {message}")]
pub struct PlanError {
    /// What kind of failure.
    pub kind: ErrorKind,
    /// Name of the node that failed.
    pub node: Arc<str>,
    /// Human-readable detail.
    pub message: String,
}

impl PlanError {
    /// Wrap a module's error with the node it came from.
    pub fn from_node(node: Arc<str>, err: &NodeError) -> Self {
        Self { kind: err.kind(), node, message: err.message().to_owned() }
    }
}

/// A module could not be built from its configuration.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// The node's `config` table did not match the module's config type.
    #[error("invalid config: {0}")]
    Config(String),
    /// The module rejected its configuration.
    #[error("{0}")]
    Other(String),
}

impl BuildError {
    /// A free-form build failure.
    pub fn msg(message: impl Into<String>) -> Self {
        BuildError::Other(message.into())
    }
}
