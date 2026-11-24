# Forkline vs Spring MVC: an HTTP GET that publishes to Kafka

> **Manual run only.** CI never builds or runs this directory: it needs a local Kafka, a JDK and
> fixed hardware to give comparable numbers. Rerun it by hand before each release.

The same service built twice:

`GET /publish?user_id=42&event=click&amount=9.99&currency=USD`

Both versions parse and validate the query, serialise the event as JSON, publish it to the
`events` topic keyed by `user_id`, **wait for the broker's acknowledgement**, then reply
`{"partition":4,"offset":1234}`. The validation rules, the JSON and the partition each key
lands on are identical; the smoke test checks this.

Both services also have a fire-and-forget mode, which replies `{"accepted":true}` as soon as the
Kafka client has the record. Its results are in [Fire and forget](#fire-and-forget---mode-async)
below; everything before that section is the default wait-for-acknowledgement design.

| | Forkline | Spring MVC |
|---|---|---|
| Code | `forkline-svc/` (plan: decode → publish → encode) | `spring-svc/` (one `@RestController`) |
| Runtime | Rust 1.100 release build (thin LTO), Tokio, hyper | Spring Boot 4.2.0, Tomcat, OpenJDK 25 (LTS), default GC and heap |
| Kafka client | `rdkafka` 0.39 (librdkafka 2.12.1, Confluent's C client) | `kafka-clients` 4.4.0 via Spring Kafka (Apache's Java client) |
| Producer | `acks=all`, idempotent, `linger.ms` 5 or 0, no compression, murmur2 partitioner | the same |

Spring runs two ways: Tomcat's default 200 platform threads, and virtual threads
(`spring.threads.virtual.enabled=true`). One Tomcat setting is changed:
`max-keep-alive-requests=-1`. By default Tomcat closes a connection after 100 requests, and on a
single machine the reconnects run out of client ports. hyper has no such limit.

## Results (Apple M5, 10 cores, 16 GB; everything on one machine)

Full tables: [results/RESULTS.md](results/RESULTS.md). Raw data: `results/*.json`.

### Memory

| | Idle | Under load (1k–20k req/s) | 1,024 connections |
|---|---:|---:|---:|
| Forkline | **7 MB** | **18–22 MB** | **60 MB** |
| Spring MVC | 223 MB | 239–307 MB | 355 MB |
| Spring MVC + virtual threads | 227 MB | 220–321 MB | 1,544 MB |

The JVM uses the default maximum heap (a quarter of RAM, 4 GB here), so its RSS grows toward that.
A smaller `-Xmx` would cap it, at the cost of more GC work.

### CPU per request at the same load (`linger.ms=5`, the Kafka default)

| Offered load | Forkline | Spring MVC | Spring + VT |
|---:|---:|---:|---:|
| 1,000 req/s | **165 µs** (0.16 cores) | 229 µs (0.23) | 235 µs (0.23) |
| 5,000 req/s | **105 µs** (0.52) | 157 µs (0.78) | 148 µs (0.74) |
| 10,000 req/s | **82 µs** (0.81) | 143 µs (1.42) | 121 µs (1.21) |
| 20,000 req/s | **62 µs** (1.23) | 92 µs (1.83) | 85 µs (1.69) |

Forkline does the same work with roughly **0.55–0.7× the CPU**. These figures include the Kafka
client's own threads in each process.

### Latency at the same load

With `linger.ms=5`, latency is mostly the producer waiting to batch. At 1k–20k req/s every service
has a p50 of 3.5–6 ms and a p99 of 7–11 ms. At 1k req/s Spring's p50 is lower (3.5–3.9 ms against
5.9 ms) because of the clients: once any partition's batch is due, the Java client sends batches for
every partition on that broker, while librdkafka waits out each partition's own linger.

With `linger.ms=0` the frameworks show through. At 20k req/s:

| | p50 | p99 | p99.9 |
|---|---:|---:|---:|
| Forkline | **0.78 ms** | 7.5 ms | 12.1 ms |
| Spring MVC | 0.86 ms | 7.0 ms | 11.9 ms |
| Spring MVC + virtual threads | 0.87 ms | 7.6 ms | 12.0 ms |

