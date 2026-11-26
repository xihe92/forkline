//! Code samples for the Forkline reference book.
//!
//! The book includes these by anchor (`{{#include ...:anchor}}`), and this
//! crate's tests run them, so every sample in the book compiles and works.
//! The plans that wire them up are in `plans.toml` next to this crate.

#![forbid(unsafe_code)]

use bytes::Bytes;
use forkline::{
    Branch, Build, BuildContext, BuildError, Ctx, Decode, Empty, Encode, ErrorKind, Join, JoinStep, Module,
    MultiBranch, NoConfig, NodeError, PlanError, Pool, Registry, RegistryError, Reply, Terminal,
};
use http::HeaderValue;
use http::header::CONTENT_TYPE;
use serde::{Deserialize, Serialize};

// ANCHOR: greet
forkline::interface! {
    /// Reads `name`, writes `greeting`.
    pub struct GreetIo {
        in  name: String,
        out greeting: String,
    }
}

/// Says hello.
#[derive(Debug)]
pub struct Greet;

impl Build for Greet {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Greet)
    }
}

impl Module for Greet {
    type Io<'a> = GreetIo<'a>;
    async fn call(&self, _cx: &Ctx, mut io: GreetIo<'_>) -> Result<(), NodeError> {
        let name = io.name().cloned().unwrap_or_default();
        io.set_greeting(format!("hello, {name}"));
        Ok(())
    }
}
// ANCHOR_END: greet

// ANCHOR: config
/// The `config` table of a `Greeting` node.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GreetingConfig {
    /// Word put before the name.
    pub salutation: String,
    /// Shout the greeting.
    #[serde(default)]
    pub loud: bool,
}

/// Says hello in a configured way.
#[derive(Debug)]
pub struct Greeting {
    salutation: String,
    loud: bool,
}

impl Build for Greeting {
    type Config = GreetingConfig;
    fn build(config: GreetingConfig) -> Result<Self, BuildError> {
        if config.salutation.is_empty() {
            return Err(BuildError::msg("salutation must not be empty"));
        }
        Ok(Self { salutation: config.salutation, loud: config.loud })
    }
}
// ANCHOR_END: config

impl Module for Greeting {
    type Io<'a> = GreetIo<'a>;
    async fn call(&self, _cx: &Ctx, mut io: GreetIo<'_>) -> Result<(), NodeError> {
        let name = io.name().cloned().unwrap_or_default();
        let text = format!("{} {name}", self.salutation);
        io.set_greeting(if self.loud { text.to_uppercase() } else { text });
        Ok(())
    }
}

// ANCHOR: order
/// An order, kept in the request map as one value.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Order {
    /// Order id.
    pub id: String,
    /// 1: standard delivery, 2: express.
    pub flag: u32,
    /// What was ordered.
    pub items: Vec<Item>,
    /// Channels to notify: "email", "sms".
    #[serde(default)]
    pub notify: Vec<String>,
}

/// One line of an order.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Item {
    /// Stock-keeping unit.
    pub sku: String,
    /// How many.
    pub quantity: u32,
    /// Price of one, in cents.
    pub unit_cents: u64,
}

impl Order {
    /// Sum of the items, in cents.
    pub fn subtotal_cents(&self) -> u64 {
        self.items.iter().map(|i| u64::from(i.quantity) * i.unit_cents).sum()
    }
}

forkline::interface! {
    /// The decoder writes the whole order under one key.
    pub struct OrderDecodeIo {
        out order: Order,
    }
}

/// Parses the JSON body into an [`Order`].
#[derive(Debug)]
pub struct OrderDecoder;

impl Build for OrderDecoder {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}

impl Decode for OrderDecoder {
    type Io<'a> = OrderDecodeIo<'a>;
    fn decode(&self, _cx: &Ctx, body: &Bytes, mut io: OrderDecodeIo<'_>) -> Result<(), NodeError> {
        let order: Order =
            serde_json::from_slice(body).map_err(|e| NodeError::bad_request(format!("invalid order: {e}")))?;
        io.set_order(order);
        Ok(())
    }
}
// ANCHOR_END: order

// ANCHOR: branch
forkline::interface! {
    /// A branch reads what it needs to choose.
    pub struct ByFlagIo {
        in order: Order,
    }
}

/// Sends standard orders one way and express orders the other.
#[derive(Debug)]
pub struct ByFlag;

impl Build for ByFlag {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}

impl Branch for ByFlag {
    type Io<'a> = ByFlagIo<'a>;
    fn choose(&self, _cx: &Ctx, io: ByFlagIo<'_>) -> Result<&'static str, NodeError> {
        match io.order().map(|o| o.flag) {
            Some(1) => Ok("standard"),
            Some(2) => Ok("express"),
            other => Err(NodeError::bad_request(format!("flag must be 1 or 2, got {other:?}"))),
        }
    }
}
// ANCHOR_END: branch

// ANCHOR: price
forkline::interface! {
    /// Each delivery path prices the order.
    pub struct PriceIo {
        in  order: Order,
        out price_cents: u64,
    }
}

/// Settings for [`Delivery`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryConfig {
    /// Flat delivery charge, in cents.
    pub charge_cents: u64,
}

