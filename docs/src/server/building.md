# Building your server

A Forkline service is an ordinary Rust binary: a `main` function that
registers your modules and hands them to Forkline's command-line server. The
server reads three files at startup: `server.toml` for listeners and
operations, `plans.toml` for the plans, and `services.toml` for the backends.

> **If you know Spring:** this page is Spring Boot's application class. The
> `main` function plays `SpringApplication.run`, the module registry replaces
> component scanning, and the three TOML files together play the role of
> `application.yml`.

## Dependencies

Until Forkline is published to crates.io, depend on it from Git:

```toml
[dependencies]
forkline = { git = "https://github.com/xihe92/forkline" }
forkline-server = { git = "https://github.com/xihe92/forkline" }
forkline-services = { git = "https://github.com/xihe92/forkline" }   # HTTP and gRPC clients

[dev-dependencies]
forkline-testkit = { git = "https://github.com/xihe92/forkline" }
```

`forkline` re-exports what a module author needs: the node traits, `Ctx`, the
errors and the `interface!` macro.

| Crate | Contents |
| --- | --- |
| `forkline` | Everything a module author needs, re-exported |
| `forkline-core` | Request map, contracts, `Ctx`, errors, node traits, registry, thread pools |
| `forkline-macros` | The `interface!` macro |
| `forkline-plan` | Plan format, validation, compilation, the `forkline-plan check` tool |
| `forkline-engine` | The executor and snapshots |
| `forkline-services` | Service clients and their layers, built from `services.toml` |
| `forkline-server` | Listeners, server config, logging, metrics, the command line |
| `forkline-testkit` | Run one module in a test |

## The main function

```rust
{{#include ../../../examples/echo/src/main.rs}}
```

`registry()` lists the binary's modules (see [Modules](../core/modules.md)).
`main_with` parses the command line and runs it with the built-in service
kinds. To add service kinds or services built in code, use `App`:

```rust
use forkline_server::cli::App;

fn main() -> std::process::ExitCode {
    match my_service::registry() {
        Ok(registry) => App::new(registry)
            .service_kinds(my_service::service_kinds())   // adds kinds next to http and grpc
            .service("secrets", my_service::secrets_client())
            .run(),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
```

## The command line

```sh
my-service serve --config server.toml --plans plans.toml --services services.toml
my-service check --config server.toml --plans plans.toml --services services.toml
```

| Command | Does |
| --- | --- |
| `serve` | Loads and checks everything, binds the listeners, and serves until `SIGTERM` or Ctrl-C |
| `check` | Runs every check `serve` runs at startup and exits: 0 if all pass, 1 with every problem listed |

| Option | Default | Meaning |
| --- | --- | --- |
| `--config` | `server.toml` for `serve`; none for `check` | Server config; with `check`, also checks routes and pools |
| `--plans` | `plans.toml` | Plans file |
| `--services` | none | Services file |

## Cargo features

| Crate | Feature | Adds |
| --- | --- | --- |
| `forkline-server` | `jemalloc` | jemalloc as the allocator, and its statistics in `/metrics`; see [Runtime and memory](runtime.md) |
| `forkline-server` | `otlp` | Trace export over OTLP; see [Observability](observability.md) |
| `forkline-server` | `console` | tokio-console support (build with `RUSTFLAGS="--cfg tokio_unstable"`) |
| `forkline-services` | `tls` | HTTPS for the HTTP client (rustls) |

## Requirements

Forkline needs Rust 1.88 or newer and is supported on Linux; it also builds and
runs on macOS for development. See
[Compatibility and support](../appendix/compatibility.md).

## See also

- [Listeners](listeners.md): what `server.toml` serves.
- [Testing plans](../testing/plans.md): run `check` from `cargo test`.