The medians differ by under 0.1 ms. The tail comes from the broker round trip, and all three share it.

### Throughput (fixed connections, no rate limit)

| Connections | Forkline | Spring MVC | Spring + VT |
|---|---:|---:|---:|
| `linger.ms=5`, 64 | 9.1k | 9.3k | 8.4k |
| `linger.ms=5`, 256 | 35.6k | 34.4k | 34.7k |
| `linger.ms=5`, 1,024 | 104k @ 3.1 cores | 34k (thread pool full) | **107k** @ 5.6 cores |
| `linger.ms=0`, 64 | 93k @ 3.3 cores | 73k @ 4.5 cores | **97k** @ 4.8 cores |
| `linger.ms=0`, 256 | 100k @ 3.1 cores | 105k @ 5.3 cores | **125k** @ 5.5 cores |
| `linger.ms=0`, 1,024 | 102k @ 3.4 cores | **119k** @ 5.5 cores | 115k @ 5.9 cores (p99 81 ms) |

- With linger 5 and up to 256 connections, all three are capped by Kafka: connections ÷ ~7 ms.
- Spring on platform threads stops at about 34k: 200 Tomcat threads, each blocked about 6 ms on the
  broker acknowledgement.
- At `linger.ms=0`, Spring with virtual threads reaches the highest peak. Forkline stops near 100k
  without running out of CPU: its worker threads are about 70% idle, and the broker uses about 0.5
  cores. The cause is how the Kafka client packs requests (next section). Spreading records over
  two librdkafka producers (`KAFKA_PRODUCERS=2`, two connections) lifts Forkline to 115k at 4.1
  cores; four do no better, as the shared 10-core machine becomes the limit. The engine itself is
  not the ceiling: it serves 192k req/s on the echo plan.

### Why the Kafka clients differ: records per produce request

A producer sends its batches to the broker in produce requests. One request may carry batches for
many partitions, and the broker handles one request at a time per client connection. So records
per request decides how much each round trip moves.

The broker's request log for the same 10,000 records (2,000 req/s for 5 s, `linger.ms=5`):

| Client | Produce requests | Partitions per request |
|---|---:|---|
| librdkafka 2.12.1 (Forkline) | 3,646 | always 1 |
| Java client 4.4.0 (Spring) | 884 | 4–6 (mostly 5 or 6) |

The broker's JMX counters, measured across a whole run:

| Load | Forkline: records per request | Spring: records per request |
|---|---:|---:|
| 20k req/s, `linger.ms=5` | 19 (1,032 requests/s) | 125 (160 requests/s) |
| 256 connections, `linger.ms=5` | 43 | 200 |
| 20k req/s, `linger.ms=0` | 3.0 (6,750 requests/s) | 4.0 (5,050 requests/s) |
| 256 connections, `linger.ms=0` | 10 (10,200 requests/s) | 22 (5,060 requests/s) |

librdkafka puts one partition's batch in each produce request. When any partition's batch is
ready, the Java client sends the batches for every partition on that broker in a single request.
With one broker and six partitions, Forkline's connection makes 2–6× as many round trips for the
same records. At its ceiling it is doing about 10,000 requests a second on its single connection.
The same behaviour gives Spring its lower median latency at 1k req/s with `linger.ms=5`: pending
batches leave early alongside the first one that is due.

## Rerun on 3 brokers, 10 partitions, 3 replicas

`KAFKA_BROKERS=3 KAFKA_PARTITIONS=10 KAFKA_RF=3 KAFKA_JMX_PORT=9999 scripts/kafka.sh start`. All three
brokers run on the same machine, and the topic has `min.insync.replicas=2`. With `acks=all`, every
record is now copied to the other two brokers before the producer gets its acknowledgement. That
raises everyone's tail latency: p99 goes from 7–11 ms to 16–24 ms.

Records per produce request, counted at the brokers:

| Load | Forkline | Spring MVC | Ratio (1 broker) |
|---|---:|---:|---:|
| 20k req/s, `linger.ms=5` | 12.3 | 42.5 | 3.5× (was 6.4×) |
| 256 connections, `linger.ms=5` | 25.4 | 66.5 | 2.6× (was 4.7×) |
| 20k req/s, `linger.ms=0` | 1.7 | 1.7 | 1.0× (was 1.3×) |
| 256 connections, `linger.ms=0` | 5.1 | 11.1 | 2.2× (was 2.2×) |

