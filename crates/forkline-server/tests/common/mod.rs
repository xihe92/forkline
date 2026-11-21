//! Test modules and a server harness shared by the server's integration tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic, unreachable_pub, missing_docs)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use forkline_core::{
    Build, BuildError, Ctx, Decode, Empty, Encode, Module, NoConfig, NodeError, PlanError, Registry, Reply, Terminal,
};
use forkline_engine::Engine;
use forkline_macros::interface;
use forkline_plan::PlansFile;
use forkline_server::{Bound, BoundServer, Hooks, ServerConfig, ServerError};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

interface! {
    #[forkline(crate = forkline_core)]
    pub struct BodyIo { out payload: Bytes }
}
interface! {
    #[forkline(crate = forkline_core)]
    pub struct WorkIo { in payload: Bytes, out reply: Bytes }
}
interface! {
    #[forkline(crate = forkline_core)]
    pub struct OutIo { in reply: Bytes }
}

pub struct BodyDecoder;
impl Build for BodyDecoder {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Decode for BodyDecoder {
    type Io<'a> = BodyIo<'a>;
    fn decode(&self, _: &Ctx, body: &Bytes, mut io: BodyIo<'_>) -> Result<(), NodeError> {
        io.set_payload(body.clone());
        Ok(())
    }
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub struct WorkConfig {
    pub suffix: String,
    pub delay_ms: u64,
}

/// Replies `payload + suffix` after `delay_ms`. A payload of the form
/// `<ms>|text` sleeps `<ms>` instead and replies `text + suffix`.
pub struct Work(WorkConfig);
impl Build for Work {
    type Config = WorkConfig;
    fn build(c: WorkConfig) -> Result<Self, BuildError> {
        Ok(Self(c))
    }
}
impl Module for Work {
    type Io<'a> = WorkIo<'a>;
    async fn call(&self, _: &Ctx, mut io: WorkIo<'_>) -> Result<(), NodeError> {
        let payload = io.payload().cloned().unwrap_or_default();
        let text = String::from_utf8_lossy(&payload).into_owned();
        let (delay, text) = match text.split_once('|') {
            Some((ms, rest)) if ms.parse::<u64>().is_ok() => (ms.parse().unwrap_or(0), rest.to_owned()),
            _ => (self.0.delay_ms, text),
        };
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        if text == "bad" {
            return Err(NodeError::bad_request("bad input"));
        }
        io.set_reply(Bytes::from(format!("{text}{}", self.0.suffix)));
        Ok(())
    }
}

pub struct ReplyEncoder;
impl Build for ReplyEncoder {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Encode for ReplyEncoder {
    type Io<'a> = OutIo<'a>;
    fn encode(&self, _: &Ctx, io: OutIo<'_>) -> Result<Reply, NodeError> {
        Ok(Reply::ok(io.reply().cloned().unwrap_or_default()))
    }
}

pub struct ErrorReply;
impl Build for ErrorReply {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Terminal for ErrorReply {
    type Io<'a> = Empty<'a>;
    fn respond(&self, _: &Ctx, _: Empty<'_>, e: &PlanError) -> Result<Reply, NodeError> {
        Ok(Reply::error(e.kind, e.message.clone()))
    }
}

pub fn registry() -> Arc<Registry> {
    Arc::new(
        Registry::builder()
            .decode::<BodyDecoder>("BodyDecoder")
            .module::<Work>("Work")
            .encode::<ReplyEncoder>("ReplyEncoder")
            .terminal::<ErrorReply>("ErrorReply")
            .build()
            .unwrap(),
    )
}

/// One plan per (name, suffix, delay_ms, budget_ms).
pub fn plans(specs: &[(&str, &str, u64, u64)]) -> PlansFile {
    let mut text = String::new();
    for (name, suffix, delay, budget) in specs {
        text.push_str(&format!(
            "[plans.{name}]\nroot = \"decode\"\nterminal = \"error\"\ntime_budget_ms = {budget}\n\
             links = [\"decode -> work\", \"work -> encode\"]\n[plans.{name}.nodes]\n\
             decode = {{ module = \"BodyDecoder\" }}\n\
             work = {{ module = \"Work\", config = {{ suffix = \"{suffix}\", delay_ms = {delay} }} }}\n\
             encode = {{ module = \"ReplyEncoder\" }}\nerror = {{ module = \"ErrorReply\" }}\n"
        ));
    }
    PlansFile::from_toml_str(&text).unwrap()
}

#[derive(Debug, Default)]
pub struct Addrs {
    pub http: Option<SocketAddr>,
    pub grpc: Option<SocketAddr>,
    pub tcp: Option<SocketAddr>,
    pub admin: Option<SocketAddr>,
}

pub struct Running {
    pub addrs: Addrs,
    pub shutdown: CancellationToken,
    pub task: tokio::task::JoinHandle<Result<(), ServerError>>,
}

pub async fn start(config: &str, engine: Arc<Engine>, hooks: Hooks) -> Running {
    let config = ServerConfig::from_toml_str(config).unwrap();
    let server = BoundServer::bind_with(&config, engine, hooks).await.unwrap();
    let mut addrs = Addrs::default();
    for b in server.bound() {
        match *b {
            Bound::Http(a) => addrs.http = Some(a),
            Bound::Grpc(a) => addrs.grpc = Some(a),
            Bound::Tcp(a) => addrs.tcp = Some(a),
            Bound::Admin(a) => addrs.admin = Some(a),
            _ => {}
        }
    }
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(server.run(shutdown.clone()));
    Running { addrs, shutdown, task }
}

impl Running {
    pub fn http(&self, path: &str) -> String {
        format!("http://{}{path}", self.addrs.http.unwrap())
    }

    pub fn admin(&self, path: &str) -> String {
        format!("http://{}{path}", self.addrs.admin.unwrap())
    }

    pub async fn stop(self) {
        self.shutdown.cancel();
        self.task.await.unwrap().unwrap();
    }
}
