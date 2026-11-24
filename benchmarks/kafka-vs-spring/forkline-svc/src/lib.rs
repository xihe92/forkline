//! `GET /publish?user_id=42&event=click&amount=9.99&currency=USD`: parse and
//! validate the query, publish the event to Kafka as JSON keyed by `user_id`,
//! and reply.
//!
//! ```text
//! parse (decode) ── publish (module) ── reply (encode)
//! ```
//!
//! In [`Mode::Wait`] the reply waits for the broker's acknowledgement and is
//! `{"partition":..,"offset":..}`. In [`Mode::Async`] the record is handed to
//! librdkafka and the reply is `{"accepted":true}` at once; the delivery
//! callback counts the acknowledgement later. `GET /stats` returns the counts.

#![forbid(unsafe_code)]

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use forkline::{
    Build, BuildContext, BuildError, Ctx, Decode, Empty, Encode, ErrorKind, Module, NoConfig, NodeError, PlanError,
    Registry, RegistryError, Reply, Terminal,
};
use http::HeaderValue;
use http::header::CONTENT_TYPE;
use rdkafka::ClientContext;
use rdkafka::message::DeliveryResult;
use rdkafka::producer::{BaseRecord, FutureProducer, FutureRecord, Producer, ProducerContext, ThreadedProducer};
use rdkafka::util::Timeout;
use serde::{Deserialize, Serialize};

/// The event published to Kafka.
#[derive(Clone, Debug, Serialize)]
pub struct Event {
    /// Caller's user id; also the record key.
    pub user_id: u64,
    /// Event name, 1 to 64 characters.
    pub event: String,
    /// Amount, finite.
    pub amount: f64,
    /// Three-letter currency code.
    pub currency: String,
    /// Milliseconds since the Unix epoch when the request was parsed.
    pub ts_ms: u64,
}

/// What the reply reports.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(untagged)]
pub enum Delivery {
    /// Where the broker stored the record (wait mode).
    Stored {
        /// Partition.
        partition: i32,
        /// Offset within the partition.
        offset: i64,
    },
    /// Handed to the Kafka client; the broker hasn't answered yet (async mode).
    Accepted {
        /// Always true.
        accepted: bool,
    },
}

/// Publish counts, shared by the producers' delivery callbacks and `/stats`.
#[derive(Debug, Default)]
pub struct Counters {
    /// Records handed to the Kafka client.
    pub accepted: AtomicU64,
    /// Records the broker acknowledged.
    pub acked: AtomicU64,
    /// Accepted records that failed later (timeout, broker error).
    pub failed: AtomicU64,
    /// Records the client refused at once (not accepted).
    pub refused: AtomicU64,
}

impl Counters {
    fn add(counter: &AtomicU64) -> u64 {
        counter.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn failure(&self, error: &dyn fmt::Display) {
        // Log the first few, then every 10,000th, so a broken broker can't
        // flood the log during a benchmark.
        let n = Self::add(&self.failed);
        if n <= 10 || n.is_multiple_of(10_000) {
            eprintln!("kafka delivery failed ({n} so far): {error}");
        }
    }
}

/// librdkafka's delivery callback, run on the producer's polling thread.
#[derive(Clone, Debug)]
pub struct CountingContext(pub Arc<Counters>);

impl ClientContext for CountingContext {}

impl ProducerContext for CountingContext {
    type DeliveryOpaque = ();

    fn delivery(&self, result: &DeliveryResult<'_>, _: ()) {
        match result {
            Ok(_) => {
                Counters::add(&self.0.acked);
            }
            Err((e, _)) => self.0.failure(e),
        }
    }
}

/// Whether a request waits for the broker's acknowledgement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Reply with the partition and offset once the broker has the record.
    Wait,
    /// Reply once the client has the record; count the acknowledgement later.
    Async,
}

impl std::str::FromStr for Mode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "wait" => Ok(Self::Wait),
            "async" => Ok(Self::Async),
            other => Err(format!("unknown publish mode `{other}` (expected wait or async)")),
        }
    }
}

forkline::interface! {
    /// The parsed request.
    pub struct ParseIo { out event: Event }
}

forkline::interface! {
    /// Publish reads the event and records where it landed.
    pub struct PublishIo { in event: Event, out delivery: Delivery }
}

forkline::interface! {
    /// The reply reads the delivery.
    pub struct ReplyIo { in delivery: Delivery }
}

fn bad(message: &str) -> NodeError {
    NodeError::bad_request(message)
}

/// Parses and validates the query string.
#[derive(Debug)]
pub struct QueryDecoder;

impl Build for QueryDecoder {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}

