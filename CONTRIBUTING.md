# Contributing

## Layout

| Path | Contents |
| --- | --- |
| `crates/forkline` | Facade: re-exports everything a module author needs |
| `crates/forkline-core` | Request map, contracts, `Ctx`, errors, node traits, registry, thread pools |
| `crates/forkline-macros` | The `interface!` macro |
| `crates/forkline-plan` | Plan config, validation, compilation, `forkline-plan check` |
| `crates/forkline-engine` | Executor and snapshots |
| `crates/forkline-services` | Service clients and tower layer stacks built from `services.toml` |
| `crates/forkline-server` | HTTP, gRPC and TCP listeners, server config, telemetry, CLI |
| `crates/forkline-testkit` | Run one module in a test with typed inputs and mock services |
| `examples/echo` | The smallest plan: decode, echo, encode |
| `examples/horoscope` | A fork, a branch and a join over three HTTP backends |
| `docs/` | The reference book (mdBook) |
| `docs/snippets` | The book's code samples, compiled and tested |
| `fuzz/` | cargo-fuzz targets; see `fuzz/README.md` |
| `scripts/` | Soak, blocking check, h2spec, benchmark gate, book link check |
| `benchmarks/kafka-vs-spring` | The Spring Boot comparison; run by hand |

## Checks

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
see `.config/nextest.toml`), requires 88% line coverage, builds every feature,
checks the Rust 1.88 build, runs a short fuzz pass and builds the book.
`FORKLINE_PANIC_ON_SLOW_POLL_MS=50` (`.cargo/config.toml`) makes any test fail
whose node blocks a worker thread for 50 ms.

A nightly run:

- benchmarks `main` against the commit the previous successful nightly run
  tested, in the same job, and fails on a regression over 10%
  (`scripts/bench-check.py`) or a p99 over 50 µs for the 7-node fork/join
  plan; this and the soak run only when `main` changed in more than docs;
- soaks the horoscope example for 30 minutes at 5,000 requests a second on
  jemalloc (`scripts/soak.sh`); a manual run can soak up to 5.5 hours with
  `gh workflow run nightly.yml -f force=true -f soak_minutes=330`, and choose
  `-f allocator=system`;
- fuzzes every target for 30 minutes;
- checks HTTP/2 conformance with [h2spec](https://github.com/summerwind/h2spec)
  (`scripts/h2spec-check.sh`; expected failures, each with its reason, are in
  `scripts/h2spec-allow.txt`);
- runs `scripts/blocking-check.sh`, a load test built with
  `--cfg tokio_unstable` that fails on any slow task poll.

## The book

```sh
cargo install mdbook --locked
mdbook serve docs                          # http://localhost:3000
mdbook build docs && python3 scripts/check-book-links.py docs/book
```

Code in the book is included from `docs/snippets` and the examples by anchor
(`{{#include ../../snippets/src/lib.rs:greet}}`), so it is compiled and tested
with the workspace. `docs/snippets/tests/reference.rs` fails when a config
key or a metric is missing from the appendix: add a new one to the reference
tables in `docs/src/appendix/`.
