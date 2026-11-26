# Runtime and memory

Forkline runs on Tokio's multi-threaded runtime. The defaults suit most
services: one worker thread per CPU core, Tokio's blocking pool for
`cx.blocking`, and the system's memory allocator. For production on Linux, we
recommend building with jemalloc.

> **If you know Spring:** this page is the JVM tuning page. There is no heap
> size to set and no garbage collector: memory is freed when the last
> reference to it goes away. The allocator choice plays the part of the GC
> choice: it decides how freed memory is reused and returned to the system.

## Threads

```toml
[runtime]
worker_threads = 8          # default: one per core
max_blocking_threads = 64   # cap on the pool behind cx.blocking (Tokio's default: 512)
slow_poll_ms = 10           # see "Blocking work and thread pools"
```

Leave `worker_threads` at its default unless the container's CPU limit is
lower than the cores the machine reports; then set it to the limit. Named
thread pools are configured under `[pools]`; see
[Blocking work and thread pools](../core/blocking.md).

## Memory and the allocator

A Forkline server is small: the horoscope example uses about 15 MB of resident
memory serving 5,000 requests a second. How much more it uses after a burst
depends on the allocator, not only on Forkline.

The system allocator on Linux (glibc's malloc) keeps memory it has freed for
reuse and rarely returns it to the system. After a burst, resident memory can
step up and stay there, even though the memory is free and will be reused.
That looks like a leak on a memory graph but isn't one.

jemalloc returns unused memory to the system a few seconds after it is freed,
and reports how much memory is really in use. Turn it on with the `jemalloc`
feature of `forkline-server`:

```toml
[dependencies]
forkline-server = { git = "https://github.com/xihe92/forkline", features = ["jemalloc"] }
```

It adds three metrics:

| Metric | Meaning |
| --- | --- |
| `forkline_allocator_allocated_bytes` | Bytes in live allocations: what the program uses. A leak makes this grow |
| `forkline_allocator_active_bytes` | Bytes in the pages those allocations occupy |
| `forkline_allocator_resident_bytes` | Physical memory jemalloc holds, including freed pages not yet returned |

If resident memory grows while allocated bytes stay flat, the allocator is
holding memory for reuse; if allocated bytes grow, something is keeping data
alive.

## Tuning jemalloc

jemalloc reads its settings from the `_RJEM_MALLOC_CONF` environment variable.
Two are worth knowing:

```sh
_RJEM_MALLOC_CONF=background_thread:true,dirty_decay_ms:10000 my-service serve
```

- `background_thread:true` returns memory from a background thread, so the
  work isn't done during a request. Recommended for latency-sensitive
  services.
- `dirty_decay_ms` is how long freed memory is kept before it is returned
  (default 10 seconds). Returning memory and taking it back costs system calls
  and page faults, so a longer time suits bursty traffic and a shorter one
  suits a container with a tight memory limit.

## See also

- [Observability](observability.md): metrics.
- [Load, soak and fuzzing](../testing/load-soak-fuzz.md): the soak test that
  watches memory.
