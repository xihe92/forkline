//! The horoscope/stock/time application: a fork, a branch and a join.
//!
//! ```text
//!            ┌── time ─────────────────────┐
//! request ───┤                             ├── join ── response
//!            └── branch ─┬─ horoscope ─────┤
//!                        └─ stock ─────────┘
//! ```
//!
//! The request is JSON: `{"want": "horoscope" | "stock", "sign": "leo", "ticker": "ACME"}`.

#![forbid(unsafe_code)]

use bytes::Bytes;
use forkline::{
    Branch, Build, BuildContext, BuildError, Ctx, Decode, Empty, Encode, Join, JoinStep, Module, NoConfig, NodeError,
    PlanError, Registry, RegistryError, Reply, Terminal,
};
use forkline_services::HttpClient;
use serde::{Deserialize, Serialize};

forkline::interface! {
    /// What the request decoder extracts.
    pub struct RequestIo {
        out want: String,
        out sign: String,
        out ticker: String,
    }
}

forkline::interface! {
    /// A lookup that needs nothing from the request.
    pub struct TimeIo {
        out svc_response: String,
    }
}

forkline::interface! {
    /// The branch reads what the caller wants.
    pub struct SelectIo {
        in want: String,
    }
}

forkline::interface! {
    /// Horoscope lookup.
    pub struct HoroscopeIo {
        in  sign: String,
        out svc_response: String,
    }
}

forkline::interface! {
    /// Stock lookup.
    pub struct StockIo {
        in  ticker: String,
        out svc_response: String,
    }
}

forkline::interface! {
    /// The join reads each path's results and builds a summary.
    pub struct ConcatIo {
        opt   svc_response: String,
        opt   time_response: String,
        inout total_response: String,
    }
}

forkline::interface! {
    /// The encoder reads everything the paths produced.
    pub struct ResponseIo {
        in want: String,
        in time_response: String,
        in svc_response: String,
        in total_response: String,
    }
}

#[derive(Deserialize)]
struct Query {
    #[serde(default = "default_want")]
    want: String,
    #[serde(default)]
    sign: String,
    #[serde(default)]
    ticker: String,
}

fn default_want() -> String {
    "horoscope".into()
}

/// Parses the JSON request.
#[derive(Debug)]
pub struct RequestDecoder;

impl Build for RequestDecoder {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}

impl Decode for RequestDecoder {
    type Io<'a> = RequestIo<'a>;
    fn decode(&self, _cx: &Ctx, body: &Bytes, mut io: RequestIo<'_>) -> Result<(), NodeError> {
        let q: Query =
            serde_json::from_slice(body).map_err(|e| NodeError::bad_request(format!("invalid JSON: {e}")))?;
        io.set_want(q.want);
        io.set_sign(q.sign);
        io.set_ticker(q.ticker);
        Ok(())
    }
}

/// Config for the lookups: which service to call.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookupConfig {
    /// Service name in `services.toml`.
    pub service: String,
}

/// `GET /time`.
#[derive(Debug)]
pub struct TimeLookup {
    client: HttpClient,
}

impl Build for TimeLookup {
    type Config = LookupConfig;
    fn build_with(config: LookupConfig, cx: &BuildContext<'_>) -> Result<Self, BuildError> {
        Ok(Self { client: cx.service(&config.service)? })
    }
}

impl Module for TimeLookup {
    type Io<'a> = TimeIo<'a>;
    async fn call(&self, cx: &Ctx, mut io: TimeIo<'_>) -> Result<(), NodeError> {
        let text = self.client.get(cx, "/time").await?.error_for_status()?.text()?;
        io.set_svc_response(text);
        Ok(())
    }
}

/// Picks `horoscope` or `stock`.
#[derive(Debug)]
pub struct SelectBranch;

impl Build for SelectBranch {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}