With partitions spread over three brokers, the Java client finds fewer partitions to combine per
request, so the gap narrows. librdkafka still sends one partition per request, but over three
connections instead of one.

| | Forkline | Spring MVC | Spring + VT |
|---|---:|---:|---:|
| CPU per request, 20k req/s, `linger.ms=5` | **50 µs** | 86 µs | 74 µs |
| CPU per request, 20k req/s, `linger.ms=0` | 61 µs | 75 µs | **59 µs** |
| Peak, `linger.ms=5`, 1,024 connections | **101.7k** @ 3.2 cores | 29.6k | 101.2k @ 5.3 cores |
| Peak, `linger.ms=0`, 256 connections | 62.1k @ 2.5 cores | 70.5k @ 3.5 cores | **79.7k** @ 3.8 cores |
| Memory under load (1k–20k req/s) | **17–21 MB** | 197–297 MB | 180–292 MB |

- **With default batching (`linger.ms=5`), Forkline now matches Spring's peak.** It reaches the
  same ~101k as Spring with virtual threads on 60% of the CPU. It also keeps its CPU lead at every
  fixed rate: 0.6–0.8× Spring's CPU per request.
- **With batching off (`linger.ms=0`), Forkline still has the lower peak (62k against 70–80k),
  and its CPU lead disappears at 20k req/s.** librdkafka sends 1–2 records per request, so the
  client spends its time on requests rather than records. Replication also makes each request take
  longer at the broker. Three connections together completed only about 12,000 requests a second,
  against 10,000 on one unreplicated broker.
- **Latency medians are within about 1 ms of each other everywhere.** With `linger.ms=5` at
  1k req/s, Spring's lead is smaller than on one broker: 5.1 ms against 6.3 ms, where it was 3.9 ms
  against 5.9 ms.

With librdkafka, keep `linger.ms` at its default of 5 ms or higher; that is where it competes.

### Startup

Time from launch until the first request publishes successfully: Forkline 0.06–0.3 s, Spring 0.9–1.2 s.

## Fire and forget (`--mode async`)

Both services can reply before Kafka answers. They still validate first (400 for a bad query),
then hand the record to the client and reply `{"accepted":true}`. The client's delivery
callback counts the acknowledgement later: librdkafka's own callback for Forkline, and
`KafkaTemplate.send(...).whenComplete(...)` for Spring. `GET /stats` returns accepted, acked,
failed and pending counts. `PUBLISH_MODE=async` switches Forkline; the `async` profile switches
Spring.

