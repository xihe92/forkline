//! Errors from service calls.

use std::fmt;
use std::sync::Arc;

/// What kind of failure a service call hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ServiceErrorKind {
    /// One attempt exceeded the service's `timeout_ms`.
    Timeout,
    /// The request's own deadline passed.
    Deadline,
    /// The request was cancelled.
    Cancelled,
    /// Refused at the concurrency limit (`load_shed = true`).
    Overloaded,
    /// The circuit breaker is open.
    CircuitOpen,
    /// Connecting or talking to the backend failed.
    Transport,
    /// The backend is temporarily unavailable (HTTP 502/503/504, gRPC UNAVAILABLE).
    Unavailable,
    /// The backend answered with an error status.
    Status,
    /// Anything else.
    Other,
}

impl ServiceErrorKind {
    /// Short lowercase name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Deadline => "deadline",
            Self::Cancelled => "cancelled",
            Self::Overloaded => "overloaded",
            Self::CircuitOpen => "circuit_open",
            Self::Transport => "transport",
            Self::Unavailable => "unavailable",
            Self::Status => "status",
            Self::Other => "other",
        }
    }
}

impl fmt::Display for ServiceErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A failed service call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceError {
    /// What went wrong.
    pub kind: ServiceErrorKind,
    /// Detail.
    pub message: String,
    /// The service's name, once known.
    pub service: Option<Arc<str>>,
}

impl ServiceError {
    /// A new error.
    pub fn new(kind: ServiceErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), service: None }
    }

    /// Attach the service name, if not already set.
    pub fn with_service(mut self, name: &Arc<str>) -> Self {
        if self.service.is_none() {
            self.service = Some(name.clone());
        }
        self
    }

    /// True for failures worth retrying: timeouts, transport errors and
    /// temporary unavailability.
    pub fn is_retryable(&self) -> bool {
        matches!(self.kind, ServiceErrorKind::Timeout | ServiceErrorKind::Transport | ServiceErrorKind::Unavailable)
    }

    /// Recover a `ServiceError` from a tower layer's boxed error.
    pub fn from_box(err: tower::BoxError) -> Self {
        let err = match err.downcast::<ServiceError>() {
            Ok(e) => return *e,
            Err(err) => err,
        };
        if err.is::<tower::timeout::error::Elapsed>() {
            return Self::new(ServiceErrorKind::Timeout, "attempt timed out");
        }
        Self::new(ServiceErrorKind::Other, err.to_string())
    }
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.service {
            Some(name) => write!(f, "service `{name}`: {}: {}", self.kind, self.message),
            None => write!(f, "{}: {}", self.kind, self.message),
        }
    }
}

impl std::error::Error for ServiceError {}
