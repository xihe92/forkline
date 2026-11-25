# Forkline

Forkline is a Rust plan engine for backend services. You write small modules,
declare which request attributes each one reads and writes, and wire them into
a graph in a TOML plan. Forkline runs that graph for every request, with the
same deadlines, error handling and tracing on every step.

Transport, concurrency and telemetry come from existing crates (tokio, tower,
axum, tonic, tracing). Forkline adds what they do not: config-defined
pipelines, typed data contracts checked at load time, fork/join with merge
rules, and plan reloads that never mix two configurations inside one request.

## Status

All five phases of the implementation plan are complete; this is release candidate 1.0.0-rc.1 (see [CHANGELOG.md](CHANGELOG.md)).

| Area | State |
| --- | --- |
| Request map (interned slots, `Arc` values, dirty bitset) | done |
| `interface!` data contracts | done |
| Plan format, graph checks, module/kind/type/config checks | done |
| Engine: chains, forks, branches, multibranches, joins | done |
| Merge rules with `on_conflict = "drop"` or `"error"` | done |
| Deadlines, cancellation, panic isolation, detached forks | done |
| Snapshot per request, atomic reload, rollback (last 5) | done |
| HTTP (axum) and gRPC (tonic, raw bytes) adapters | done |
| JSON or text logs with request spans | done |
| Services from `services.toml`: HTTP and gRPC clients, timeout, retry budget, concurrency limit, circuit breaker | done |
| Data-flow validation (`in` keys written upstream on every path) | done |
| Subplans (`cx.execute_plan`) | done |
| Module test kit (`forkline-testkit`) | done |
| Admin API: health, readiness, metrics, snapshot, reload, rollback | done |
| Reload triggers: admin API, SIGHUP, file watcher | done |
| Admission control (red/green), immediate refusal, stale-request shedding | done |
| Prometheus metrics for requests, nodes, services, reloads | done |
| Length-delimited TCP adapter with pipelining | done |
| Graceful drain with timeout; jemalloc feature | done |
| Fuzzing (plans, configs, TCP frames), load tests, soak script | done |
| OTLP trace export (`otlp` feature), tokio-console (`console` feature) | done |

## How a request runs

Nodes in a straight line run inline on the request's task. At a fork (any
non-branch node with two or more children) the paths run concurrently on that
task, or on separate tasks when the node has `spawn = true`. A branch picks one
labelled child; a multibranch picks any set of them.

A join's module is called once per arriving path, serially. The join continues
once no running path can still reach it, or as soon as its module returns
`Done`, which cancels the paths still on their way. It then merges:

1. keys the join module wrote keep the join's value;
2. keys no path changed carry through;
3. keys changed on one path take that path's value;
4. keys changed on several paths are conflicts: dropped, or a
   `merge_conflict` error, per the plan's `on_conflict` (default `error`).

The reply goes out as soon as the encode node runs. Paths that cannot reach
the encode node are detached: they finish under the request's deadline, their
failures are logged, and shutdown waits for them up to
`shutdown.drain_timeout_ms`.

A module error, a panic, a deadline or a merge conflict cancels the request's
other paths and runs the terminal node.

## Layout

| Crate | Contents |
| --- | --- |
| `forkline` | Facade: re-exports everything a module author needs |
| `forkline-core` | Request map, contracts, `Ctx`, errors, node traits, registry |
| `forkline-macros` | The `interface!` macro |
| `forkline-plan` | Plan config, validation, compilation, `forkline-plan check` |
| `forkline-engine` | Executor and snapshots |
| `forkline-services` | Service clients and tower layer stacks built from `services.toml` |
| `forkline-server` | HTTP and gRPC adapters, server config, logging, CLI |
| `forkline-testkit` | Run one module in a test with typed inputs and mock services |
| `examples/echo` | The smallest plan: decode, echo, encode |
| `examples/horoscope` | A fork, a branch and a join over three HTTP backends |

## Quick start

