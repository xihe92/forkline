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

### Before 1.0.0
- Choose a licence (plan decision 8) and set `license` and `publish`.
- Run `scripts/soak.sh` for 24 hours on production-like hardware.
- Add `cargo semver-checks` to CI once the crates are published.
