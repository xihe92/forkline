//! ```sh
//! forkline-kafka-svc serve --config server.toml --plans plans.toml
//! curl 'http://127.0.0.1:8081/publish?user_id=42&event=click&amount=9.99'
//! curl 'http://127.0.0.1:8081/stats'
//! ```
//! `KAFKA_BOOTSTRAP` sets the brokers (default `127.0.0.1:9092`),
//! `KAFKA_LINGER_MS` the producer's batching delay (default 5, as in the Java
//! client) and `PUBLISH_MODE` whether a request waits for the broker's
//! acknowledgement (`wait`, the default) or not (`async`).

#![forbid(unsafe_code)]

use std::process::ExitCode;
use std::sync::Arc;

use forkline_kafka_svc::{Counters, CountingContext, Kafka, Mode, Producers, registry};
use forkline_server::cli::App;
use rdkafka::ClientConfig;
use rdkafka::producer::{FutureProducer, ThreadedProducer};

fn main() -> ExitCode {
    let mode: Mode = match std::env::var("PUBLISH_MODE").unwrap_or_else(|_| "wait".into()).parse() {
        Ok(mode) => mode,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let bootstrap = std::env::var("KAFKA_BOOTSTRAP").unwrap_or_else(|_| "127.0.0.1:9092".into());
    let linger = std::env::var("KAFKA_LINGER_MS").unwrap_or_else(|_| "5".into());
    // The same producer settings as the Spring service's application.properties.
    let mut config = ClientConfig::new();
    config
        .set("bootstrap.servers", &bootstrap)
        .set("acks", "all")
        .set("enable.idempotence", "true")
        .set("linger.ms", &linger)
        .set("compression.type", "none")
        // The Java client's partitioner for keyed records.
        .set("partitioner", "murmur2_random");
    if mode == Mode::Async {
        // No backlog cap: the queue limits at their maximum (defaults 100,000
        // messages and 1 GB). The delivery timeout and batch size are set
        // explicitly to the values the Spring service uses in async mode.
        config
            .set("queue.buffering.max.messages", "2147483647")
            .set("queue.buffering.max.kbytes", "2147483647")
            .set("message.timeout.ms", "300000")
            .set("batch.size", "1000000");
    }
    // Extra librdkafka properties for experiments, e.g. `KAFKA_PROPS=enable.idempotence=false`.
    for kv in std::env::var("KAFKA_PROPS").unwrap_or_default().split(',').filter(|s| !s.is_empty()) {
        if let Some((k, v)) = kv.split_once('=') {
            config.set(k.trim(), v.trim());
        }
    }
    // KAFKA_PRODUCERS > 1 spreads records over several producers (each with
    // its own broker connection), chosen by key so per-key order holds.
    let count = std::env::var("KAFKA_PRODUCERS").ok().and_then(|n| n.parse().ok()).unwrap_or(1usize).max(1);
    let counters = Arc::new(Counters::default());
    let producers = match build(&config, mode, count, &counters) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: cannot create the Kafka producer: {e}");
            return ExitCode::FAILURE;
        }
    };
    match registry() {
        Ok(registry) => App::new(registry).service("kafka", Kafka { producers, counters }).run(),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn build(
    config: &ClientConfig,
    mode: Mode,
    count: usize,
    counters: &Arc<Counters>,
) -> rdkafka::error::KafkaResult<Producers> {
    Ok(match mode {
        Mode::Wait => Producers::Wait((0..count).map(|_| config.create::<FutureProducer>()).collect::<Result<_, _>>()?),
        Mode::Async => Producers::Async(
            (0..count)
                .map(|_| config.create_with_context::<_, ThreadedProducer<_>>(CountingContext(counters.clone())))
                .collect::<Result<_, _>>()?,
        ),
    })
}
