# Testing plans

Plans are data, so the compiler doesn't see them: a plan that names a missing
module or reads a key no node writes compiles fine and fails when the server
loads it. Two kinds of test close that gap. A check test runs every load-time
check from `cargo test`; an engine test runs requests through a plan in
process, with no server or network.

> **If you know Spring:** the check test is the plan's equivalent of a Spring
> context test (`@SpringBootTest` failing on a broken bean definition); the
> engine test is `WebTestClient` against the whole application, without a
> port.

## Check every plan from cargo test

```rust
{{#include ../../../examples/horoscope/tests/plans.rs}}
```

`assert_files_valid` runs every check `serve` runs at startup, with the
modules registered exactly as `main` registers them, and panics listing every
problem. With `server.toml`, it also checks that routes point at existing
plans and that every pool a module asks for is configured. Backends don't need
to be running: clients connect lazily.

`App::check_files` does the same and returns the problems instead of
panicking, for a test that wants to assert on them.

## From the command line

```sh
my-service check --config server.toml --plans plans.toml --services services.toml
forkline-plan check plans.toml
```

`check` is the same check, run by your binary; it prints `ok` or every
problem, and exits 1 on a problem, so it fits a deployment pipeline.
`forkline-plan check` needs no modules: it checks only the graph rules, for
example in a repository that holds plans but no code.

## Run requests through a plan

To test what a plan does, build an engine from the plans file and call it in
process:

```rust
{{#include ../../snippets/tests/plans.rs:engine}}
```

`Engine::with_services` takes the registry, the parsed plans and the services
(here only a thread pool; mock clients go in the same registry).
`engine.call(plan, request)` runs one request through the full engine: branch,
joins, merge rules, deadlines and the terminal, and returns the reply.

## See also

- [Testing modules](modules.md).
- [Load, soak and fuzzing](load-soak-fuzz.md).
