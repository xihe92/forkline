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