impl Decode for QueryDecoder {
    type Io<'a> = ParseIo<'a>;
    fn decode(&self, cx: &Ctx, _body: &Bytes, mut io: ParseIo<'_>) -> Result<(), NodeError> {
        let query = cx.env().query.as_deref().unwrap_or_default();
        let (mut user_id, mut event, mut amount, mut currency) = (None, None, 0.0, None);
        for (k, v) in form_urlencoded::parse(query.as_bytes()) {
            match &*k {
                "user_id" => user_id = Some(v.parse::<u64>().map_err(|_| bad("user_id must be an unsigned integer"))?),
                "event" => event = Some(v.into_owned()),
                "amount" => {
                    amount =
                        v.parse::<f64>().ok().filter(|a| a.is_finite()).ok_or_else(|| bad("amount must be a number"))?
                }
                "currency" => currency = Some(v.into_owned()),
                _ => {}
            }
        }
        let user_id = user_id.ok_or_else(|| bad("user_id is required"))?;
        let event = event.ok_or_else(|| bad("event is required"))?;
        if event.is_empty() || event.chars().count() > 64 {
            return Err(bad("event must be 1 to 64 characters"));
        }
        let currency = currency.unwrap_or_else(|| "USD".to_owned());
        if currency.chars().count() != 3 {
            return Err(bad("currency must be a 3-letter code"));
        }
        let ts_ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or_default();
        io.set_event(Event { user_id, event, amount, currency, ts_ms });
        Ok(())
    }
}

/// The producers for one mode; usually one producer.
#[derive(Clone)]
pub enum Producers {
    /// Futures resolved by the broker's acknowledgement.
    Wait(Arc<[FutureProducer]>),
    /// Fire and forget; [`CountingContext`] counts the acknowledgements.
    Async(Arc<[ThreadedProducer<CountingContext>]>),
}

impl Producers {
    fn len(&self) -> usize {
        match self {
            Self::Wait(p) => p.len(),
            Self::Async(p) => p.len(),
        }
    }

    /// Records in librdkafka's queues, waiting to be sent or acknowledged.
    fn in_flight(&self) -> i64 {
        match self {
            Self::Wait(p) => p.iter().map(|p| i64::from(p.in_flight_count())).sum(),
            Self::Async(p) => p.iter().map(|p| i64::from(p.in_flight_count())).sum(),
        }
    }
}

/// The Kafka producers and their counts, registered as a service.
#[derive(Clone)]
pub struct Kafka {
    /// The producers.
    pub producers: Producers,
    /// Shared with the async producers' [`CountingContext`].
    pub counters: Arc<Counters>,
}

impl Kafka {
    /// The mode the producers were built for.
    pub fn mode(&self) -> Mode {
        match self.producers {
            Producers::Wait(_) => Mode::Wait,
            Producers::Async(_) => Mode::Async,
        }
    }
}

impl fmt::Debug for Kafka {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Kafka").field("mode", &self.mode()).finish_non_exhaustive()
    }
}

/// Which producer and topic to publish to.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishConfig {
    /// Service name of the [`Kafka`] producer.
    pub service: String,
    /// Topic to publish to.
    pub topic: String,
}

/// Publishes the event: waits for the broker's acknowledgement, or only
/// hands the record to the client, depending on the service's [`Mode`].
pub struct Publish {
    kafka: Kafka,
    topic: String,
}

impl fmt::Debug for Publish {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Publish").field("topic", &self.topic).field("mode", &self.kafka.mode()).finish()
    }
}

impl Build for Publish {
    type Config = PublishConfig;
    fn build_with(config: PublishConfig, cx: &BuildContext<'_>) -> Result<Self, BuildError> {
        let kafka: Kafka = cx.service(&config.service)?;
        if kafka.producers.len() == 0 {
            return Err(BuildError::msg("the kafka service has no producers"));
        }
        Ok(Self { kafka, topic: config.topic })
    }
}

impl Module for Publish {
    type Io<'a> = PublishIo<'a>;
    async fn call(&self, _cx: &Ctx, mut io: PublishIo<'_>) -> Result<(), NodeError> {
        let event = io.event().ok_or_else(|| NodeError::internal("no event"))?;
        let payload = serde_json::to_vec(event)?;
        let key = event.user_id.to_string();
        let index = (event.user_id % self.kafka.producers.len() as u64) as usize;
        let counters = &self.kafka.counters;
        let delivery = match &self.kafka.producers {
            Producers::Wait(producers) => {
                let record = FutureRecord::to(&self.topic).key(&key).payload(&payload);
                Counters::add(&counters.accepted);
                match producers[index].send(record, Timeout::After(Duration::from_secs(2))).await {
                    Ok(d) => {
                        Counters::add(&counters.acked);
                        Delivery::Stored { partition: d.partition, offset: d.offset }
                    }
                    Err((e, _)) => {
                        counters.failure(&e);
                        return Err(NodeError::new(ErrorKind::Module, format!("kafka: {e}")));
                    }
                }
            }
            Producers::Async(producers) => {
                let record = BaseRecord::to(&self.topic).key(&key).payload(&payload);
                // Counted before the send, so the delivery callback can never
                // run ahead of it; undone if the client refuses the record.
                Counters::add(&counters.accepted);
                match producers[index].send(record) {
                    Ok(()) => Delivery::Accepted { accepted: true },
                    Err((e, _)) => {
                        counters.accepted.fetch_sub(1, Ordering::Relaxed);
                        Counters::add(&counters.refused);
                        return Err(NodeError::new(ErrorKind::Module, format!("kafka refused the record: {e}")));
                    }
                }
            }
        };
        io.set_delivery(delivery);
        Ok(())
    }
}

