# Fuzzing

Needs a nightly toolchain and `cargo install cargo-fuzz`.

```sh
cd fuzz
mkdir -p corpus/plans && cp seeds/plans/* corpus/plans/
cargo +nightly fuzz run plans corpus/plans -- -max_total_time=300
```

| Target | Input | Checks |
| --- | --- | --- |
| `plans` | any text as `plans.toml` | parsing, graph checks and compilation never panic; whatever compiles also loads |
| `config` | any text as `services.toml` / `server.toml` | parsing and building services never panic |
| `tcp_frames` | any byte stream into the TCP adapter | no panic, the connection always closes, every response is a well-formed frame |
| `http_request` | any bytes into an HTTP listener connection (HTTP/1.1, or h2c after the HTTP/2 preface) | no panic, the connection always closes, responses are HTTP/1 status lines or complete HTTP/2 frames, no task left behind |
| `grpc_frames` | any HTTP/2 frames after the preface into a gRPC listener connection | no panic, the connection always closes, responses are complete HTTP/2 frames starting with SETTINGS, no task left behind |

CI fuzzes every target for 60 seconds on each pull request, and for 30
minutes each, in parallel, nightly. The nightly corpus is minimised and kept
in the GitHub Actions cache, and pull requests start from it. Crashes are
uploaded as workflow artifacts.

The `h2c_post` and `unary_call` seeds are real requests captured from curl
(`--http2-prior-knowledge`); `unary_call` starts after the preface, which the
`grpc_frames` target adds itself.