```sh
# A fake backend, then the example server.
cargo run --release -p forkline-horoscope --bin horoscope-backend &
cargo run --release -p forkline-horoscope -- serve --config examples/horoscope/server.toml \
  --plans examples/horoscope/plans.toml --services examples/horoscope/services.toml

curl -d '{"want":"horoscope","sign":"leo"}' http://127.0.0.1:8080/query
# {"want":"horoscope","time":"...","result":"leo: a good day to ship","summary":"..."}
curl -d '{"want":"stock","ticker":"ACME"}' http://127.0.0.1:8080/query
```

gRPC: `/forkline.v1.Plans/Query` on port 50051 takes and returns raw message
bytes, so any client that can send bytes works without generated code.

## Writing a module

```rust
use forkline::{Build, BuildError, Ctx, Module, NoConfig, NodeError};

forkline::interface! {
    pub struct GreetIo {
        in  name: String,
        out greeting: String,
    }
}

struct Greet;

impl Build for Greet {
    type Config = NoConfig;
    fn build(_: NoConfig) -> Result<Self, BuildError> { Ok(Greet) }
}

impl Module for Greet {
    type Io<'a> = GreetIo<'a>;
    async fn call(&self, _cx: &Ctx, mut io: GreetIo<'_>) -> Result<(), NodeError> {
        let name = io.name().cloned().unwrap_or_default();
        io.set_greeting(format!("hello, {name}"));
        Ok(())
    }
}
```

Register modules by name and hand the registry to the shared CLI:

```rust
let registry = Registry::builder()
    .decode::<BodyDecoder>("BodyDecoder")
    .module::<Greet>("Greet")
    .encode::<ReplyEncoder>("ReplyEncoder")
    .terminal::<ErrorReply>("ErrorReply")
    .build()?;
forkline_server::cli::main_with(registry)
```

Node kinds: `Decode` (root), `Module` (async step), `Branch`, `MultiBranch`,
`Join`, `Encode` (builds the reply) and `Terminal` (builds the reply when the
plan fails). Any non-branch node with two or more children is a fork.

Contract keys are `in` (read; must be written upstream on every path),
`opt` (read if present), `out` (written), `inout` (both) or `unset`
(removed). Plans may rename a module's keys per node.

## Plans

```toml
[plans.echo]
root = "decode"
terminal = "error"
time_budget_ms = 100
links = ["decode -> echo", "echo -> encode"]

[plans.echo.nodes]
decode = { module = "BodyDecoder" }
echo   = { module = "Echo", config = { prefix = "echo: " } }
encode = { module = "ReplyEncoder" }
error  = { module = "ErrorReply" }
```

Every problem in a plan is reported at once, and a plan with any problem is
rejected whole:

```sh
forkline-horoscope check --config server.toml --plans plans.toml --services services.toml   # every check
forkline-echo check --plans plans.toml   # full check with this binary's modules
forkline-plan check plans.toml           # graph rules only, no modules needed
```

Plans are data, so `cargo build` doesn't read them. To catch a broken plan
before it reaches a server, add a test that runs the same full check, with the
`App` built exactly as `main` builds it; `cargo test` then fails on any plan
problem `serve` would reject:

```rust
// tests/plans.rs
use std::path::Path;
use forkline_server::cli::App;

#[test]
fn shipped_plans_pass_every_check() {
    App::new(my_service::registry().unwrap()).assert_files_valid(
        Some(Path::new("server.toml")),     // also checks pools and the plans listeners serve
        "plans.toml",
        Some(Path::new("services.toml")),
    );
}
```

Backends need not be running: clients connect lazily. Both examples have this
test.

## Services

Modules reach backends only through named services. Each one is built from
`services.toml` and wrapped in tower layers:

```toml
[services.horoscope]
kind = "http"                         # or "grpc", or a kind registered in code
url = "http://horoscope.internal:8080/"
timeout_ms = 40                       # per attempt; the request deadline also applies
retries = 1                           # within a retry budget (20% of requests by default)
concurrency_limit = 256
load_shed = true                      # at the limit, fail at once instead of queueing
breaker = { failures = 20, open_ms = 1000 }
```

A module resolves its clients while it is built, so a missing or mistyped
service rejects the plan at load time rather than on the first request:

