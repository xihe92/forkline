# Shutdown

On `SIGTERM` or Ctrl-C, the server stops taking new work, gives the work in
progress a bounded time to finish, and then stops. Nothing waits forever: the
whole drain is capped by `drain_timeout_ms`.

> **If you know Spring:** this is Spring Boot's graceful shutdown
> (`server.shutdown=graceful` with `spring.lifecycle.timeout-per-shutdown-phase`),
> extended to the work Forkline adds: detached paths and thread pools.

```toml
[shutdown]
drain_timeout_ms = 10000   # default 10 s
```

## The order

1. `/readyz` turns 503, so load balancers stop sending traffic.
2. The traffic listeners stop accepting connections, and ask open
   connections to close once their requests are done.
3. In-flight requests get up to `drain_timeout_ms` to finish. When the time is
   up, the ones still running are cancelled and answered "shutting down".
4. Detached paths get what is left of `drain_timeout_ms`; any still running
   then are abandoned, and the number is logged.
5. The admin listener stops, so metrics can be scraped during the drain.
6. Each thread pool gets up to `drain_timeout_ms` to finish its jobs; a
   warning is printed for a pool that still has jobs running.

## In Kubernetes

Set the pod's `terminationGracePeriodSeconds` above `drain_timeout_ms`, with a
few seconds to spare, so the kubelet doesn't kill the process mid-drain. The
readiness probe on `/readyz` takes the pod out of the service as soon as
shutdown starts.

## See also

- [Admin endpoints](admin.md): the probes.
- [Branching and forking](../core/branching.md#detached-paths): detached
  paths.
