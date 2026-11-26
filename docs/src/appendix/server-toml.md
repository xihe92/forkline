# server.toml reference

`server.toml` configures the process: listeners, admission control, reloads,
shutdown, thread pools, the runtime and telemetry. It is read once at startup;
changes need a restart. Unknown keys are rejected.

## A complete example

```toml
{{#include ../../snippets/reference/server.toml}}
```

## Sections

### `[[http]]`, one per HTTP listener

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `addr` | address | required | Address and port to bind |
| `routes` | table | `{}` | Path (starting with `/`) to plan name; each runs on GET and POST |
| `max_body_bytes` | integer | `2097152` | Largest request body; larger gets 413 |
| `header_read_timeout_ms` | integer | `60000` | Time a client may take to send a request's headers (HTTP/1) |
| `idle_timeout_ms` | integer | `60000` | Close a connection with no request in flight for this long |

### `[[grpc]]`, one per gRPC listener

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `addr` | address | required | Address and port to bind |
| `methods` | table | `{}` | Method name to plan name; `Quote` serves `/forkline.v1.Plans/Quote` |
| `idle_timeout_ms` | integer | `60000` | Close a connection with no call in flight for this long |

### `[[tcp]]`, one per TCP listener

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `addr` | address | required | Address and port to bind |
| `plan` | string | required | Plan every frame runs |
| `max_frame_bytes` | integer | `1048576` | Largest frame; a larger one ends the connection |
| `max_in_flight_per_connection` | integer | `32` | Pipelined requests per connection |
| `idle_timeout_ms` | integer | `60000` | Close a connection idle this long |

### `[admin]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `addr` | address | required | Address and port to bind; keep it private |
| `allow` | list of networks | `["127.0.0.1/32", "::1/128"]` | Callers allowed; others get 403 |

### `[admission]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `red` | integer | required | Requests in flight at which new requests are refused |
| `green` | integer | required | Requests in flight at which admission resumes; below `red` |
| `max_queue_ms` | integer | none | Shed requests that waited longer than this to start |

### `[reload]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `history` | integer | `5` | Previous snapshots kept for rollback |
| `watch_interval_ms` | integer | `0` | Check the plans and services files this often and reload on change; 0 is off |

### `[shutdown]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `drain_timeout_ms` | integer | `10000` | Time for in-flight requests, then detached paths, to finish |

### `[pools.<name>]`, one per thread pool

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `threads` | integer | one per core | Threads in the pool |
| `queue` | integer | `1024` | Jobs that may wait; more are refused with `Overloaded` |

### `[runtime]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `worker_threads` | integer | one per core | Tokio worker threads |
| `max_blocking_threads` | integer | Tokio's default (512) | Cap on the pool behind `cx.blocking` |
| `slow_poll_ms` | integer | `10` | Report a node that holds a worker thread this long in one poll; 0 is off |

### `[telemetry]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `log_format` | `"text"` or `"json"` | `"text"` | Log line format |
| `filter` | string | `"info"` | tracing filter directive; `RUST_LOG` overrides it |
| `otlp_endpoint` | string | none | OTLP/gRPC collector for traces (needs the `otlp` feature) |
| `service_name` | string | `"forkline"` | Service name reported with traces |

## Rules

- At least one HTTP, gRPC or TCP listener.
- Every route, method and TCP `plan` names a plan in `plans.toml`.
- Timeouts, sizes, `threads` and `queue` are greater than 0.
- Pool names are letters, digits, `_` and `-`.
- `allow` entries are valid networks or addresses.
