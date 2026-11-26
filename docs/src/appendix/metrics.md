# Metrics reference

`GET /metrics` on the admin listener returns every metric in the OpenMetrics
text format. Counters carry the `_total` suffix there. Durations are
histograms in seconds.

## Requests and nodes

| Metric | Type | Labels | Meaning |
| --- | --- | --- | --- |
| `forkline_requests_total` | counter | `plan`, `outcome` | Requests by outcome: `ok`, `error`, `bad_request`, `deadline`, `shed` |
| `forkline_request_duration_seconds` | histogram | `plan` | Request duration |
| `forkline_node_duration_seconds` | histogram | `plan`, `node` | Node duration |
| `forkline_node_errors_total` | counter | `plan`, `node`, `kind` | Node failures by [error kind](error-kinds.md) |
| `forkline_node_slow_polls_total` | counter | `plan`, `node` | Polls that held a worker thread longer than `slow_poll_ms` |
| `forkline_merge_conflicts_total` | counter | `plan`, `node`, `key` | Keys changed on several paths and not resolved by the join |

## Listeners

| Metric | Type | Labels | Meaning |
| --- | --- | --- | --- |
| `forkline_inflight_requests` | gauge | `listener` | Requests in flight, such as `listener="http:0.0.0.0:8080"` |
| `forkline_admission_refused_total` | counter | `listener` | Requests refused by admission control |
| `forkline_task_slow_polls_total` | counter | none | Task polls longer than `slow_poll_ms`; counted only in builds with `--cfg tokio_unstable` |

## Services

| Metric | Type | Labels | Meaning |
| --- | --- | --- | --- |
| `forkline_service_call_duration_seconds` | histogram | `service`, `outcome` | Service calls, `ok` or `error` |

## Thread pools

| Metric | Type | Labels | Meaning |
| --- | --- | --- | --- |
| `forkline_pool_threads` | gauge | `pool` | Threads in the pool |
| `forkline_pool_queued` | gauge | `pool` | Jobs waiting for a thread |
| `forkline_pool_busy` | gauge | `pool` | Threads running a job |
| `forkline_pool_jobs_total` | counter | `pool`, `result` | Jobs by result: `done`, `refused` (queue full), `panicked`, `skipped` (the request gave up while the job was queued) |
| `forkline_pool_wait_seconds` | histogram | `pool` | Time jobs waited in the queue |

## Reloads

| Metric | Type | Labels | Meaning |
| --- | --- | --- | --- |
| `forkline_reloads_total` | counter | `result` | Snapshot loads: `ok`, `error`, `rollback` |
| `forkline_snapshot_version` | gauge | none | Version of the snapshot serving requests |

## Memory (with the `jemalloc` feature)

| Metric | Type | Labels | Meaning |
| --- | --- | --- | --- |
| `forkline_allocator_allocated_bytes` | gauge | none | Bytes in live allocations |
| `forkline_allocator_active_bytes` | gauge | none | Bytes in the pages holding live allocations |
| `forkline_allocator_resident_bytes` | gauge | none | Physical memory jemalloc holds, including freed pages not yet returned |
