# Services

Modules reach backends only through named services. A service is a client,
such as an HTTP or gRPC client, built from `services.toml` and wrapped in the
same layers every time: a timeout per attempt, retries within a budget, a
concurrency limit and a circuit breaker. Modules look services up by name, so
the backend's address and behaviour live in configuration, not in code.

> **If you know Spring:** a service is a configured `WebClient` bean that
> modules inject by name, with Resilience4j-style timeouts, retries and a
> circuit breaker already applied. `services.toml` is the part of
> `application.yml` that would describe those clients.

## Declaring services

```toml
{{#include ../../../examples/horoscope/services.toml}}
```

Each `[services.<name>]` table has a `kind`, the layer settings every kind
shares, and the kind's own settings (here, `url`). The built-in kinds are
`http` and `grpc`; their settings and the layers are described in
[HTTP and gRPC clients](clients.md).

Several services may point at the same backend, as `time`, `horoscope` and
`stock` do here, so each one gets its own timeout, retries and limits.

## Using a service in a module

A module resolves its client once, while it is built, so a missing service, or
one of the wrong kind, rejects the plan at load time rather than failing the
first request:

```rust
{{#include ../../../examples/horoscope/src/lib.rs:lookup}}
```

The node names the service in its config, so the same module can call
different services at different nodes:

```toml
time = { module = "TimeLookup", config = { service = "time" } }
```

Clients are cheap to clone and safe to share; the module keeps one for the life
of the plan. `cx.service::<T>(name)` also looks a service up per request, for
the rare module that can't know its service at build time.

## Services and reloads

`services.toml` is reloaded together with `plans.toml`: a reload builds new
clients, and requests already running finish with the old ones. Clients
connect lazily, so a backend that is down doesn't stop a plan from loading.

## Services built in code

A binary can add services that aren't in `services.toml`, such as a client
built from a secret store, with `App::service(name, value)`. An entry of the
same name in `services.toml` replaces it.

For a backend Forkline has no client for, register a service kind. The kind's
factory reads its own settings from the service's table and inserts any
cloneable client:

```rust
{{#include ../../snippets/src/kinds.rs:kind}}
```

```toml
[services.sessions]
kind = "kv"
addr = "10.0.0.5:6379"
prefix = "s:"
```

Modules then resolve it with `cx.service::<KvClient>("sessions")`, like any
other service. The shared layer settings (`timeout_ms`, `retries` and the
rest) are parsed for every kind; a custom kind applies them by wrapping its
transport with `forkline_services::layered`, as the built-in kinds do.

## See also

- [HTTP and gRPC clients](clients.md): settings, layers, errors.
- [services.toml reference](../appendix/plans-services-toml.md#servicestoml).
- [Building your server](../server/building.md): `App`.
