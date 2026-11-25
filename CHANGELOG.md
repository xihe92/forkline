# Changelog

## 1.0.0-rc.1 (2026-11-20)

First release candidate. Everything in the implementation plan is built;
see "Before 1.0.0" for what remains.

### Engine
- Plans as TOML graphs: decode, module, branch, multibranch, fork (implicit),
  join, encode and terminal nodes.
- Load-time validation that reports every problem at once: graph rules,
  module kinds by position, key types across modules, renames, module config,
  services, and data flow (`in` keys written upstream on every path).
- Scheduler: straight runs inline on the request's task; forks concurrent on
  that task or spawned (`spawn = true`); joins fire once no live path can
  reach them, or early on `Done`, which cancels the remaining paths.
- Merge rules relative to the join's immediate dominator, with nested-fork
  bookkeeping; `on_conflict = "error"` (default) or `"drop"`.
- Deadlines (plan budget or a shorter inbound deadline), request-wide
  cancellation, panic isolation, detached paths, subplans with a nesting limit.
- Snapshots pinned per request; atomic reload; rollback over the last N
  snapshots; stale-request shedding.

### Services
- `services.toml` with built-in `http` (reqwest) and `grpc` (tonic, raw
  bytes) kinds plus kinds registered in code.
- Layer stack: per-attempt timeout, budgeted retry, circuit breaker,
  concurrency limit with optional shedding. Optional `tls` feature.
- Services resolved while modules are built, so a missing or mistyped
  service rejects the plan at load.
- W3C `traceparent` / `tracestate` forwarded on outbound calls.

### Server
- HTTP (axum), gRPC (tonic, raw bytes, `grpc-timeout` honoured) and
  length-delimited TCP (ordered pipelining) adapters.
- Admin listener with IP allowlist: health, readiness, Prometheus metrics,
  snapshot, reload, rollback. Reload also on SIGHUP and file change.
- Red/green admission control with immediate refusal.
- Graceful drain: readiness flip, drain timeout, then cut-off.
- Features: `otlp` (trace export, inbound trace context), `console`
  (tokio-console), `jemalloc`.

### Tooling
- `forkline-testkit` to run one module with typed inputs and mock services.
- Property tests (merge rules, scheduling invariants), fuzz targets (plans,
  configs, TCP frames), criterion benchmarks, a latency run, a soak script.
- CI: fmt, clippy `-D warnings`, tests, MSRV 1.88, cargo-deny, feature
  builds, a 10% benchmark-regression gate on pull requests, a short fuzz run.

### Fixed during hardening
- The first metrics exporter buffered every histogram sample until a scrape,
  so memory grew with traffic (found by the soak test).

### Dependency review
- Metrics now use the official Prometheus client (`prometheus-client`,
  maintained by the Prometheus project) instead of `metrics` and
  `metrics-exporter-prometheus`. `/metrics` serves OpenMetrics text; metric
  names are unchanged.
- The admin allowlist parses networks with the standard library instead of
  `ipnet`.
- The `mimalloc` feature is removed; `jemalloc` (TiKV) remains.

### After rc.1
- HTTP routes accept GET as well as POST; the raw query string is in the new
  `RequestEnv::query` field (`None` for gRPC, TCP and in-process calls).
- Outbound gRPC calls send `grpc-timeout` with the time left before the
  request's deadline, computed again on each retry. `GrpcRequest` has a new
  `deadline` field, so code that builds one must set it.
