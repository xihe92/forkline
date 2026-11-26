# Hot reload

Forkline reloads `plans.toml` and `services.toml` while it serves. A reload
builds a complete new snapshot of plans and services, checks it the same way
startup does, and swaps it in only if everything passes. Requests already
running finish on the snapshot they started with, so no request ever mixes two
configurations.

> **If you know Spring:** this goes further than Spring Cloud's
> `@RefreshScope`, which rebinds properties on beans. A Forkline reload
> replaces the whole request flow, atomically, and can be rolled back.

## Triggering a reload

Any of these rebuilds from the files:

- `POST /admin/reload` on the [admin listener](admin.md);
- the `SIGHUP` signal;
- a file watcher: with `watch_interval_ms` set, the server checks the files
  that often and reloads when their contents change.

```toml
[reload]
history = 5                # previous snapshots kept for rollback
watch_interval_ms = 1000   # 0 (the default) turns the watcher off
```

Each reload is logged with what triggered it and the new version, or with
every problem that rejected it.

## What is checked

A reload runs every check that startup runs (see
[Plans](../core/plans.md#what-is-checked-when-a-plan-loads)), and one more: it
is rejected if it would remove a plan that a route, gRPC method or TCP
listener serves, so a listener can never point at a missing plan.

A rejected reload changes nothing: the running snapshot keeps serving.

## Rolling back

`POST /admin/rollback` swaps back to the previous snapshot. The server keeps
the last `history` snapshots; each rollback steps one further back. A rollback
that would lose a plan a listener serves is refused.

## What a reload doesn't change

`server.toml` is read once, at startup. Listeners, admission control, thread
pools, runtime and telemetry settings need a restart. The binary's modules
don't change either: a reload can rewire and reconfigure them, and add or
remove plans, but a new module needs a new build.

## Cost

Swapping a snapshot is one atomic pointer swap. Forkline's own tests reload
every 10 ms while 32 clients send requests: about 165 reloads and 19,000
requests in 2 seconds, with no failed request.

| Metric | Meaning |
| --- | --- |
| `forkline_reloads_total{result}` | Reloads by result: `ok`, `error`, `rollback` |
| `forkline_snapshot_version` | Version of the running snapshot |

## See also

- [Admin endpoints](admin.md).
- [Testing plans](../testing/plans.md): catch a bad plan before it reaches a
  server.
