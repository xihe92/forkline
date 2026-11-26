# Admission control

Admission control caps the requests a server works on at once. Above the cap,
new requests are refused immediately instead of waiting in a queue, so the
requests already admitted keep their latency and callers learn at once that
they should retry elsewhere or later.

> **If you know Spring:** there is no direct equivalent in Spring Boot. The
> nearest is Tomcat's `maxConnections` and `acceptCount`, but those queue
> connections; Forkline refuses requests, which keeps a client from waiting on
> a server that can't serve it in time.

## Configuration

```toml
[admission]
red = 800            # refuse new requests at 800 in flight...
green = 500          # ...until back down to 500
max_queue_ms = 50    # shed requests that waited longer than this to start
```

When requests in flight reach `red`, the server stops admitting. It starts
again only when the count is back down to `green`. The gap between the two
keeps the server from flapping at the threshold. `green` must be below `red`.

The count is shared by every listener on the server, and the admin listener is
not counted.

A refused request gets an answer at once:

| Protocol | Answer |
| --- | --- |
| HTTP | 503 with `Retry-After: 1` |
| gRPC | `RESOURCE_EXHAUSTED` |
| TCP | status 4 |

## Shedding stale requests

`max_queue_ms` sheds requests that waited too long between arriving and
starting, for example behind a burst. They get the same answer as a refused
request, without running anything. A request whose deadline has already
passed when it would start is shed whether or not `max_queue_ms` is set.

## Choosing the numbers

`red` should be the number of requests the server can work on at once while
still meeting its latency target. Measure it: run a load test at increasing
concurrency and note where p99 latency stops meeting the target. Set `green`
around 60% to 80% of `red`.

In Forkline's own tests, 80 requests at once against `red = 40` admitted 40
(p99 104 ms for 100 ms of work) and refused the other 40 in about 1 ms each,
including the HTTP round trip.

## Metrics

| Metric | Meaning |
| --- | --- |
| `forkline_inflight_requests{listener}` | Requests in flight, per listener |
| `forkline_admission_refused_total{listener}` | Requests refused |
| `forkline_requests_total{plan,outcome="shed"}` | Requests shed before starting |

The `listener` label is the protocol and address, such as `http:0.0.0.0:8080`.

## See also

- [Listeners](listeners.md).
- [Errors and deadlines](../core/errors.md): `Overloaded` from a full pool.
