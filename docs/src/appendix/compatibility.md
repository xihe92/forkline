# Compatibility and support

## Platforms

| | |
| --- | --- |
| Rust | 1.88 or newer (the minimum is tested in CI) |
| Linux | Supported: CI, benchmarks and the soak test run on Linux (x86-64) |
| macOS | Builds and runs, for development |
| Windows | Not tested |

## Versions

Forkline follows Semantic Versioning. The public Rust API of the `forkline*`
crates and the formats of `server.toml`, `plans.toml` and `services.toml` are
covered: a 1.x release doesn't remove or change them in a way that breaks a
working service, and a plan that loads on 1.0 loads on every 1.x.

Not covered:

- items marked `#[doc(hidden)]`, which exist for fuzzing and tests;
- log messages and their wording;
- the exact buckets of duration histograms.

Changes are listed in `CHANGELOG.md` in the repository.

## Dependencies

Forkline builds on Tokio, tower, hyper, axum, tonic, reqwest and tracing. Every
dependency is checked in CI with cargo-deny for security advisories and
licences. Forkline itself is licensed under either the MIT licence or the
Apache License 2.0, at your option.
