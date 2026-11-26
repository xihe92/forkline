# Quick start

This page runs the two examples in the Forkline repository: `echo`, the
smallest plan, and `horoscope`, a plan that forks, branches and joins over
three backend calls. You need Rust 1.88 or newer and the repository:

```sh
git clone https://github.com/xihe92/forkline
cd forkline
```

## Run the echo example

```sh
cargo run -p forkline-echo -- serve \
  --config examples/echo/server.toml --plans examples/echo/plans.toml
```

The server logs one line per listener: HTTP on port 8080, gRPC on 50051, TCP
on 7000, and the admin listener on 6789. In another terminal:

```sh
curl -d 'hi' http://127.0.0.1:8080/echo
# echo: hi
curl -i http://127.0.0.1:8080/echo
# HTTP/1.1 400 Bad Request   (an empty body is rejected)
```

The plan behind `/echo` is three nodes in a line, in
`examples/echo/plans.toml`:

```toml
{{#include ../../examples/echo/plans.toml:1:12}}
```

`decode` stores the body, `echo` adds the prefix from its `config`, and
`encode` sends the result. `error` answers when anything fails; the empty body
fails with a bad request, which becomes 400.

## Change the plan while it runs

The example's `server.toml` sets `watch_interval_ms = 1000`, so the server
reloads when the files change. Edit `examples/echo/plans.toml` and change the
prefix to `"hello: "`. A second later:

```sh
curl -d 'hi' http://127.0.0.1:8080/echo
# hello: hi
curl http://127.0.0.1:6789/admin/snapshot
# {"config_hash":"...","history":[1],"plans":["echo","slow"],"services":[],"version":2}
```

The server logged `reloaded version=2 trigger="file change"`. Requests that
were running during the reload finished on version 1.

Now break the plan: change `module = "Echo"` to `module = "Ecko"`. The reload
is rejected, the log says ``module `Ecko` is not registered`` and names the
node, and `/echo` keeps
answering with version 2. Change it back, and stop the server with Ctrl-C.

## Run the horoscope example

The horoscope plan reads a JSON request, then at the same time looks up the
time and, depending on what the caller wants, a horoscope or a stock price,
and joins the results:

```toml
{{#include ../../examples/horoscope/plans.toml}}
```

It calls three backends, which a small fake server provides on port 9001:

```sh
cargo run --release -p forkline-horoscope --bin horoscope-backend &
cargo run --release -p forkline-horoscope -- serve --config examples/horoscope/server.toml \
  --plans examples/horoscope/plans.toml --services examples/horoscope/services.toml
```

```sh
curl -d '{"want":"horoscope","sign":"leo"}' http://127.0.0.1:8080/query
# {"want":"horoscope","time":"...","result":"leo: a good day to ship","summary":"..."}
curl -d '{"want":"stock","ticker":"ACME"}' http://127.0.0.1:8080/query
curl -i -d '{"want":"weather"}' http://127.0.0.1:8080/query
# HTTP/1.1 400, with a JSON error naming the `branch` node
```

## Next

- [Coming from Spring](coming-from-spring.md), if you know Spring.
- [Overview](core/overview.md), for the design and when to use Forkline.
- [Modules](core/modules.md), to write your own.