impl Branch for SelectBranch {
    type Io<'a> = SelectIo<'a>;
    fn choose(&self, _cx: &Ctx, io: SelectIo<'_>) -> Result<&'static str, NodeError> {
        match io.want().map(String::as_str) {
            Some("horoscope") => Ok("horoscope"),
            Some("stock") => Ok("stock"),
            other => Err(NodeError::bad_request(format!("`want` must be horoscope or stock, got {other:?}"))),
        }
    }
}

/// `GET /horoscope/{sign}`.
#[derive(Debug)]
pub struct HoroscopeLookup {
    client: HttpClient,
}

impl Build for HoroscopeLookup {
    type Config = LookupConfig;
    fn build_with(config: LookupConfig, cx: &BuildContext<'_>) -> Result<Self, BuildError> {
        Ok(Self { client: cx.service(&config.service)? })
    }
}

impl Module for HoroscopeLookup {
    type Io<'a> = HoroscopeIo<'a>;
    async fn call(&self, cx: &Ctx, mut io: HoroscopeIo<'_>) -> Result<(), NodeError> {
        let sign = io.sign().cloned().unwrap_or_default();
        if sign.is_empty() || !sign.chars().all(|c| c.is_ascii_alphabetic()) {
            return Err(NodeError::bad_request("`sign` must be letters"));
        }
        let text = self.client.get(cx, format!("/horoscope/{sign}")).await?.error_for_status()?.text()?;
        io.set_svc_response(text);
        Ok(())
    }
}

/// `GET /stock/{ticker}`.
#[derive(Debug)]
pub struct StockLookup {
    client: HttpClient,
}

impl Build for StockLookup {
    type Config = LookupConfig;
    fn build_with(config: LookupConfig, cx: &BuildContext<'_>) -> Result<Self, BuildError> {
        Ok(Self { client: cx.service(&config.service)? })
    }
}

impl Module for StockLookup {
    type Io<'a> = StockIo<'a>;
    async fn call(&self, cx: &Ctx, mut io: StockIo<'_>) -> Result<(), NodeError> {
        let ticker = io.ticker().cloned().unwrap_or_default();
        if ticker.is_empty() || !ticker.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(NodeError::bad_request("`ticker` must be letters and digits"));
        }
        let text = self.client.get(cx, format!("/stock/{ticker}")).await?.error_for_status()?.text()?;
        io.set_svc_response(text);
        Ok(())
    }
}

/// Collects one line per arriving path into `total_response`, sorted so the
/// summary does not depend on which path finished first.
#[derive(Debug)]
pub struct ConcatJoin;

impl Build for ConcatJoin {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}

impl Join for ConcatJoin {
    type Io<'a> = ConcatIo<'a>;
    fn on_path(&self, _cx: &Ctx, path: ConcatIo<'_>, mut merged: ConcatIo<'_>) -> Result<JoinStep, NodeError> {
        let mut lines: Vec<String> =
            merged.total_response().map(|t| t.lines().map(str::to_owned).collect()).unwrap_or_default();
        if let Some(t) = path.time_response() {
            lines.push(format!("time: {t}"));
        }
        if let Some(r) = path.svc_response() {
            lines.push(format!("result: {r}"));
        }
        lines.sort();
        merged.set_total_response(lines.join("\n"));
        Ok(JoinStep::Wait)
    }
}

#[derive(Serialize)]
struct Answer<'a> {
    want: &'a str,
    time: &'a str,
    result: &'a str,
    summary: &'a str,
}

/// Encodes the answer as JSON.
#[derive(Debug)]
pub struct ResponseEncoder;

impl Build for ResponseEncoder {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}

fn json_header() -> (http::HeaderName, http::HeaderValue) {
    (http::header::CONTENT_TYPE, http::HeaderValue::from_static("application/json"))
}

impl Encode for ResponseEncoder {
    type Io<'a> = ResponseIo<'a>;
    fn encode(&self, _cx: &Ctx, io: ResponseIo<'_>) -> Result<Reply, NodeError> {
        let get = |v: Option<&String>| v.map_or("", String::as_str).to_owned();
        let (want, time, result, summary) =
            (get(io.want()), get(io.time_response()), get(io.svc_response()), get(io.total_response()));
        let body = serde_json::to_vec(&Answer { want: &want, time: &time, result: &result, summary: &summary })?;
        let (name, value) = json_header();
        Ok(Reply::ok(body).with_header(name, value))
    }
}

#[derive(Serialize)]
struct Failure<'a> {
    error: &'a str,
    node: &'a str,
    message: &'a str,
}

/// Encodes a failure as JSON with the matching status.
#[derive(Debug)]
pub struct ErrorEncoder;

impl Build for ErrorEncoder {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}

impl Terminal for ErrorEncoder {
    type Io<'a> = Empty<'a>;
    fn respond(&self, _cx: &Ctx, _io: Empty<'_>, error: &PlanError) -> Result<Reply, NodeError> {
        let body =
            serde_json::to_vec(&Failure { error: error.kind.as_str(), node: &error.node, message: &error.message })?;
        let (name, value) = json_header();
        Ok(Reply::error(error.kind, body).with_header(name, value))
    }
}

/// Every module in this application.
pub fn registry() -> Result<Registry, RegistryError> {
    Registry::builder()
        .decode::<RequestDecoder>("RequestDecoder")
        .module::<TimeLookup>("TimeLookup")
        .branch::<SelectBranch>("SelectBranch")
        .module::<HoroscopeLookup>("HoroscopeLookup")
        .module::<StockLookup>("StockLookup")
        .join::<ConcatJoin>("ConcatJoin")
        .encode::<ResponseEncoder>("ResponseEncoder")
        .terminal::<ErrorEncoder>("ErrorEncoder")
        .build()
}
