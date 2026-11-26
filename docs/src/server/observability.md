# Observability

Forkline logs every request failure with its plan and node, exposes Prometheus
metrics for requests, nodes, services, pools and listeners, and creates a
tracing span per request that can be exported over OTLP.

> **If you know Spring:** this is Spring Boot's observability support:
> structured logging, Micrometer's Prometheus registry, and Micrometer Tracing
> with an OTLP exporter, built in and with no extra dependencies except the
> `otlp` feature.

## Logs

```toml
[telemetry]
log_format = "json"   # or "text" (the default)
filter = "info"       # a tracing filter; RUST_LOG overrides it
```

`filter` takes tracing's directive syntax, for example
`"info,forkline_engine=debug"`. JSON lines carry the current span and its
parents, so every line of a request has its `request_id` and `plan`.

What is logged at the default level:

- startup: each listener's address, or every problem that stopped the start;
- every reload, with its trigger and version, or every problem that rejected
  it;
- every failed request at `warn`, with the plan, node, kind and message;
- slow polls, at most one warning per node every 10 seconds;
- shutdown and drain progress.

## Metrics

`GET /metrics` on the admin listener returns every metric in the OpenMetrics
text format, from the official Prometheus client for Rust. The main ones:

| Metric | Meaning |
| --- | --- |
| `forkline_requests_total{plan,outcome}` | Requests by outcome: `ok`, `error`, `bad_request`, `deadline`, `shed` |
| `forkline_request_duration_seconds{plan}` | Request duration |
| `forkline_node_duration_seconds{plan,node}` | Node duration |
| `forkline_node_errors_total{plan,node,kind}` | Node failures by kind |
| `forkline_service_call_duration_seconds{service,outcome}` | Service call duration, `ok` or `error` |
| `forkline_inflight_requests{listener}` | Requests in flight |

Every metric is listed in the [Metrics reference](../appendix/metrics.md).

## Traces

Every request runs in a span named `request`, with the request id, plan,
snapshot version and config hash; subplans get child spans. Inbound
`traceparent` and `tracestate` headers are forwarded on every outbound HTTP
and gRPC call, so backends join the caller's trace.

With the `otlp` feature, spans are exported over OTLP/gRPC, and an inbound
`traceparent` becomes the parent of the request's span:

```toml
[telemetry]
otlp_endpoint = "http://otel-collector:4317"
service_name = "quote-service"   # default "forkline"
```

If `otlp_endpoint` is set but the binary was built without the feature, the
server prints a warning at startup and runs without export.

## tokio-console

With the `console` feature, and built with `RUSTFLAGS="--cfg tokio_unstable"`,
the server serves tokio-console, which shows every task live. Use it in
development; it costs some performance.

## See also

- [Metrics reference](../appendix/metrics.md).
- [Runtime and memory](runtime.md): allocator metrics.