```rust
impl Build for HoroscopeLookup {
    type Config = LookupConfig;
    fn build_with(config: LookupConfig, cx: &BuildContext<'_>) -> Result<Self, BuildError> {
        Ok(Self { client: cx.service::<HttpClient>(&config.service)? })
    }
}
```

HTTPS needs the `tls` feature of `forkline-services`.

## Blocking work and thread pools

Modules run on Tokio's worker threads (one per core by default), which every
request shares in short turns. Work that waits should use an async API and
`.await`, which frees the thread. Synchronous work that takes more than about a
millisecond, or a synchronous call that waits (a non-async database driver,
`std::fs`), must leave the worker threads, or every request queued behind it
waits too. Two ways:

* `cx.blocking(move || work())`: Tokio's shared blocking pool, no setup.
* A named pool from `server.toml`, for isolation and sizing: a fixed number of
  threads and a bounded queue, created when the server starts.

```toml
[pools.scoring]
threads = 4      # default: one per core
queue = 1000     # jobs that may wait; more are refused (default 1024)
```

```rust
impl Build for Rank {
    type Config = RankConfig;
    fn build_with(config: RankConfig, cx: &BuildContext<'_>) -> Result<Self, BuildError> {
        Ok(Self { scoring: cx.pool("scoring")? })   // a missing pool rejects the plan at load
    }
}

impl Module for Rank {
    type Io<'a> = RankIo<'a>;
    async fn call(&self, cx: &Ctx, mut io: RankIo<'_>) -> Result<(), NodeError> {
        let candidates = io.candidates().cloned().unwrap_or_default();
        let scores = self.scoring.run(cx, move || score(&candidates)).await?;
        io.set_scores(scores);
        Ok(())
    }
}
```

`Pool::run` waits without holding a worker thread. A full queue fails at once
with `Overloaded` (HTTP 503, gRPC `RESOURCE_EXHAUSTED`, TCP status 4). The wait
ends at the request's deadline or on cancellation; a job already running can't
be stopped and finishes, while one still queued is skipped. A panic fails the
job, not the thread. `/metrics` has `forkline_pool_threads`, `_queued`, `_busy`,
`_jobs_total{result="done|refused|panicked|skipped"}` and `_wait_seconds`.

Pools live in `server.toml`, so changing one needs a restart; at shutdown each
pool finishes its queue within `drain_timeout_ms`. Run `check` with
`--config server.toml` (or pass it to `assert_files_valid`) so plans are
checked against the configured pools and listeners. In a module test, give
the harness a registry holding the pool:
`ServiceRegistry::new().with_pool("scoring", Pool::new("scoring", 2, 16)?)`.

### Finding a module that blocks

The engine times every poll of every node. One that holds its worker thread
longer than `slow_poll_ms` (default 10 ms; 0 switches it off) adds to
`forkline_node_slow_polls_total{plan,node}` and logs a warning naming the plan
and node, at most once per node every 10 s. Waiting with `.await` never counts,
however long; a blocking call or a long computation does.

```toml
[runtime]
slow_poll_ms = 10
```

The timing reuses the clock reads the node-duration metric already makes, so
it costs nothing measurable. In a module's own test,
`Harness::max_poll(Duration::from_millis(10))` fails the run with
`TestError::Blocked` instead. A test suite can set
`FORKLINE_PANIC_ON_SLOW_POLL_MS`: any longer poll then panics, naming the
node. Forkline's own tests run with 50 ms (`.cargo/config.toml`).

Built with `RUSTFLAGS="--cfg tokio_unstable"`, the server's runtime also times
every task poll, not only nodes, and counts slow ones in
`forkline_task_slow_polls_total`, logging where the task was spawned. CI runs
`scripts/blocking-check.sh` this way nightly.

## Testing modules

```rust
let services = ServiceRegistry::new().with("horoscope", HttpClient::from_service("horoscope", mock));
let out = Harness::<HoroscopeLookup>::from_config("service = \"horoscope\"", services)?
    .inputs(inputs! { sign: String::from("leo") })
    .run_module()
    .await?;
assert_eq!(out.get::<String>("svc_response").map(String::as_str), Some("bold"));
```

