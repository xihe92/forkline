# Listeners

A listener accepts requests on one address and runs a plan for each. A server
can have any number of HTTP, gRPC and TCP listeners, configured in
`server.toml`, and they can serve the same plans: a plan doesn't know which
protocol its request came from unless it asks `cx.env()`.

> **If you know Spring:** these are Spring Boot's embedded server and its
> `server.*` properties, except that one process can listen on several
> addresses and protocols at once. There is no TLS on the listeners; terminate
> TLS in a load balancer or proxy in front of the server.

## HTTP

```toml
[[http]]
addr = "0.0.0.0:8080"
routes = { "/quote" = "quote", "/notify" = "notify" }
max_body_bytes = 2097152
header_read_timeout_ms = 60000
idle_timeout_ms = 60000
```

Each route runs its plan for `GET` and `POST`. The request body (empty for a
`GET`) goes to the decode node; the query string, path and headers are in
`cx.env()` (`query`, `route` and `metadata`). The reply's body and headers go
back as the response, with a status taken from its kind: 200 for success, and
for failures the mapping in [Errors and deadlines](../core/errors.md).

The listener speaks HTTP/1.1 and HTTP/2 over plain TCP (h2c, with prior
knowledge). A body larger than `max_body_bytes` gets 413.

## gRPC

```toml
[[grpc]]
addr = "0.0.0.0:50051"
methods = { Quote = "quote" }
idle_timeout_ms = 60000
```

Each method runs its plan as a unary call: `Quote` above is
`/forkline.v1.Plans/Quote`. The request message's bytes go to the decode node
as they are, and the reply body goes back as the response message, so any
client that can send bytes works without generated code. Decode the message in
the decode node with the protobuf library you prefer.

The caller's `grpc-timeout` becomes the request's deadline if it is earlier
than the plan's budget. Request metadata is in `cx.env().metadata`.

## TCP

```toml
[[tcp]]
addr = "0.0.0.0:7000"
plan = "quote"
max_frame_bytes = 1048576
max_in_flight_per_connection = 32
idle_timeout_ms = 60000
```

A TCP listener runs one plan for every frame. A request frame is a 4-byte
big-endian length followed by the body. A response frame is the same length
prefix, then one status byte, then the reply body:

| Status byte | Meaning |
| --- | --- |
| 0 | ok |
| 1 | bad request |
| 2 | failed (module error, panic, merge conflict, internal) |
| 3 | deadline |
| 4 | overloaded: refused by admission control, shed, or a full pool |

A client may pipeline up to `max_in_flight_per_connection` requests on one
connection; responses come back in request order. A frame larger than
`max_frame_bytes` ends the connection after the responses already owed are
sent.

## Timeouts for slow and idle clients

Every listener closes a connection that has had no request in flight for
`idle_timeout_ms`. On TCP, a client that trickles a frame one byte at a time
counts as idle; on gRPC, so does one that never sends the HTTP/2 preface.
HTTP listeners also close a connection whose client takes longer than
`header_read_timeout_ms` to send a request's headers (HTTP/1). All default to
60 seconds, the same as Spring MVC's (Tomcat's) connection timeout.

A connection asked to close (idle, or at shutdown) that hasn't closed one
second later, with nothing in flight, is dropped.

## Settings

| Listener | Key | Default | Meaning |
| --- | --- | --- | --- |
| all | `addr` | required | Address and port to bind |
| all | `idle_timeout_ms` | `60000` | Close a connection idle this long |
| `[[http]]` | `routes` | `{}` | Path to plan; paths start with `/` |
| `[[http]]` | `max_body_bytes` | `2097152` (2 MiB) | Larger bodies get 413 |
| `[[http]]` | `header_read_timeout_ms` | `60000` | Time to send a request's headers (HTTP/1) |
| `[[grpc]]` | `methods` | `{}` | Method name to plan |
| `[[tcp]]` | `plan` | required | Plan every frame runs |
| `[[tcp]]` | `max_frame_bytes` | `1048576` (1 MiB) | Largest frame; larger ends the connection |
| `[[tcp]]` | `max_in_flight_per_connection` | `32` | Pipelined requests per connection |

Every route, method and TCP plan must name a plan that exists when the server
starts, and a reload that would remove one is rejected. A server needs at
least one listener.

## See also

- [Admission control](admission.md): refusing work under overload.
- [server.toml reference](../appendix/server-toml.md).
