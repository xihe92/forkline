# Forkline

Forkline is a Rust plan engine for backend services. You write small modules,
declare which request attributes each one reads and writes, and wire them into
a graph in a TOML plan. Forkline runs that graph for every request, over HTTP,
gRPC or TCP, with the same deadlines, error handling, metrics and tracing on
every step.

Transport, concurrency and telemetry come from existing crates (Tokio, tower,
hyper, axum, tonic, tracing). Forkline adds what they don't: config-defined
pipelines, typed data contracts checked at load time, fork/join with merge
rules, and plan reloads that never mix two configurations inside one request.

```toml
[plans.quote]
root = "decode"
terminal = "error"
time_budget_ms = 100
links = [
  "decode -> route",
  "route -> standard : standard", "route -> express : express",
  "standard -> merge", "express -> merge",
  "merge -> encode",
]
```

## Documentation

The reference manual is an mdBook in [`docs/`](docs/src/introduction.md),
modelled on the Spring WebFlux reference: concepts, every config key, metric
and error kind, and a page for readers coming from Spring. Read it locally
with `mdbook serve docs`. Its code samples are compiled and tested
(`docs/snippets`).

## Features

- Plans with forks, branches, multibranches, joins, merge rules and subplans
- Contracts (`interface!`) checked when a plan loads: types, and keys written
  upstream on every path
- Deadlines, cancellation, panic isolation, detached paths
- HTTP (HTTP/1.1 and h2c), gRPC (raw bytes, no generated code) and
  length-delimited TCP listeners
- Services from `services.toml`: HTTP and gRPC clients with per-attempt
  timeouts, a retry budget, concurrency limits and a circuit breaker
- Hot reload (admin API, `SIGHUP`, file watcher) with rollback
- Admission control, stale-request shedding, graceful drain
- Named thread pools for blocking work, and a detector for modules that block
- Prometheus metrics, JSON logs, OTLP traces, optional jemalloc statistics
- A test kit for modules, and plan checks that run from `cargo test`

Status: release candidate 1.0.0-rc.1; see [CHANGELOG.md](CHANGELOG.md).

## Quick start

```sh
cargo run -p forkline-echo -- serve \
  --config examples/echo/server.toml --plans examples/echo/plans.toml

curl -d 'hi' http://127.0.0.1:8080/echo
# echo: hi
```

The [Quick start](docs/src/quick-start.md) continues with a live plan change
and the horoscope example, which forks, branches and joins over three backend
calls.

## Requirements

Rust 1.88 or newer. Supported on Linux; builds and runs on macOS for
development.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for the crate layout, the checks CI
runs, and how to build the book.

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