Joins run over a fork base and a list of paths, with the engine's merge rules.

## Operating it

```toml
# server.toml
[admin]
addr = "127.0.0.1:6789"                # keep it private; also IP-allowlisted
allow = ["127.0.0.1/32", "::1/128"]

[admission]
red = 800                              # refuse new requests at 800 in flight...
green = 500                            # ...until back down to 500
max_queue_ms = 50                      # shed requests that waited longer to start

[reload]
history = 5                            # snapshots kept for rollback
watch_interval_ms = 1000               # reload when plans.toml or services.toml change

[shutdown]
drain_timeout_ms = 10000
```

| Endpoint (admin port) | Does |
| --- | --- |
| `GET /healthz` | 200 while the process runs |
| `GET /readyz` | 200, or 503 once shutdown has started |
| `GET /metrics` | Prometheus metrics (OpenMetrics text, official `prometheus-client`) |
| `GET /admin/snapshot` | Version, config hash, plans, services, rollback history |
| `POST /admin/reload` | Rebuild from the files and swap; 400 with every problem if invalid |
| `POST /admin/rollback` | Swap back to the previous snapshot |

`SIGHUP` also reloads. A failed reload leaves the running snapshot in place.
A reload (or rollback) that would remove a plan a listener serves is rejected,
so a route can never point at a missing plan.

HTTP request bodies are limited to `max_body_bytes` per listener (default
2 MiB; larger bodies get 413), TCP frames to `max_frame_bytes`.

Slow and idle clients: every listener closes a connection with no request in
flight for `idle_timeout_ms`; on TCP, a client that trickles a frame one byte
at a time counts as idle, and on gRPC so does one that never sends the HTTP/2
preface. HTTP listeners also close a connection whose client takes longer than
`header_read_timeout_ms` to send a request's headers (HTTP/1). All default to
60 s, Spring MVC's (Tomcat's) connection timeout:

```toml
[[http]]
addr = "0.0.0.0:8080"
routes = { "/echo" = "echo" }
header_read_timeout_ms = 60000
idle_timeout_ms = 60000

[[grpc]]
addr = "0.0.0.0:50051"
methods = { Echo = "echo" }
idle_timeout_ms = 60000

[[tcp]]
addr = "0.0.0.0:7000"
plan = "echo"
idle_timeout_ms = 60000
```

A connection asked to close (idle, or at shutdown) that has not closed a
second later with nothing in flight is dropped.

A failure after the request's deadline has passed is reported as a deadline
(HTTP 504, gRPC `DEADLINE_EXCEEDED`, TCP status 3), even when the module saw it
as its own error, for example a service call cut off by the same deadline.
Requests refused by admission control get HTTP 503 (with `Retry-After`),
gRPC `RESOURCE_EXHAUSTED` or TCP status 4 at once, instead of queueing.

On `SIGTERM` readiness turns false, listeners stop accepting, in-flight
requests get up to `drain_timeout_ms`, the rest are answered "shutting down",
detached paths get what time is left, and the admin listener stops last.

Metrics: `forkline_requests_total{plan,outcome}`,
`forkline_request_duration_seconds{plan}`,
`forkline_node_duration_seconds{plan,node}`,
`forkline_node_errors_total{plan,node,kind}`,
`forkline_merge_conflicts_total{plan,node,key}`,
`forkline_inflight_requests{listener}`,
`forkline_admission_refused_total{listener}`,
`forkline_service_call_duration_seconds{service,outcome}`,
`forkline_reloads_total{result}`, `forkline_snapshot_version`.

Inbound `traceparent` and `tracestate` headers are forwarded on every
outbound HTTP and gRPC call. Outbound gRPC calls also carry `grpc-timeout`
set to the time left before the request's deadline, measured again for each
retry, so the backend stops work the caller has given up on.

## Benchmarks

Engine overhead only: modules do no work and there is no network. Measured on
an Apple M5 laptop, release build.