/// Encodes the delivery as JSON.
#[derive(Debug)]
pub struct JsonReply;

impl Build for JsonReply {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}

const JSON: HeaderValue = HeaderValue::from_static("application/json");

impl Encode for JsonReply {
    type Io<'a> = ReplyIo<'a>;
    fn encode(&self, _cx: &Ctx, io: ReplyIo<'_>) -> Result<Reply, NodeError> {
        let delivery = io.delivery().ok_or_else(|| NodeError::internal("no delivery"))?;
        Ok(Reply::ok(serde_json::to_vec(delivery)?).with_header(CONTENT_TYPE, JSON))
    }
}

/// Replies `{"error": message}` with the failure's status.
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
    fn respond(&self, _cx: &Ctx, _io: Empty<'_>, e: &PlanError) -> Result<Reply, NodeError> {
        let body = serde_json::json!({ "error": e.message }).to_string();
        Ok(Reply::error(e.kind, body).with_header(CONTENT_TYPE, JSON))
    }
}

/// A snapshot of the publish counts, for `GET /stats`.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Stats {
    /// The service's mode.
    pub mode: Mode,
    /// Records handed to the Kafka client.
    pub accepted: u64,
    /// Records the broker acknowledged.
    pub acked: u64,
    /// Accepted records that failed later.
    pub failed: u64,
    /// Records the client refused at once.
    pub refused: u64,
    /// accepted − acked − failed: records still waiting for the broker.
    pub pending: u64,
    /// librdkafka's own count of queued and in-flight records, as a cross-check.
    pub client_queue: i64,
}

forkline::interface! {
    /// The stats snapshot.
    pub struct StatsIo { out stats: Stats }
}

forkline::interface! {
    /// The stats reply reads the snapshot.
    pub struct StatsReplyIo { in stats: Stats }
}

/// Which Kafka service to report on.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatsConfig {
    /// Service name of the [`Kafka`] producer.
    pub service: String,
}

/// Reads the publish counts (ignores the request).
#[derive(Debug)]
pub struct StatsDecoder {
    kafka: Kafka,
}

impl Build for StatsDecoder {
    type Config = StatsConfig;
    fn build_with(config: StatsConfig, cx: &BuildContext<'_>) -> Result<Self, BuildError> {
        Ok(Self { kafka: cx.service(&config.service)? })
    }
}

impl Decode for StatsDecoder {
    type Io<'a> = StatsIo<'a>;
    fn decode(&self, _cx: &Ctx, _body: &Bytes, mut io: StatsIo<'_>) -> Result<(), NodeError> {
        let c = &self.kafka.counters;
        // Read the outcomes before `accepted`, so pending is never negative.
        let acked = c.acked.load(Ordering::Relaxed);
        let failed = c.failed.load(Ordering::Relaxed);
        let refused = c.refused.load(Ordering::Relaxed);
        let accepted = c.accepted.load(Ordering::Relaxed);
        io.set_stats(Stats {
            mode: self.kafka.mode(),
            accepted,
            acked,
            failed,
            refused,
            pending: accepted.saturating_sub(acked + failed),
            client_queue: self.kafka.producers.in_flight(),
        });
        Ok(())
    }
}

/// Encodes the stats snapshot as JSON.
#[derive(Debug)]
pub struct StatsReply;

impl Build for StatsReply {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}

impl Encode for StatsReply {
    type Io<'a> = StatsReplyIo<'a>;
    fn encode(&self, _cx: &Ctx, io: StatsReplyIo<'_>) -> Result<Reply, NodeError> {
        let stats = io.stats().ok_or_else(|| NodeError::internal("no stats"))?;
        Ok(Reply::ok(serde_json::to_vec(stats)?).with_header(CONTENT_TYPE, JSON))
    }
}

/// The modules this service offers.
pub fn registry() -> Result<Registry, RegistryError> {
    Registry::builder()
        .decode::<QueryDecoder>("QueryDecoder")
        .module::<Publish>("Publish")
        .encode::<JsonReply>("JsonReply")
        .terminal::<ErrorReply>("ErrorReply")
        .decode::<StatsDecoder>("StatsDecoder")
        .encode::<StatsReply>("StatsReply")
        .build()
}
