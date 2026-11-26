# HTTP and gRPC clients

Forkline has two built-in service kinds: `http`, a client built on reqwest
with one connection pool per service, and `grpc`, a client built on tonic that
sends and receives raw message bytes. Both run every call through the same
layers and stop at the request's deadline.

> **If you know Spring:** the `http` client is `WebClient` with a base URL,
> and the layers are what you would add with Resilience4j: `TimeLimiter`,
> `Retry` with a budget, `Bulkhead` and `CircuitBreaker`.

## HTTP

```toml
[services.horoscope]
kind = "http"
url = "http://horoscope.internal:8080/"
connect_timeout_ms = 200
pool_max_idle_per_host = 64
timeout_ms = 40
retries = 1
```

```rust
let reply = self.client.get(cx, format!("/horoscope/{sign}")).await?;
let text = reply.error_for_status()?.text()?;

let created = self.client.post(cx, "/orders", body).await?;

let request = HttpRequest::post("/orders", body)
    .header(http::header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
let response = self.client.call(cx, request).await?;
```

Paths are joined onto the service's `url`. Every call takes `cx`, so it stops
at the request's deadline or when the request is cancelled, and forwards the
inbound `traceparent` and `tracestate` headers.

What comes back:

- 2xx, 3xx and 4xx responses arrive as `Ok(HttpResponse)` with the status, so
  a module can handle a 404 itself; `error_for_status()` turns any non-2xx
  status into an error;
- 502, 503 and 504 arrive as `Unavailable` errors, which are retried;
- other 5xx statuses arrive as `Status` errors, which are not.

HTTPS needs the `tls` feature of `forkline-services` (rustls).

| Setting | Default | Meaning |
| --- | --- | --- |
| `url` | required | Base URL; request paths are joined onto it |
| `connect_timeout_ms` | none | TCP connect timeout |
| `pool_max_idle_per_host` | reqwest's default | Idle connections kept per host |

## gRPC

```toml
[services.ranking]
kind = "grpc"
url = "http://ranking.internal:50051"
timeout_ms = 30
```

```rust
let reply: Bytes = self.client.call(cx, "/ranking.v1.Ranker/Rank", request_bytes).await?;
```

The client takes and returns encoded message bytes, so it needs no generated
code; encode and decode the messages with the library you prefer, such as
prost. Each attempt sends the time left before the request's deadline as
`grpc-timeout`, measured again for each retry, so the backend can stop work the
caller has given up on. The channel connects on first use.

| Setting | Default | Meaning |
| --- | --- | --- |
| `url` | required | Endpoint, such as `http://backend:50051` |
| `connect_timeout_ms` | none | TCP connect timeout |

## Layers

Every kind shares these settings. The layers apply in this order, outermost
first: concurrency limit, circuit breaker, retry, per-attempt timeout, then
the transport.

| Setting | Default | Meaning |
| --- | --- | --- |
| `timeout_ms` | none | Timeout for each attempt; the request's deadline applies as well |
| `retries` | `0` | Retries after a retryable failure |
| `retry_budget_percent` | `20` | Retries may add at most this share of extra calls |
| `retry_backoff_ms` | `0` | Pause before each retry |
| `concurrency_limit` | none | Calls in flight at once |
| `load_shed` | `false` | At the limit, fail at once instead of waiting |
| `breaker` | none | `{ failures, open_ms }`: open after this many consecutive failures, for this long |

Retries happen only for failures worth retrying: an attempt that timed out, a
transport error, or a backend that answered "unavailable" (HTTP 502, 503, 504;
gRPC `UNAVAILABLE` or `RESOURCE_EXHAUSTED`). A gRPC backend's own
`DEADLINE_EXCEEDED` counts as an attempt that timed out. The retry budget stops retries from multiplying load
during an outage: with the default, retries add at most 20% to the calls the
service makes.

When the breaker is open, calls fail at once with `CircuitOpen`. After
`open_ms`, one trial call goes through: if it succeeds the breaker closes, and
if it fails the breaker stays open for another `open_ms`.

## Errors

A failed call returns a `ServiceError`; with `?` it becomes a module error.

| Kind | Meaning | Retried |
| --- | --- | --- |
| `Timeout` | One attempt exceeded `timeout_ms` | yes |
| `Transport` | Connecting or talking to the backend failed | yes |
| `Unavailable` | HTTP 502, 503, 504; gRPC `UNAVAILABLE` or `RESOURCE_EXHAUSTED` | yes |
| `Status` | Any other error status | no |
| `Deadline` | The request's deadline passed | no |
| `Cancelled` | The request was cancelled | no |
| `Overloaded` | Refused at the concurrency limit (`load_shed = true`) | no |
| `CircuitOpen` | The breaker is open | no |
| `Other` | Anything else | no |

Every call is timed in
`forkline_service_call_duration_seconds{service,outcome}`, with `outcome`
`ok` or `error`.

## See also

- [Services](services.md): declaring and resolving services.
- [Errors and deadlines](../core/errors.md): how a failed call becomes a reply.
