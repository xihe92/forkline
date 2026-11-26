# Performance

Forkline is fast at what it controls, the engine, and frugal with memory and
CPU. Real request latency is set by the network and the backends, so the
numbers that matter for most services are cost per request and behaviour
under load. Every number on this page names its machine; treat them as
comparisons, not guarantees.

> **If you know Spring:** like the WebFlux reference, this page doesn't claim
> that Forkline makes an application faster. The benefit is that a request
> costs less memory and CPU, so a server handles more load per core with
> steadier tails.

## The engine

`cargo bench -p forkline-engine --bench latency` runs 200,000 requests one
after another through plans whose modules do no work, with no network, on an
Apple M5 laptop:

| Plan | p50 | p99 | p99.9 |
| --- | --- | --- | --- |
| 7 nodes, straight chain | 0.75 µs | 0.96 µs | 1.08 µs |
| 7 nodes, one fork and join | 1.67 µs | 2.12 µs | 2.25 µs |
| Fan-out to 8 paths and join | 3.38 µs | 4.21 µs | 4.92 µs |

The nightly CI run fails if the 7-node fork-and-join plan's p99 goes over
50 µs, or if any engine benchmark regresses by more than 10%.

## Over HTTP

Release builds on the same laptop, with oha (64 connections), the server and a
fake backend on one machine:

| Plan | Throughput | p50 | p99 | p99.9 | Errors |
| --- | --- | --- | --- | --- | --- |
| echo (3 nodes) | 191,000 req/s | 0.31 ms | 1.02 ms | 2.81 ms | 0 |
| horoscope (fork, branch, join, 2 backend calls per request) | 58,500 req/s | 1.06 ms | 1.97 ms | 3.15 ms | 0 |

## Against Spring Boot

The repository's `benchmarks/kafka-vs-spring` compares Forkline with Spring
Boot 4.2 on JDK 25, Spring MVC on platform threads and on virtual threads. Both
services do the same work: parse an HTTP request's parameters, publish them to
Kafka, and answer once Kafka acknowledges. They ran on an Apple M5 laptop (10
cores, 16 GB) against a local Kafka.

With Kafka's default batching (`linger.ms=5`) on one broker:

| | Forkline | Spring MVC | Spring MVC, virtual threads |
| --- | --- | --- | --- |
| Memory when idle | 7 MB | 223 MB | 227 MB |
| Memory at 1,000 to 20,000 req/s | 18 to 22 MB | 239 to 307 MB | 220 to 321 MB |
| CPU per request at 20,000 req/s | 62 µs | 92 µs | 85 µs |
| Median latency at 20,000 req/s | 4.2 ms | 4.7 ms | 5.7 ms |
| Peak at 1,024 connections | 104,000 req/s on 3.1 cores | 34,000 req/s (thread pool full) | 107,000 req/s on 5.6 cores |
| Startup to first request | 0.06 to 0.3 s | 0.9 to 1.2 s | 0.9 to 1.2 s |

Where Forkline did worse:

- **With Kafka batching off (`linger.ms=0`)**, Spring with virtual threads
  reached a higher peak (80,000 against 62,000 requests a second on three
  brokers), and Forkline's CPU advantage disappeared at 20,000 req/s. The Kafka
  client Forkline's benchmark uses (librdkafka) sends fewer records per request
  to the broker than the Java client does in that mode.
- **Replying before Kafka acknowledges**, a single librdkafka producer capped
  Forkline at about 110,000 to 120,000 requests a second while Spring reached
  160,000 to 170,000; four producers brought Forkline level on one broker.

Medians stayed within about 1 ms of each other in every scenario, because
Kafka's acknowledgement dominates them. The full method, every scenario and
the raw results are in `benchmarks/kafka-vs-spring/README.md`.

## Under overload and over time

- Admission control at twice its limit admitted half the requests (p99 104 ms
  for 100 ms of work) and refused the other half in about 1 ms each.
- 32 clients sending for 2 seconds while the plans reloaded every 10 ms: about
  165 reloads, 19,000 requests, none failed.
- A 5.5-hour soak at 5,000 requests a second on a GitHub runner: 99 million
  requests, p99 0.70 ms, threads and file descriptors flat, memory 14 MB to
  17 MB in one step after a pause of the runner, with no growth after it.

## See also

- [Overview](../core/overview.md): when Forkline fits.
- [Load, soak and fuzzing](load-soak-fuzz.md): run these tests yourself.
