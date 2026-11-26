# Modules

A module is a Rust type that does one step of a plan. It implements `Build`,
which says how to construct it from its node's config, and exactly one
node-kind trait, which says what kind of step it is. Modules are registered by
name, and plans refer to them by that name.

> **If you know Spring:** a module is the closest thing to a WebFlux handler
> method, but smaller: it does one step and never calls another step. Its
> contract plays the part of the method's annotated parameters and return
> value, and its `Config` is like a `@ConfigurationProperties` class bound
> from the plan instead of from `application.yml`.

## A first module

```rust
{{#include ../../snippets/src/lib.rs:greet}}
```

Three parts:

- `interface!` declares the module's contract: it reads `name` and writes
  `greeting`, both `String`. It generates `GreetIo`, with a getter for each key
  it reads and a setter for each key it writes. See
  [The request map](request-map.md).
- `Build` constructs the module once, when a plan loads. `NoConfig` means the
  node takes no `config` table (and rejects one).
- `Module::call` runs once per request. It gets the request's context and its
  view of the map, and returns `Ok(())` or an error.

## Node kinds

Each module implements one of these traits. Only `Module` is async: the others
work on data already in the map, so they are plain functions and never hold
the request's task while waiting.

| Trait | Method | Runs | Returns |
| --- | --- | --- | --- |
| `Decode` | `decode(cx, body, io)` | At the root, once | Writes the parsed body into the map |
| `Module` | `async call(cx, io)` | Anywhere between root and encode | Reads and writes the map; may await |
| `Branch` | `choose(cx, io)` | Where a plan branches | The label of the one child to run |
| `MultiBranch` | `choose(cx, io)` | Where a plan branches | The labels of any number of children to run |
| `Join` | `on_path(cx, path, merged)` | Where paths meet, once per arriving path | `JoinStep::Wait`, or `Done` to stop waiting |
| `Encode` | `encode(cx, io)` | Once, at the end | The `Reply` |
| `Terminal` | `respond(cx, io, error)` | When the plan fails | The error `Reply` |

A fork is not a module kind: any node that isn't a branch and has two or more
children is a fork, and its children run concurrently.

## Configuration

A module's `Config` type is deserialized from its node's `config` table, so one
module can run at several nodes with different settings:

```rust
{{#include ../../snippets/src/lib.rs:config}}
```

```toml
hello = { module = "Greeting", config = { salutation = "hello" } }
shout = { module = "Greeting", config = { salutation = "hey", loud = true } }
```

`build` runs once per plan load, never per request, so it's the place for
anything expensive: compiling a regular expression, reading a lookup table.
Returning an error from `build` rejects the plan with that message, and a
config table that doesn't match the type (a missing field, a wrong type, an
unknown key with `deny_unknown_fields`) is rejected the same way.

To use a service or a thread pool, implement `build_with` instead of `build`:
it also gets a `BuildContext`, which resolves services by name. A missing
service then fails the plan at load time, not on the first request. See
[Services](../services/services.md).

## Registering modules

A binary registers its modules by name and hands the registry to the server:

```rust
{{#include ../../snippets/src/lib.rs:registry}}
```

Each registration method matches a node kind: `decode`, `module`, `branch`,
`multi_branch`, `join`, `encode` and `terminal`. Registering two modules
under one name is an error. [Building your server](../server/building.md)
shows the `main` function around it.

## The request context

Every method gets `cx: &Ctx`, the request's context:

| Method | Gives |
| --- | --- |
| `cx.deadline()`, `cx.remaining()` | The request's deadline, and the time left |
| `cx.is_cancelled()`, `cx.cancellation()` | Whether the request was cancelled, and a token to await |
| `cx.env()` | What the listener knows: protocol, peer address, route, query string, headers or gRPC metadata |
| `cx.request_id()`, `cx.plan()`, `cx.span()` | The request's id, plan name and tracing span |
| `cx.service::<T>(name)` | A service, looked up per request (prefer `build_with`) |
| `cx.blocking(work)` | Runs synchronous work off the worker threads |
| `cx.execute_plan(plan, body)` | Runs another plan inline; see [Subplans](subplans.md) |

## Errors

A module fails by returning a `NodeError`. Any standard error converts into
one with `?`, as a module error. To say what kind of failure it is, construct
it directly:

```rust
return Err(NodeError::bad_request("`sign` must be letters"));   // HTTP 400
return Err(NodeError::msg("inventory answered nonsense"));      // HTTP 500
```

A panic in a module is caught and fails the request like an error. The kinds
and how they become protocol statuses are in
[Errors and deadlines](errors.md).

## See also

- [The request map](request-map.md): contracts and keys.
- [Testing modules](../testing/modules.md): run one module in a test.
- API docs: the `forkline` crate.
