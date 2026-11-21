//! What a plan returns to its adapter.

use bytes::Bytes;
use http::HeaderMap;

use crate::error::ErrorKind;

/// Whether a reply reports success or a failure the adapter should signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Success: HTTP 200, gRPC OK.
    Ok,
    /// Failure of this kind; the adapter maps it to a protocol status.
    Error(ErrorKind),
}

/// The bytes and status a plan sends back.
#[derive(Clone, Debug)]
pub struct Reply {
    /// Success or failure.
    pub status: Status,
    /// Response body.
    pub body: Bytes,
    /// Response headers or trailing gRPC metadata.
    pub headers: HeaderMap,
}

impl Reply {
    /// A successful reply.
    pub fn ok(body: impl Into<Bytes>) -> Self {
        Self { status: Status::Ok, body: body.into(), headers: HeaderMap::new() }
    }

    /// A failure reply. Adapters choose the protocol status from `kind`.
    pub fn error(kind: ErrorKind, body: impl Into<Bytes>) -> Self {
        Self { status: Status::Error(kind), body: body.into(), headers: HeaderMap::new() }
    }

    /// Add a header.
    pub fn with_header(mut self, name: http::header::HeaderName, value: http::HeaderValue) -> Self {
        self.headers.insert(name, value);
        self
    }

    /// `Ok(body)` for a success reply, an error of the reply's kind otherwise.
    pub fn into_result(self) -> Result<Bytes, crate::error::NodeError> {
        match self.status {
            Status::Ok => Ok(self.body),
            Status::Error(kind) => {
                Err(crate::error::NodeError::new(kind, String::from_utf8_lossy(&self.body).into_owned()))
            }
        }
    }
}
