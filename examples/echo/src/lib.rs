//! Echo modules: decode the body, echo it with an optional prefix, encode it.

#![forbid(unsafe_code)]

use std::time::Duration;

use bytes::{Bytes, BytesMut};
use forkline::{
    Build, BuildError, Ctx, Decode, Empty, Encode, Module, NoConfig, NodeError, PlanError, Registry, RegistryError,
    Reply, Terminal,
};
use serde::Deserialize;

forkline::interface! {
    /// The decoder's contract.
    pub struct BodyIo {
        out payload: Bytes,
    }
}

forkline::interface! {
    /// The echo step's contract.
    pub struct EchoIo {
        in  payload: Bytes,
        out reply: Bytes,
    }
}

forkline::interface! {
    /// The encoder's contract.
    pub struct ReplyIo {
        in reply: Bytes,
    }
}

/// Stores the request body as `payload`.
#[derive(Debug)]
pub struct BodyDecoder;

impl Build for BodyDecoder {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}

impl Decode for BodyDecoder {
    type Io<'a> = BodyIo<'a>;
    fn decode(&self, _cx: &Ctx, body: &Bytes, mut io: BodyIo<'_>) -> Result<(), NodeError> {
        io.set_payload(body.clone());
        Ok(())
    }
}

/// Echo settings.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct EchoConfig {
    /// Bytes put in front of the echoed payload.
    pub prefix: String,
    /// Artificial latency, for deadline demos.
    pub delay_ms: u64,
    /// Answer an empty body with a bad-request error.
    pub reject_empty: bool,
}

/// Copies `payload` to `reply`, with the configured prefix.
#[derive(Debug)]
pub struct Echo(EchoConfig);

impl Build for Echo {
    type Config = EchoConfig;
    fn build(config: EchoConfig) -> Result<Self, BuildError> {
        if config.prefix.len() > 1024 {
            return Err(BuildError::msg("prefix is longer than 1024 bytes"));
        }
        Ok(Self(config))
    }
}

impl Module for Echo {
    type Io<'a> = EchoIo<'a>;
    async fn call(&self, _cx: &Ctx, mut io: EchoIo<'_>) -> Result<(), NodeError> {
        if self.0.delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.0.delay_ms)).await;
        }
        let payload = io.payload().cloned().unwrap_or_default();
        if payload.is_empty() && self.0.reject_empty {
            return Err(NodeError::bad_request("empty body"));
        }
        let reply = if self.0.prefix.is_empty() {
            payload
        } else {
            let mut buf = BytesMut::with_capacity(self.0.prefix.len() + payload.len());
            buf.extend_from_slice(self.0.prefix.as_bytes());
            buf.extend_from_slice(&payload);
            buf.freeze()
        };
        io.set_reply(reply);
        Ok(())
    }
}

/// Sends `reply` as the response body.
#[derive(Debug)]
pub struct ReplyEncoder;

impl Build for ReplyEncoder {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}

impl Encode for ReplyEncoder {
    type Io<'a> = ReplyIo<'a>;
    fn encode(&self, _cx: &Ctx, io: ReplyIo<'_>) -> Result<Reply, NodeError> {
        Ok(Reply::ok(io.reply().cloned().unwrap_or_default()))
    }
}

/// Answers failures with the error kind's status and a short message.
#[derive(Debug)]
pub struct ErrorReply;

impl Build for ErrorReply {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}

impl Terminal for ErrorReply {
    type Io<'a> = Empty<'a>;
    fn respond(&self, _cx: &Ctx, _io: Empty<'_>, error: &PlanError) -> Result<Reply, NodeError> {
        Ok(Reply::error(error.kind, error.message.clone()))
    }
}

/// Every module in this binary.
pub fn registry() -> Result<Registry, RegistryError> {
    Registry::builder()
        .decode::<BodyDecoder>("BodyDecoder")
        .module::<Echo>("Echo")
        .encode::<ReplyEncoder>("ReplyEncoder")
        .terminal::<ErrorReply>("ErrorReply")
        .build()
}