`cargo bench -p forkline-engine --bench latency` (200,000 sequential requests):

| Plan | p50 | p99 | p99.9 |
| --- | --- | --- | --- |
| 7 nodes, straight chain | 0.75 µs | 0.96 µs | 1.08 µs |
| 7 nodes, one fork and join | 1.67 µs | 2.12 µs | 2.25 µs |
| Fan-out to 8 paths and join | 3.38 µs | 4.21 µs | 4.92 µs |

The proposed target is under 50 µs at p99 for the 7-node fork/join plan.
`cargo bench -p forkline-engine --bench engine` covers map operations, plan
shapes and dispatch cost (boxing each async node adds about 6 ns).

Operational gate tests (`cargo test -p forkline-server --test ops -- --nocapture`),
over real sockets on the same laptop:

| Scenario | Result |
| --- | --- |
| 32 clients for 2 s while reloading every 10 ms | about 165 reloads, 19,000 requests, 0 failed |
| 2x the admission limit at once (80 requests, red = 40, 100 ms work, 500 ms budget) | 40 admitted, p99 104 ms; 40 refused, p99 about 1 ms client-side including the HTTP round trip |

## Load and soak

Release builds on an Apple M5 laptop, client ([oha](https://github.com/hatoo/oha),
64 connections), server and fake backend on the same machine:

| Plan | Throughput | p50 | p99 | p99.9 | Errors |
| --- | --- | --- | --- | --- | --- |
| echo (3 nodes) over HTTP | 191,000 req/s | 0.31 ms | 1.02 ms | 2.81 ms | 0 |
| horoscope (fork, branch, join, 2 backend HTTP calls per request) | 58,500 req/s | 1.06 ms | 1.97 ms | 3.15 ms | 0 |

`scripts/soak.sh [seconds] [rate]` runs the horoscope example at a steady
rate and samples the server's memory, threads and file descriptors (Linux or
macOS). Comparing the second quarter of the run (after warm-up) with the last,
it fails if memory grows more than 10% and more than 8 MB, or the lowest
count of file descriptors more than 10% + 16 or of threads more than 25% + 8
(pools make the peaks swing; a leak raises the floor); if more than 0.1% of requests
fail; or if the server panics or stops. The nightly CI run includes it, for
30 minutes at 5,000 req/s, whenever `main` changed in more than docs since the
last successful nightly run (as does the benchmark gate). A manual run can
soak for up to 5.5 hours: `gh workflow run nightly.yml -f force=true -f soak_minutes=330`.
A 10-minute run at 5,000 req/s served 3,000,000 requests with no errors
(p99 1.04 ms) and memory flat at 24 MB (+0.2%). A 24-hour run is still to do
before 1.0.0.

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace              # or: cargo test --workspace
cargo test --workspace --doc
cargo deny check
cd fuzz && cargo +nightly fuzz run plans   # see fuzz/README.md
```

The minimum supported Rust version is 1.88, and CI runs on Linux. Every crate
forbids `unsafe` code, and library code may not `unwrap`, `expect` or `panic`.
CI runs the tests with [cargo-nextest](https://nexte.st) (one process per
test; a test still running after 3 minutes is killed and named; no retries,
see `.config/nextest.toml`), builds every feature and runs a short fuzz pass.
A nightly job benchmarks `main` against the commit the previous nightly run
tested, in the same job, and fails on a regression over 10%
(`scripts/bench-check.py`) or a p99 over 50 µs for the 7-node fork/join plan
(`cargo bench -p forkline-engine --bench latency`). Nightly also: every fuzz
target for 30 minutes (`fuzz/README.md`), `scripts/blocking-check.sh`, and
HTTP/2 conformance with [h2spec](https://github.com/summerwind/h2spec) on the
HTTP (h2c) and gRPC listeners (`scripts/h2spec-check.sh`; expected failures,
each with its reason, are in `scripts/h2spec-allow.txt`).

## Licence

Licensed under either of

* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <https://www.apache.org/licenses/LICENSE-2.0>)
* MIT licence ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
licence, shall be dual licensed as above, without any additional terms or
conditions.
