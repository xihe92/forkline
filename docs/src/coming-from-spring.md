# Coming from Spring

If you know Spring WebFlux or Spring Boot, most of Forkline maps onto
something you already use. The biggest difference is where the flow of a
request lives: in Spring it is code in a controller or handler, composed with
Reactor operators; in Forkline it is a plan in a TOML file, and the code is
split into small modules that each do one step.

## Concepts

| Spring | Forkline | Notes |
| --- | --- | --- |
| `@RestController` method, or `HandlerFunction` | A plan | The plan holds the flow; the steps are modules |
| `RouterFunction`, `@RequestMapping` | `routes` in `server.toml` | Path (or gRPC method) to plan name |
| `@RequestBody` binding, `HttpMessageReader` | A `Decode` module | Parses the body into the request map |
| A service method called from a controller | A `Module` | One async step |
| `Mono.zip(a, b)` | A fork | A node with two or more children |
| `if` / `switch` choosing which call to make | A `Branch` module | Returns the label of the child to run |
| The combinator passed to `zip` | A `Join` module | Combines what the paths wrote |
| Return value, `HttpMessageWriter` | An `Encode` module | Builds the reply |
| `@ExceptionHandler`, `WebExceptionHandler` | The `Terminal` module | Builds the reply when anything fails |
| `ServerWebExchange` attributes | The request map | Typed keys, checked when the plan loads |
| Bean injection | `BuildContext::service`, `cx.pool` | Resolved by name when the plan loads |
| `WebClient` with Resilience4j | A service in `services.toml` | Timeout, retries with a budget, limit, breaker |
| `publishOn(Schedulers.boundedElastic())` | `cx.blocking`, or a named pool | Synchronous work off the worker threads |
| Reactor Netty event loop | Tokio worker threads | One per core; modules must not block |

## Configuration

| Spring Boot | Forkline |
| --- | --- |
| `application.yml` | `server.toml` (process), `plans.toml` (flows), `services.toml` (backends) |
| `server.port`, `server.netty.*` | `[[http]]`, `[[grpc]]`, `[[tcp]]` listeners |
| `server.shutdown=graceful` | `[shutdown] drain_timeout_ms` |
| `@ConfigurationProperties` | A module's `Config` type, from its node's `config` table |
| Spring Cloud `@RefreshScope` | Hot reload of plans and services, with rollback |

## Operations

| Spring Boot | Forkline |
| --- | --- |
| Actuator health and readiness | `/healthz`, `/readyz` on the admin listener |
| Actuator Prometheus endpoint | `/metrics` |
| Micrometer Tracing | A span per request; OTLP export with the `otlp` feature |
| JVM heap and GC tuning | `[runtime]` threads, and the allocator (jemalloc) |
| `@SpringBootTest` | `App::assert_files_valid` and in-process engine tests |
| `WebTestClient` on one controller | `forkline-testkit`'s `Harness` on one module |

## What is different

- **No reflection, no proxies, no runtime wiring.** Module names map to types
  in one registry function. Everything a plan needs is checked when it loads,
  and a plan with a problem never serves.
- **No `Mono` or `Flux`.** A module is an ordinary `async fn`; the plan does
  the composition that Reactor operators do in WebFlux.
- **One reply per request.** There is no streaming request or response, and
  no server-sent events or WebSockets.
- **No database layer.** Use an async driver (such as SQLx) from a module, or
  register it as a custom service kind.
- **Memory is small and steady.** There is no garbage collector; a service
  idles at a few megabytes and stays within tens of megabytes under load.

## See also

- [Overview](core/overview.md).
- [Quick start](quick-start.md).
