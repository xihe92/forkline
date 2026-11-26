# Testing modules

`forkline-testkit` runs one module in a test, without a plan or a server. You
give it inputs by key name, mock services if it needs them, and read its
outputs by key name. It checks the inputs against the module's contract, so a
test can't pass a key of the wrong type.

> **If you know Spring:** this is the unit-test half of `WebTestClient`'s
> `bindToController`: one handler, no server, mocked collaborators. For the
> whole flow, test the plan (see [Testing plans](plans.md)).

```toml
[dev-dependencies]
forkline-testkit = { git = "https://github.com/xihe92/forkline" }
tokio = { version = "1", features = ["macros", "rt"] }
```

## A module

```rust
{{#include ../../snippets/tests/modules.rs:module}}
```

- `Harness::<M>::from_config(config, services)` builds the module from the
  body of its node's `config` table, the way a plan would, resolving services
  from `services`. `Harness::new(module)` takes a module you built yourself.
- `inputs!` names the keys and values to put in the map before the run.
- `run_module()` runs it and returns the outputs: `get::<T>(key)` reads a
  value, `changed()` lists the keys the module wrote, `present()` lists every
  key in the map.
- `max_poll` fails the test with `TestError::Blocked` if the module holds the
  thread longer than the limit in one poll; see
  [Blocking work](../core/blocking.md).

## Other node kinds

Each kind has its own run method: `run_decode(body)`, `run_branch()`,
`run_multi_branch()`, `run_encode()`, `run_terminal(&error)` and `run_join()`.

```rust
{{#include ../../snippets/tests/modules.rs:decode_branch}}
```

A failing module returns `TestError::Node` with the module's error, so a test
can check its kind.

## Joins

A join test describes the fork: the map before it (`fork_base`) and what each
path changed (`path`, once per path, in arrival order). The harness calls the
join once per path and then applies the engine's merge rules, so the test sees
what the next node would:

```rust
{{#include ../../snippets/tests/modules.rs:join}}
```

`conflicts` lists keys several paths changed that the join didn't resolve, and
`paths_seen` tells how many paths the join saw before it returned `Done`.

## Services and pools

Give the harness the services and pools a module resolves at build time, in a
`ServiceRegistry`. A real pool is cheap to create in a test:

```rust
{{#include ../../snippets/tests/modules.rs:pool}}
```

For a service, wrap any tower service as a client, for example a mock that
answers fixed responses:
`HttpClient::from_service("horoscope", mock)` or
`GrpcClient::from_service("ranking", mock)`.

## Time

The harness gives each run a deadline (1 second by default; change it with
`budget`). Async tests can use Tokio's paused clock: the harness takes the
deadline from Tokio's clock, so `tokio::time::pause()` and `advance` work with
deadlines and timeouts.

## See also

- [Testing plans](plans.md): check and run whole plans.
- API docs: the `forkline-testkit` crate.
