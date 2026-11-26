# Blocking work and thread pools

Modules run on Tokio's worker threads, one per CPU core by default, and every
request shares them in short turns. Work that waits must `.await`, which frees
the thread. Synchronous work that takes more than about a millisecond, or a
synchronous call that waits (a non-async database driver, `std::fs`), must
leave the worker threads, or every request queued behind it waits too.

> **If you know Spring:** this is WebFlux's rule for the event loop, and these
> are its remedies: `cx.blocking` is `publishOn(Schedulers.boundedElastic())`,
> and a named pool is a dedicated `Scheduler`, configured in `server.toml`.
> Unlike WebFlux, Forkline measures every step and tells you which one blocks.

## Two ways off the worker threads

- `cx.blocking(move || work())` runs the closure on Tokio's shared blocking
  pool. It needs no setup; `[runtime] max_blocking_threads` caps the pool.
- A named pool from `server.toml` gives a fixed number of threads and a
  bounded queue, created when the server starts. Use one to isolate a kind of
  work and size it on its own.

```toml
[pools.pricing]
threads = 2      # default: one per core
queue = 256      # jobs that may wait; more are refused (default 1024)
```

A module resolves the pool while it is built, so a plan that names a missing
pool is rejected at load time:

```rust
{{#include ../../snippets/src/lib.rs:pool}}
```

`pool.run` waits without holding a worker thread. A full queue fails at once
with `Overloaded` (HTTP 503, gRPC `RESOURCE_EXHAUSTED`, TCP status 4). The wait
ends at the request's deadline or on cancellation: a job still queued is then
skipped, while a job already running can't be stopped and runs to completion.
A panic fails the job, not the thread.

Pools live in `server.toml`, so changing one needs a restart. At shutdown each
pool finishes its queue within the drain timeout.

| Metric | Meaning |
| --- | --- |
| `forkline_pool_threads{pool}` | Threads in the pool |
| `forkline_pool_queued{pool}` | Jobs waiting for a thread |
| `forkline_pool_busy{pool}` | Threads running a job |
| `forkline_pool_jobs_total{pool,result}` | Jobs by result: `done`, `refused`, `panicked`, `skipped` |
| `forkline_pool_wait_seconds{pool}` | Time jobs waited for a thread |

## Work that can't be cut off

Rust can't stop a running thread. A job on a pool or on `cx.blocking` that
outlives the request keeps its thread, and anything it holds, such as a
database connection, until it finishes. Two habits keep that rare:

- check `cx.remaining()` before submitting a job that won't finish in time;
- pass a cancellation flag into long loops and check it between steps.

## Finding a module that blocks

The engine times every poll of every node. A node that holds its worker
thread longer than `slow_poll_ms` in one go adds to
`forkline_node_slow_polls_total{plan,node}` and logs a warning naming the plan
and the node, at most once per node every 10 seconds. Waiting with `.await`
never counts, however long it takes; a blocking call or a long computation
does.

```toml
[runtime]
slow_poll_ms = 10   # default 10; 0 switches it off
```

The timing reuses the clock readings the node-duration metric already takes,
so it costs nothing measurable.

In tests:

- `Harness::max_poll(Duration::from_millis(10))` fails a module's test with
  `TestError::Blocked` if one poll takes longer;
- the environment variable `FORKLINE_PANIC_ON_SLOW_POLL_MS=50` makes any
  longer poll panic, naming the node, so a whole test suite catches blocking
  modules.

Built with `RUSTFLAGS="--cfg tokio_unstable"`, the server also times every
Tokio task poll, not only nodes, counts slow ones in
`forkline_task_slow_polls_total`, and logs where the slow task was spawned.

## See also

- [Runtime and memory](../server/runtime.md): worker threads.
- [Testing modules](../testing/modules.md): `max_poll`.
