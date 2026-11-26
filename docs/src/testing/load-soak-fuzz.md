# Load, soak and fuzzing

Forkline's own robustness tests are scripts in the repository, and you can
point them at your service. This page describes what each one does and how to
read its results.

> **If you know Spring:** these are the tests a Spring team runs with Gatling
> or JMeter (load), a long JMeter run with a heap dump at the end (soak) and
> Jazzer (fuzzing). Here the load generator is oha, and the soak watches the
> server's memory, threads and file descriptors itself.

## Load

Use any HTTP load generator. Forkline's numbers come from
[oha](https://github.com/hatoo/oha):

```sh
oha -z 30s -c 64 -m POST -d '{"want":"horoscope","sign":"leo"}' http://127.0.0.1:8080/query
```

Watch the server's `/metrics` during the run: `forkline_inflight_requests`
shows the concurrency the server actually sees, and
`forkline_node_duration_seconds` shows which node the time goes to.

## Soak

`scripts/soak.sh [seconds] [rate] [out_dir]` runs the horoscope example at a
steady rate (64 connections) and samples the server's memory, threads and
open file descriptors. It compares the second quarter of the run, after
warm-up, with the last quarter, and fails if:

- average memory grows more than 10% and more than 8 MB;
- the lowest count of open file descriptors grows more than 10% + 16, or of
  threads more than 25% + 8 (connection pools make the peaks swing, but a
  leak never gives its descriptor back, so it raises the lowest count);
- with jemalloc, the lowest count of allocated bytes grows more than 10% and
  more than 2 MB;
- more than 0.1% of requests fail or get a non-2xx answer;
- the server panics or stops.

```sh
cargo build --release -p forkline-horoscope --bins --features jemalloc
scripts/soak.sh 86400 5000 target/soak     # 24 hours at 5,000 requests a second
```

It writes `samples.csv`, `summary.txt`, the server's log and its final
metrics to the output directory. With jemalloc, the samples include allocated
and resident bytes, which separate real growth from memory the allocator keeps
for reuse (see [Runtime and memory](../server/runtime.md)).

## Fuzzing

The `fuzz/` directory has five cargo-fuzz targets. Each feeds arbitrary input
to one part of Forkline and checks it never panics and always ends cleanly:

| Target | Input |
| --- | --- |
| `plans` | Any text as `plans.toml` |
| `config` | Any text as `services.toml` or `server.toml` |
| `tcp_frames` | Any byte stream into the TCP listener |
| `http_request` | Any bytes into an HTTP connection (HTTP/1.1, or h2c after the preface) |
| `grpc_frames` | Any HTTP/2 frames into a gRPC connection |

```sh
cd fuzz && cargo +nightly fuzz run plans corpus/plans -- -max_total_time=300
```

## HTTP/2 conformance

`scripts/h2spec-check.sh` runs [h2spec](https://github.com/summerwind/h2spec)
against the HTTP listener (over h2c) and the gRPC listener. The few expected
failures are listed, each with its reason, in `scripts/h2spec-allow.txt`.

## What runs in CI

| When | What |
| --- | --- |
| Every push and pull request | Lint, tests (with a 50 ms blocking detector on every node), line coverage of at least 88%, the Rust 1.88 build, dependency audit, a 60-second run of every fuzz target, this book |
| Nightly, when `main` changed | Benchmarks against the previous night (fail on a 10% regression or a p99 over 50 µs), a 30-minute soak on jemalloc |
| Nightly, always | Every fuzz target for 30 minutes, HTTP/2 conformance, a load test built with `tokio_unstable` that fails on any slow task poll |

## See also

- [Performance](performance.md).
- [Admission control](../server/admission.md): what to expect at overload.
