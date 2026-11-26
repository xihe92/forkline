# Forkline Reference

Forkline is a Rust plan engine for backend services. You write small modules,
declare which request attributes each one reads and writes, and wire them into
a graph in a TOML plan. Forkline runs that graph for every request, over HTTP,
gRPC or TCP, with the same deadlines, error handling, metrics and tracing on
every step.

Transport, concurrency and telemetry come from established crates: Tokio,
tower, hyper, axum, tonic and tracing. Forkline adds what they don't:
pipelines defined in configuration, typed data contracts checked when a plan
loads, fork and join with merge rules, and plan reloads that never mix two
configurations inside one request.

This is the reference manual for Forkline 1.0.

## How this book is organised

| Part | Read it to |
| --- | --- |
| Getting started | Run the examples, and map Forkline onto Spring if you know it |
| Core | Understand plans, modules and the request map, and how a request runs |
| Services | Call backends with timeouts, retries, limits and circuit breakers |
| Server | Build, configure, operate and observe a Forkline service |
| Testing | Test modules and plans, and see how Forkline performs |
| Appendix | Look up every config key, metric and error kind |

New to Forkline: read the [Quick start](quick-start.md), then the
[Overview](core/overview.md). Every page opens with "If you know Spring", a
note naming the closest Spring WebFlux or Spring Boot feature and how
Forkline differs; [Coming from Spring](coming-from-spring.md) collects them in
one table.

The Rust API is documented separately in the API docs. The source, issues and
changelog are in the [GitHub repository](https://github.com/xihe92/forkline).

## Licence

Forkline is licensed under either the MIT licence or the Apache License 2.0, at
your option.
