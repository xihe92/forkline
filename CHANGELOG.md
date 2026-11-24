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
- `ErrorKind::Overloaded` (HTTP 503, gRPC `RESOURCE_EXHAUSTED`, TCP status 4)
  for work refused because a resource is full, such as a pool's queue.
- `check --config server.toml`, and a `config` argument on `App::check_files`
  and `App::assert_files_valid`: with it, plans are also checked against the
  configured pools and the plans the listeners serve. Both examples pass their
  `server.toml`.

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

### Before 1.0.0
- Choose a licence (plan decision 8) and set `license` and `publish`.
- Run `scripts/soak.sh` for 24 hours on production-like hardware.
- Add `cargo semver-checks` to CI once the crates are published.
