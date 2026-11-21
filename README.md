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

Phase 1 of 5 (foundations) is complete:

| Area | State |
| --- | --- |
| Request map (interned slots, `Arc` values, dirty bitset) | done |
| `interface!` data contracts | done |
| Plan format, graph checks, module/kind/type/config checks | done |
| Engine: straight chains, deadlines, cancellation, terminal node | done |
| Snapshot per request, atomic reload | done (triggers come in phase 4) |
| HTTP (axum) and gRPC (tonic, raw bytes) adapters | done |
| JSON or text logs with request spans | done |
| Fork, branch, multibranch, join, merge rules | phase 2 |
| Service registry, data-flow validation, test kit | phase 3 |
| Admin API, admission control, metrics, framed TCP | phase 4 |
| Fuzzing, load and soak tests | phase 5 |

Plans that fork, branch or join pass validation but the phase 1 engine refuses
to load them.

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

`cargo bench -p forkline-engine`. Phase 1 baseline on an Apple-silicon laptop,
release build, mean time per request through `Engine::call` with no-op modules:

| Benchmark | Time |
| --- | --- |
| 3-node chain | 0.28 µs |
| 7-node chain | 0.59 µs |
| 15-node chain | 1.19 µs |
| Boxed dynamic dispatch, per async node | about 6 ns over a static call |

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
