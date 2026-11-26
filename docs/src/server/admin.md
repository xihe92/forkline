# Admin endpoints

The admin listener serves health checks, metrics, and the controls for
reloading and rolling back plans. It runs on its own address, separate from
the listeners that serve traffic, and only answers callers on an allow-list.

> **If you know Spring:** this is Spring Boot Actuator: `/healthz` and
> `/readyz` are the liveness and readiness probes, `/metrics` is
> `/actuator/prometheus`, and reload and rollback have no Actuator equivalent.

## Configuration

```toml
[admin]
addr = "127.0.0.1:6789"
allow = ["127.0.0.1/32", "::1/128", "10.0.0.0/8"]
```

Bind the admin listener to a private address. `allow` lists the networks that
may call it, as CIDR blocks or single addresses; the default is loopback only.
Other callers get 403.

## Endpoints

| Endpoint | Answers |
| --- | --- |
| `GET /healthz` | 200 while the process runs |
| `GET /readyz` | 200 while serving; 503 once shutdown has started |
| `GET /metrics` | Every metric, in the OpenMetrics text format |
| `GET /admin/snapshot` | The running snapshot: version, config hash, plans, services, rollback history |
| `POST /admin/reload` | Rebuild from the files and swap it in |
| `POST /admin/rollback` | Swap back to the previous snapshot |

`/admin/snapshot` returns JSON:

```json
{
  "version": 3,
  "config_hash": "9f2c4e7a1b0d5e83",
  "plans": ["checkout", "notify", "price", "quote"],
  "services": [{ "name": "horoscope", "type": "forkline_services::http::HttpClient" }],
  "history": [1, 2]
}
```

`/admin/reload` answers `{"version": 4}` on success, or 400 with every problem
found, one per entry of `"errors"`, leaving the running snapshot in place.
`/admin/rollback` answers `{"version": 2}`, or 409 when there is no earlier
snapshot or the earlier one lacks a plan a listener serves. See
[Hot reload](reload.md).

## Probes

For Kubernetes, point the liveness probe at `/healthz` and the readiness probe
at `/readyz`. When shutdown starts, `/readyz` turns 503 at once, so the load
balancer stops sending traffic while in-flight requests finish.

## See also

- [Hot reload](reload.md).
- [Metrics reference](../appendix/metrics.md).
- [Shutdown](shutdown.md).
