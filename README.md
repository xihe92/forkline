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

Phases 1 and 2 of 5 are complete:

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
| Service registry, data-flow validation, test kit | phase 3 |
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
| `forkline-server` | HTTP and gRPC adapters, server config, logging, CLI |
| `examples/echo` | A runnable echo service |

## Quick start

```sh
cargo run --release -p forkline-echo -- serve \
  --config examples/echo/server.toml --plans examples/echo/plans.toml

curl -d 'hello' http://127.0.0.1:8080/echo      # echo: hello
curl -X POST http://127.0.0.1:8080/echo         # 400 empty body
```

gRPC: `/forkline.v1.Plans/Echo` on port 50051 takes and returns raw message
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
forkline-echo check --plans plans.toml   # full check with this binary's modules
forkline-plan check plans.toml           # graph rules only, no modules needed
```

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