- `benchmarks/kafka-vs-spring`: the same Kafka-publishing GET service in
  Forkline and Spring MVC, with a runner that compares CPU, memory, latency
  and throughput, both waiting for Kafka's acknowledgement and fire and
  forget (`--mode async`, which also times the backlog's drain). A manual
  run, before each release; CI doesn't build it.
- `App::check_files` runs every check `serve` runs at startup without
  serving, inside or outside a Tokio runtime, and `App::assert_files_valid`
  turns it into a test assertion, so a broken plan fails `cargo test`. The
  `check` command now uses it; both examples test their shipped plans this way.
- Named thread pools: `[pools.<name>]` in `server.toml` (`threads`, default
  one per core; `queue`, default 1024) creates a fixed pool with a bounded
  queue at startup. Modules get one with `cx.pool(name)` while they are built,
  so a missing pool rejects the plan at load, and run work with `Pool::run`,
  which waits without holding a worker thread, stops waiting at the deadline
  or on cancellation, skips jobs whose request gave up while queued, survives
  panics, and refuses at once when the queue is full. Pools drain at shutdown
  and report `forkline_pool_*` metrics.
- HTTP listeners take `header_read_timeout_ms` (HTTP/1: time to send a
  request's headers, counted from the start of the connection or the end of
  the previous response) and `idle_timeout_ms` (no request in flight), both
  60 s by default as in Spring MVC on Tomcat. Before, neither existed:
  `axum::serve` configures no timer, so hyper's 30 s header timeout was off,
  and a client could hold a connection forever by sending nothing. The HTTP
  and admin listeners now run on Forkline's own accept loop (hyper-util),
  which also closes every connection when the drain cut-off aborts it.
- gRPC and TCP listeners take `idle_timeout_ms` (default 60 s) too. Neither
  closed idle connections before: tonic's server has no idle timeout, and a
  gRPC client that never sent the HTTP/2 preface, or a TCP client that sent
  nothing or trickled a frame, held its connection forever (found by the
  listener contract suite). The gRPC listener now runs on the same accept
  loop as HTTP, HTTP/2 only; tonic still decodes the calls. A connection
  asked to close that hasn't a second later with nothing in flight is
  dropped, so a silent client can't hold up shutdown either.
- Blocking detector. The engine times every poll of every node; one longer
  than the threshold (`Engine::set_slow_poll`, `[runtime] slow_poll_ms`,
  default 10 ms, 0 off) adds to `forkline_node_slow_polls_total{plan,node}`
  and logs a warning naming the plan and node, at most once per node every
  10 s. Waiting never counts, nor does a poll that panicked (the first
  unwind in a process can take 100 ms; the panic is reported as one). It
  reuses the node-duration clock reads; the benchmark gate shows no
  change beyond 10% (the 8-way fan-out, whose joins are timed separately,
  +8%). `FORKLINE_PANIC_ON_SLOW_POLL_MS` makes a longer poll panic naming
  the node; Forkline's own tests run with 50 ms (`.cargo/config.toml`).
  `Harness::max_poll` fails a module test with `TestError::Blocked`. Built
  with `--cfg tokio_unstable`, the server also times every task poll
  (`forkline_task_slow_polls_total`, logged with the spawn location), which
  `scripts/blocking-check.sh` checks nightly under load.
- Fuzzing through the real server: `http_request` (any bytes into an HTTP
  listener connection, HTTP/1.1 or h2c) and `grpc_frames` (any HTTP/2 frames
  into a gRPC listener connection) run the listeners' connection code over an
  in-memory stream and check that it never panics, always closes, answers
  with well-formed HTTP/1 or HTTP/2, and leaves no task behind. Each target
  now fuzzes for 30 minutes nightly; the corpus is kept between runs and
  pull requests' 60-second run starts from it.
- HTTP/2 conformance: `scripts/h2spec-check.sh` runs h2spec 2.6.0 against
  the echo example's HTTP listener (h2c) and gRPC listener, nightly. Of 146
  tests per listener, HTTP passes all but one and gRPC all but four, none a
  Forkline defect: an invalid preface ends in a reset rather than a GOAWAY
  (both), gRPC answers body-less requests trailers-only where h2spec expects
  a DATA frame (two tests), and the concurrency test never exceeds the limit
  because gRPC closes each stream at once. A reset-versus-close race after a
  client's GOAWAY fails two more tests now and then. Each is listed with its
  reason in `scripts/h2spec-allow.txt`; any other failure fails the job.
- 30-minute soak in the nightly CI run: the horoscope example at 5,000
  req/s. It and the benchmark gate run only when `main` changed in more than
  docs (`*.md`, licence files, the manual Kafka benchmark) since the last
  successful nightly run; a manual run can force the soak and set its
  length (`soak_minutes`, up to 330). `scripts/soak.sh` now runs on Linux too (threads and
  file descriptors from `/proc`), also fails when file descriptors or threads
  grow, when answers are not 2xx, or when the server panics or stops, keeps
  oha's errors, and writes `summary.txt`. Memory fails only past 10% and
  8 MB: on a 12 MB process the allocator's steps alone exceed 10%.
  File descriptors and threads are compared by their floor (the lowest
  count per quarter), since connection pools make the peaks swing: on
  GitHub the outgoing pool to the backend moved between about 20 and 126
  connections while incoming connections stayed at 64.
- `jemalloc` feature: `/metrics` exports jemalloc's statistics
  (`forkline_allocator_allocated_bytes`, `_active_bytes`, `_resident_bytes`)
  through `tikv-jemalloc-ctl` (TiKV). The horoscope example gained a
  `jemalloc` feature; the nightly soak runs on it and fails if the floor of
  allocated bytes grows more than 10% and 2 MB. A 5.5-hour soak on the system
  allocator showed resident memory step from 14 to 17 MB after a pause on the
  runner and stay there: memory glibc kept for reuse, which RSS alone can't
  tell from a leak.
- `ErrorKind::Overloaded` (HTTP 503, gRPC `RESOURCE_EXHAUSTED`, TCP status 4)
  for work refused because a resource is full, such as a pool's queue.
- `check --config server.toml`, and a `config` argument on `App::check_files`
  and `App::assert_files_valid`: with it, plans are also checked against the
  configured pools and the plans the listeners serve. Both examples pass their
  `server.toml`.

- Licensed under MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`); every
  crate declares `license` and `repository`. The repository is public at
  https://github.com/xihe92/forkline.

### Fixes after rc.1
- A reload or rollback that removes a plan a listener serves is now rejected
  (`Engine::require_plans`, set when the server binds). Before, it was
  accepted and the route answered 404 "plan not loaded".
- A module that fails once the deadline has passed (typically a service call
  bounded by the same deadline) is reported as `Deadline` (504), not `Module`
  (500); which one you got used to depend on which timer fired first.
- gRPC service calls that fail after the request's deadline report
  `ServiceErrorKind::Deadline` instead of `Cancelled` or a retryable `Timeout`
  (backends now see our `grpc-timeout` and give up at the same moment).
- A request cancelled before its reply reports `ErrorKind::Cancelled` (503)
  instead of `Internal`.
- HTTP listeners take `max_body_bytes` (default 2 MiB, axum's previous
  implicit limit); larger bodies get 413.
- A TCP listener kept its listening socket open while draining, so the
  kernel went on accepting connections that were never served. It now closes
  as soon as shutdown starts, like the HTTP and gRPC listeners (found by the
  listener contract suite).

### Tests after rc.1
- Engine: failing and panicking joins, panics in every node kind including
  the error node, detached paths cancelled at the deadline, a multibranch
  that picks nothing, late arrivals at a fired join, the deadline between
  nodes, `cx.blocking`, dropped requests aborting spawned paths, reloads
  that replace services, served plans across reload and rollback.
- Server: every outcome on HTTP, gRPC and TCP; admission on every protocol;
  freed admission slots on client disconnect; the drain cut-off on gRPC and
  TCP; detached paths waited for and abandoned at shutdown; `server.toml`
  validation; the TCP adapter's owed responses, abandoned pipelines and
  in-flight limit.
- CLI: `check` and `serve` exit codes, reload on SIGHUP and on file change,
  rejected reloads, clean exit on SIGTERM (the real binary); `App` reloads of
  plans and services from files; the `forkline-plan check` binary.
- Services: breaker half-open behaviour, queueing at the concurrency limit,
  HTTP 502/504 retried and 500 not, gRPC status mapping, deadline,
  cancellation and connection failures, `services.toml` shape errors.
- CI fails below 88% line coverage (90% today, up from 81%).
- Tests run with cargo-nextest (`.config/nextest.toml`): one process per
  test, a warning after 60 s and a kill after 3 minutes that names the hung
  test, no retries, and a JUnit report kept when the CI job fails. Doc tests
  still run with `cargo test --doc`. CI is Linux only.
- The benchmark gate moved from pull requests to a nightly workflow
  (`.github/workflows/nightly.yml`), which benchmarks `main` against the
  commit the previous successful nightly run tested, back to back in one job,
  reruns once before failing on a regression over 10%, and skips when `main`
  hasn't changed. Pull requests compile the benchmark code through clippy's
  `--all-targets`. The
  latency run now fails when the 7-node fork/join plan's p99 is over 50 µs.
- Leak checks: every server test that stops its server checks that no
  request is still in flight on any listener or in admission control while
  it serves, and after shutdown that no detached path, no extra task and
  (under nextest) no extra file descriptor is left. Every async engine test
  checks that no task outlives its requests, and so does each generated plan
  in the scheduling property test. Checked by leaking an admission slot, a
  task and a socket on purpose: each fails the suite.
- Listener contract suite (`crates/forkline-server/tests/contract.rs`): one
  client over HTTP, gRPC and TCP runs the same cases on every listener, as
  Spring runs one suite on Tomcat, Jetty and Netty. Outcomes; admission;
  a client leaving mid-request; a request in flight at shutdown completing,
  one past the drain timeout cut off, idle and silent connections not
  holding up shutdown, a stop racing startup, new connections refused while
  draining; body, message, frame and header limits; silent, trickling and
  idle clients; HTTP request smuggling (both lengths, two lengths, bad
  chunks, bad methods and header names), garbage HTTP/2 frames, HTTP/1 and
  non-gRPC requests on the gRPC port, truncated and empty TCP frames;
  keep-alive, h2c and TCP half-close. The per-protocol outcome, admission and
  drain cut-off tests moved into it. It found the TCP listener accepting
  while draining and the missing gRPC and TCP idle timeouts. Two behaviours
  are by design and documented in the tests: a request with both
  Content-Length and Transfer-Encoding is read by Transfer-Encoding and the
  connection then closed (RFC 9112 6.3), and a TCP request whose client left
  runs to its end, since a closed and a half-closed connection look the same.

### Before 1.0.0
- Decide whether to publish the crates on crates.io (`publish`).
- Run `scripts/soak.sh` for 24 hours on production-like hardware.
- Add `cargo semver-checks` to CI once the crates are published.