/// Adds a delivery charge to the order's subtotal.
#[derive(Debug)]
pub struct Delivery {
    charge_cents: u64,
}

impl Build for Delivery {
    type Config = DeliveryConfig;
    fn build(config: DeliveryConfig) -> Result<Self, BuildError> {
        Ok(Self { charge_cents: config.charge_cents })
    }
}

impl Module for Delivery {
    type Io<'a> = PriceIo<'a>;
    async fn call(&self, _cx: &Ctx, mut io: PriceIo<'_>) -> Result<(), NodeError> {
        let subtotal = io.order().map(Order::subtotal_cents).unwrap_or_default();
        io.set_price_cents(subtotal + self.charge_cents);
        Ok(())
    }
}
// ANCHOR_END: price

// ANCHOR: pass_join
/// Lets the one path a branch picked through. It touches no keys: the
/// engine's merge rules carry that path's changes into the merged map.
#[derive(Debug)]
pub struct Rejoin;

impl Build for Rejoin {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}

impl Join for Rejoin {
    type Io<'a> = Empty<'a>;
    fn on_path(&self, _cx: &Ctx, _path: Empty<'_>, _merged: Empty<'_>) -> Result<JoinStep, NodeError> {
        Ok(JoinStep::Wait)
    }
}
// ANCHOR_END: pass_join

// ANCHOR: multibranch
forkline::interface! {
    /// The multibranch reads the channels the order asks for.
    pub struct ChannelsIo {
        in order: Order,
    }
}

/// Notifies on every channel the order lists, at the same time.
#[derive(Debug)]
pub struct Channels;

impl Build for Channels {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}

impl MultiBranch for Channels {
    type Io<'a> = ChannelsIo<'a>;
    fn choose(&self, _cx: &Ctx, io: ChannelsIo<'_>) -> Result<Vec<&'static str>, NodeError> {
        let mut labels = Vec::new();
        for channel in io.order().map(|o| o.notify.as_slice()).unwrap_or_default() {
            labels.push(match channel.as_str() {
                "email" => "email",
                "sms" => "sms",
                other => return Err(NodeError::bad_request(format!("unknown channel `{other}`"))),
            });
        }
        Ok(labels)
    }
}
// ANCHOR_END: multibranch

// ANCHOR: notify
forkline::interface! {
    /// One notification; plans rename `receipt` per node.
    pub struct NotifyIo {
        in  order: Order,
        out receipt: String,
    }
}

/// Settings for [`Notify`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotifyConfig {
    /// Channel name put in the receipt.
    pub channel: String,
}

/// Sends one notification (here it only writes a receipt).
#[derive(Debug)]
pub struct Notify {
    channel: String,
}

impl Build for Notify {
    type Config = NotifyConfig;
    fn build(config: NotifyConfig) -> Result<Self, BuildError> {
        Ok(Self { channel: config.channel })
    }
}

impl Module for Notify {
    type Io<'a> = NotifyIo<'a>;
    async fn call(&self, _cx: &Ctx, mut io: NotifyIo<'_>) -> Result<(), NodeError> {
        let id = io.order().map(|o| o.id.clone()).unwrap_or_default();
        io.set_receipt(format!("{}: order {id}", self.channel));
        Ok(())
    }
}
// ANCHOR_END: notify

// ANCHOR: join
forkline::interface! {
    /// The join reads each path's receipt and builds the list.
    pub struct ReceiptsIo {
        opt   email_receipt: String,
        opt   sms_receipt: String,
        inout receipts: Vec<String>,
    }
}

/// Collects the receipts of every notification path.
#[derive(Debug)]
pub struct Receipts;

impl Build for Receipts {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}

impl Join for Receipts {
    type Io<'a> = ReceiptsIo<'a>;
    fn on_path(&self, _cx: &Ctx, path: ReceiptsIo<'_>, mut merged: ReceiptsIo<'_>) -> Result<JoinStep, NodeError> {
        let mut all = merged.receipts().cloned().unwrap_or_default();
        all.extend(path.email_receipt().cloned());
        all.extend(path.sms_receipt().cloned());
        all.sort(); // paths arrive in any order
        merged.set_receipts(all);
        Ok(JoinStep::Wait)
    }
}
// ANCHOR_END: join

// ANCHOR: pool
/// Settings for [`Subtotal`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubtotalConfig {
    /// The `[pools.<name>]` to compute on.
    pub pool: String,
}

/// Prices the order on a dedicated thread pool (imagine an expensive model).
#[derive(Debug)]
pub struct Subtotal {
    pool: Pool,
}

impl Build for Subtotal {
    type Config = SubtotalConfig;
    fn build_with(config: SubtotalConfig, cx: &BuildContext<'_>) -> Result<Self, BuildError> {
        Ok(Self { pool: cx.pool(&config.pool)? }) // a missing pool rejects the plan at load
    }
}