The two clients get the same headroom in this mode: no backlog cap (librdkafka's queue limits at
their maximum; Spring's `buffer.memory` 6 GiB with `-Xmx8g`), a 300 s delivery timeout, and a
1 MB `batch.size`. The batch size matters here: the Java default is 16 KB against librdkafka's
1 MB, and once requests stop waiting, batches grow to that limit. The wait-mode settings are
unchanged.

After each scenario the runner waits until `/stats` shows nothing pending, timing the drain, and
checks that Kafka's offsets grew by exactly the acknowledged count. A fourth target,
`forkline-4p`, runs Forkline with four librdkafka producers (records spread by key), because a
single producer turned out to be Forkline's ceiling.

| `linger.ms=5` | Forkline | Forkline, 4 producers | Spring MVC | Spring + VT |
|---|---:|---:|---:|---:|
| CPU per request, 20k req/s, 1 broker | **52 µs** | 60 µs | 82 µs | 82 µs |
| CPU per request, 20k req/s, 3 brokers | **45 µs** | 48 µs | 69 µs | 67 µs |
| Peak, 1 broker | 118.6k @ 3.4 cores | **172.6k** @ 4.5 cores | 170.0k @ 5.5 cores | 169.8k @ 5.7 cores |
| Peak, 3 brokers | 109.4k @ 3.3 cores | 131.1k @ 3.8 cores | **161.5k** @ 5.2 cores | 156.9k @ 5.1 cores |
| CPU per request at peak | 28–30 µs | **26–29 µs** | 32 µs | 33–34 µs |
| Memory at 1k–20k req/s | **18–20 MB** | 18–22 MB | 272–408 MB | 276–332 MB |
| Memory at peak and overload | **20–48 MB** | 20–49 MB | 394–651 MB | 482–790 MB |
| Records per produce request at peak, 1 / 3 brokers | 104 / 60 | 39 / 19 | 1,076 / 352 | 987 / 300 |

- **No backlog formed.** Kafka acknowledged records as fast as any service accepted them, up to
  173k a second: the largest backlog seen was 20,636 records, and the longest drain 0.11 s. With
  1 MB batches the broker never fell behind. To measure draining, something has to slow Kafka down
  (a broker pause, a quota), which this run doesn't do.
- **Not waiting is what frees Spring MVC.** Its peak went from 30–34k (200 Tomcat threads, each
  blocked on an acknowledgement) to 161–170k. With virtual threads it was already at 101–107k and
  now reaches 157–170k.
- **A single librdkafka producer is Forkline's ceiling: about 110–120k req/s at 3.3 cores**, with
  most of the machine idle. Every request thread hands its record to the same producer. With
  four producers Forkline matches Spring's peak on one broker using less CPU per request. On three
  brokers it reaches 131k against Spring's 161k. The Java client's single producer has no such
  limit at these rates.
- **More producers cost more at low load.** Four producers send smaller batches (19–39 records
  per request against 60–104), so they use more CPU per request at 1k–10k req/s than one, and more
  broker CPU (1.9 cores at peak on three brokers, against 0.4 for Spring).
- **Memory stays 10–20× apart**, as in wait mode. Right after the overload scenario Spring's heap
  had 226–342 MB in use.
- **The reply itself is fast everywhere:** p50 0.15–0.21 ms at 1k req/s, against 3.5–6.3 ms when
  waiting for Kafka.

On 3 brokers at 1,024 connections, oha recorded connection errors on 0.01–0.04% of the 4-producer
target's requests. The service had accepted 26 and 7 of those requests, so their replies were lost
on the way back. Kafka still held exactly the accepted and acknowledged count: nothing was lost or
duplicated. Spring saw the same kind of errors once in the wait-mode run.

## Caveats

- One laptop: oha, the broker and the service share 10 cores (4 performance, 6 efficiency), with
  no CPU pinning on macOS. Saturation numbers vary by about ±10% between runs (compare a quick
  pass: Forkline 113k and Spring + VT 98k at 1,024 connections). CPU per request at a fixed rate
  is the most stable comparison.
- The two Kafka clients differ (librdkafka against the Java client), and that shows in batching
  latency and peak throughput (see "Why the Kafka clients differ"). That gap belongs to the client,
  not to the framework. With several brokers, librdkafka opens a connection to each, so the gap
  should narrow; one broker is the worst case for it.
- One run per configuration, 30 s per scenario after a 60 s warm-up (enough for the JIT).
- The JVM runs with default settings. Tuning heap, GC or Tomcat threads would shift Spring's
  numbers; so would `jemalloc` or a producer pool for Forkline. The async runs are the
  exception: Spring gets `-Xmx8g` so its 6 GiB client buffer fits.
- In async mode the 150k req/s overload scenario asks more than a single-producer Forkline
  accepts, so oha's latency correction reports multi-second latencies for it. Those measure the
  requests oha couldn't send on schedule, not time spent in the service.

## Reproduce

Needs Homebrew `kafka` and `maven`, `openjdk@25`, Rust, and `oha` (`cargo install oha`).

```sh
cd benchmarks/kafka-vs-spring
(cd forkline-svc && cargo build --release)
(cd spring-svc && JAVA_HOME=/opt/homebrew/opt/openjdk@25 mvn -q package -DskipTests)
scripts/kafka.sh start
scripts/bench.py --linger 5        # about 17 minutes; --quick for a 6-minute check
scripts/bench.py --linger 0
scripts/bench.py --mode async --targets forkline,forkline-4p,spring,spring-vt   # about 26 minutes
scripts/report.py results/run-*.json
scripts/kafka.sh stop
```

Every scenario checks that the number of records Kafka gained equals the number of 200 responses
(in async mode: the number the broker acknowledged).
