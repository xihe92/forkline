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

Phases 1 to 3 of 5 are complete:

| Area | State |
| --- | --- |
| Request map (interned slots, `Arc` values, dirty bitset) | done |
| `interface!` data contracts | done |
| Plan format, graph checks, module/kind/type/config checks | done |
| Engine: chains, forks, branches, multibranches, joins | done |
| Merge rules with `on_conflict = "drop"` or `"error"` | done |
| Deadlines, cancellation, panic isolation, detached forks | done |
| Snapshot per request, atomic reload | done (triggers come in phase 4) |
| HTTP (axum) and gRPC (tonic, raw bytes) adapters | done |
| JSON or text logs with request spans | done |
| Services from `services.toml`: HTTP and gRPC clients, timeout, retry budget, concurrency limit, circuit breaker | done |
| Data-flow validation (`in` keys written upstream on every path) | done |
| Subplans (`cx.execute_plan`) | done |
| Module test kit (`forkline-testkit`) | done |
| Admin API, admission control, metrics, framed TCP | phase 4 |
| Fuzzing, load and soak tests | phase 5 |

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
forkline-horoscope check --plans plans.toml --services services.toml   # every check
forkline-echo check --plans plans.toml   # full check with this binary's modules
forkline-plan check plans.toml           # graph rules only, no modules needed
```

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

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
```

The minimum supported Rust version is 1.88. Every crate forbids `unsafe` code,
and library code may not `unwrap`, `expect` or `panic`.

## Licence

Not yet chosen. The crates are marked `publish = false` until it is.