impl Module for Subtotal {
    type Io<'a> = PriceIo<'a>;
    async fn call(&self, cx: &Ctx, mut io: PriceIo<'_>) -> Result<(), NodeError> {
        let order = io.order().cloned().ok_or_else(|| NodeError::bad_request("no order"))?;
        let cents = self.pool.run(cx, move || order.subtotal_cents()).await?;
        io.set_price_cents(cents);
        Ok(())
    }
}
// ANCHOR_END: pool

// ANCHOR: subplan
/// Prices the order by running the `price` plan inline.
#[derive(Debug)]
pub struct PriceViaPlan;

impl Build for PriceViaPlan {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}

impl Module for PriceViaPlan {
    type Io<'a> = PriceIo<'a>;
    async fn call(&self, cx: &Ctx, mut io: PriceIo<'_>) -> Result<(), NodeError> {
        let body = serde_json::to_vec(io.order().ok_or_else(|| NodeError::bad_request("no order"))?)?;
        let reply = cx.execute_plan("price", body).await?.into_result()?;
        let cents: u64 = std::str::from_utf8(&reply)?.parse()?;
        io.set_price_cents(cents);
        Ok(())
    }
}
// ANCHOR_END: subplan

// ANCHOR: encode
forkline::interface! {
    /// The quote encoder reads the order and its price.
    pub struct QuoteIo {
        in  order: Order,
        in  price_cents: u64,
        opt receipts: Vec<String>,
    }
}

#[derive(Serialize)]
struct Quote<'a> {
    order: &'a str,
    price_cents: u64,
    receipts: &'a [String],
}

/// Answers with the quote as JSON.
#[derive(Debug)]
pub struct QuoteEncoder;

impl Build for QuoteEncoder {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}

impl Encode for QuoteEncoder {
    type Io<'a> = QuoteIo<'a>;
    fn encode(&self, _cx: &Ctx, io: QuoteIo<'_>) -> Result<Reply, NodeError> {
        let order = io.order().map_or("", |o| o.id.as_str());
        let price_cents = io.price_cents().copied().unwrap_or_default();
        let receipts = io.receipts().map_or(&[][..], Vec::as_slice);
        let body = serde_json::to_vec(&Quote { order, price_cents, receipts })?;
        Ok(Reply::ok(body).with_header(CONTENT_TYPE, HeaderValue::from_static("application/json")))
    }
}
// ANCHOR_END: encode

forkline::interface! {
    /// The text encoder reads the price.
    pub struct PriceTextIo {
        in price_cents: u64,
    }
}

/// Answers with the price alone, as text: what the `price` subplan returns.
#[derive(Debug)]
pub struct PriceText;

impl Build for PriceText {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}

impl Encode for PriceText {
    type Io<'a> = PriceTextIo<'a>;
    fn encode(&self, _cx: &Ctx, io: PriceTextIo<'_>) -> Result<Reply, NodeError> {
        Ok(Reply::ok(io.price_cents().copied().unwrap_or_default().to_string()))
    }
}

// ANCHOR: terminal
#[derive(Serialize)]
struct Failure<'a> {
    error: &'a str,
    node: &'a str,
    message: &'a str,
}

/// Answers every failure with JSON; the adapter picks the status from the kind.
#[derive(Debug)]
pub struct JsonError;

impl Build for JsonError {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> {
        Ok(Self)
    }
}

impl Terminal for JsonError {
    type Io<'a> = Empty<'a>;
    fn respond(&self, _cx: &Ctx, _io: Empty<'_>, error: &PlanError) -> Result<Reply, NodeError> {
        // Don't show internal details to callers; they are in the logs.
        let message = match error.kind {
            ErrorKind::BadRequest => error.message.as_str(),
            _ => "the request failed",
        };
        let body = serde_json::to_vec(&Failure { error: error.kind.as_str(), node: &error.node, message })?;
        Ok(Reply::error(error.kind, body))
    }
}
// ANCHOR_END: terminal

// ANCHOR: registry
/// Every module in this binary, by the name plans use.
pub fn registry() -> Result<Registry, RegistryError> {
    Registry::builder()
        .decode::<OrderDecoder>("OrderDecoder")
        .module::<Greet>("Greet")
        .module::<Greeting>("Greeting")
        .branch::<ByFlag>("ByFlag")
        .module::<Delivery>("Delivery")
        .join::<Rejoin>("Rejoin")
        .multi_branch::<Channels>("Channels")
        .module::<Notify>("Notify")
        .join::<Receipts>("Receipts")
        .module::<Subtotal>("Subtotal")
        .module::<PriceViaPlan>("PriceViaPlan")
        .encode::<QuoteEncoder>("QuoteEncoder")
        .encode::<PriceText>("PriceText")
        .terminal::<JsonError>("JsonError")
        .build()
}
// ANCHOR_END: registry

pub mod kinds;
