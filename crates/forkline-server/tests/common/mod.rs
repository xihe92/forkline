//! Test modules and a server harness shared by the server's integration tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic, unreachable_pub, missing_docs)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use forkline_core::{
    Build, BuildContext, BuildError, Ctx, Decode, Empty, Encode, Module, NoConfig, NodeError, PlanError, Pool,
    Registry, Reply, Terminal,
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
    /// The payload is the body, or the query string when the body is empty.
    fn decode(&self, cx: &Ctx, body: &Bytes, mut io: BodyIo<'_>) -> Result<(), NodeError> {
        let payload = match &cx.env().query {
            Some(q) if body.is_empty() => Bytes::from(q.clone()),
            _ => body.clone(),
        };
        io.set_payload(payload);
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
        match text.as_str() {
            "bad" => return Err(NodeError::bad_request("bad input")),
            "fail" => return Err(NodeError::msg("module failed")),
            "panic" => panic!("module panicked"),
            _ => {}
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PooledConfig {
    pub pool: String,
}

/// Runs each request on its thread pool: a payload `<ms>|text` sleeps `<ms>`
/// on a pool thread, then replies `text@<thread name>`.
pub struct Pooled(Pool);
impl Build for Pooled {
    type Config = PooledConfig;
    fn build_with(c: PooledConfig, cx: &BuildContext<'_>) -> Result<Self, BuildError> {
        Ok(Self(cx.pool(&c.pool)?))
    }
}
impl Module for Pooled {
    type Io<'a> = WorkIo<'a>;
    async fn call(&self, cx: &Ctx, mut io: WorkIo<'_>) -> Result<(), NodeError> {
        let text = String::from_utf8_lossy(&io.payload().cloned().unwrap_or_default()).into_owned();
        let (ms, text) =
            text.split_once('|').and_then(|(ms, t)| Some((ms.parse::<u64>().ok()?, t.to_owned()))).unwrap_or((0, text));
        let reply = self
            .0
            .run(cx, move || {
                std::thread::sleep(Duration::from_millis(ms));
                format!("{text}@{}", std::thread::current().name().unwrap_or("?"))
            })
            .await?;
        io.set_reply(Bytes::from(reply));
        Ok(())
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

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub struct MarkConfig {
    pub delay_ms: u64,
    pub name: String,
}

static MARKS: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, u32>>> =
    std::sync::LazyLock::new(Default::default);

/// How many times a `Mark` node named `name` has finished.
pub fn marks(name: &str) -> u32 {
    MARKS.lock().unwrap().get(name).copied().unwrap_or(0)
}

/// Waits `delay_ms`, then counts itself under `name`.
pub struct Mark(MarkConfig);
impl Build for Mark {
    type Config = MarkConfig;
    fn build(c: MarkConfig) -> Result<Self, BuildError> {
        Ok(Self(c))
    }
}
impl Module for Mark {
    type Io<'a> = Empty<'a>;
    async fn call(&self, _: &Ctx, _: Empty<'_>) -> Result<(), NodeError> {
        tokio::time::sleep(Duration::from_millis(self.0.delay_ms)).await;
        *MARKS.lock().unwrap().entry(self.0.name.clone()).or_default() += 1;
        Ok(())
    }
}

/// An error node that fails too, so the request has no reply at all.
pub struct BrokenReply;
impl Build for BrokenReply {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}
impl Terminal for BrokenReply {
    type Io<'a> = Empty<'a>;
    fn respond(&self, _: &Ctx, _: Empty<'_>, _: &PlanError) -> Result<Reply, NodeError> {
        Err(NodeError::msg("the error node broke"))
    }
}

/// The test modules, for code that takes a `Registry` by value.
pub fn modules() -> Registry {
    Registry::builder()
        .decode::<BodyDecoder>("BodyDecoder")
        .module::<Work>("Work")
        .module::<Mark>("Mark")
        .module::<Pooled>("Pooled")
        .encode::<ReplyEncoder>("ReplyEncoder")
        .terminal::<ErrorReply>("ErrorReply")
        .terminal::<BrokenReply>("BrokenReply")
        .build()
        .unwrap()
}

pub fn registry() -> Arc<Registry> {
    Arc::new(modules())
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

/// Plan `name` as in [`plans`], with an error node that fails.
pub fn broken_terminal_plan(name: &str) -> PlansFile {
    let text = format!(
        "[plans.{name}]\nroot = \"decode\"\nterminal = \"error\"\nlinks = [\"decode -> work\", \"work -> encode\"]\n\
         [plans.{name}.nodes]\ndecode = {{ module = \"BodyDecoder\" }}\nwork = {{ module = \"Work\" }}\n\
         encode = {{ module = \"ReplyEncoder\" }}\nerror = {{ module = \"BrokenReply\" }}\n"
    );
    PlansFile::from_toml_str(&text).unwrap()
}

/// Plan `name`: replies at once, and a detached `side` path marks
/// `mark` after `side_ms`.
pub fn detached_plan(name: &str, mark: &str, side_ms: u64, budget_ms: u64) -> PlansFile {
    let text = format!(
        "[plans.{name}]\nroot = \"decode\"\nterminal = \"error\"\ntime_budget_ms = {budget_ms}\n\
         links = [\"decode -> work\", \"work -> encode\", \"decode -> side\"]\n[plans.{name}.nodes]\n\
         decode = {{ module = \"BodyDecoder\" }}\nwork = {{ module = \"Work\" }}\n\
         side = {{ module = \"Mark\", config = {{ delay_ms = {side_ms}, name = \"{mark}\" }} }}\n\
         encode = {{ module = \"ReplyEncoder\" }}\nerror = {{ module = \"ErrorReply\" }}\n"
    );
    PlansFile::from_toml_str(&text).unwrap()
}

/// One unary call to `/forkline.v1.Plans/<method>`.
pub async fn grpc_call(addr: SocketAddr, method: &str, body: &str) -> Result<Bytes, tonic::Status> {
    let channel = tonic::transport::Endpoint::from_shared(format!("http://{addr}")).unwrap().connect().await.unwrap();
    let mut client = tonic::client::Grpc::new(channel);
    client.ready().await.unwrap();
    let request = tonic::Request::new(Bytes::from(body.to_owned()));
    let path = http::uri::PathAndQuery::try_from(format!("/forkline.v1.Plans/{method}")).unwrap();
    client.unary(request, path, forkline_server::grpc::BytesCodec).await.map(tonic::Response::into_inner)
}

pub async fn send_frame(stream: &mut tokio::net::TcpStream, body: &[u8]) {
    use bytes::BufMut;
    use tokio::io::AsyncWriteExt;
    let mut buf = bytes::BytesMut::with_capacity(4 + body.len());
    buf.put_u32(u32::try_from(body.len()).unwrap());
    buf.put_slice(body);
    stream.write_all(&buf).await.unwrap();
}

/// The next response frame as (status byte, body), or `None` once the server closed.
pub async fn read_frame(stream: &mut tokio::net::TcpStream) -> Option<(u8, Bytes)> {
    use bytes::Buf;
    use tokio::io::AsyncReadExt;
    let mut len = [0u8; 4];
    stream.read_exact(&mut len).await.ok()?;
    let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
    stream.read_exact(&mut body).await.ok()?;
    let mut body = Bytes::from(body);
    let status = body.get_u8();
    Some((status, body))
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

    pub fn grpc(&self) -> SocketAddr {
        self.addrs.grpc.unwrap()
    }

    pub async fn tcp(&self) -> tokio::net::TcpStream {
        tokio::net::TcpStream::connect(self.addrs.tcp.unwrap()).await.unwrap()
    }

    pub fn admin(&self, path: &str) -> String {
        format!("http://{}{path}", self.addrs.admin.unwrap())
    }

    pub async fn stop(self) {
        self.shutdown.cancel();
        self.task.await.unwrap().unwrap();
    }
}
