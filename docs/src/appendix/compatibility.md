# Compatibility and support

## Platforms

| | |
| --- | --- |
| Rust | 1.88 or newer (the minimum is tested in CI) |
| Linux | Supported: CI, benchmarks and the soak test run on Linux (x86-64) |
| macOS | Builds and runs, for development |
| Windows | Not tested |

## Versions

Forkline follows Semantic Versioning from 1.0.0. These are covered:

- the public Rust API of the `forkline*` crates;
- the formats of `server.toml`, `plans.toml` and `services.toml`;
- metric names and their label names.

A 1.x release doesn't remove or change any of them in a way that breaks a
working service, and a plan that loads on 1.0 loads on every 1.x. A minor
release may add to them: new API items, new config keys with defaults that
keep the old behaviour, new metrics and new labels.

Not covered:

- release candidates (`1.0.0-rc.N`), which may still change anything before
  1.0.0;
- items marked `#[doc(hidden)]`, which exist for fuzzing and tests;
- log messages and their wording;
- the exact buckets of duration histograms.

The minimum supported Rust version may rise in a minor release, never in a
patch release. A rise is announced six months ahead, goes to a Rust version
at least six months old, and is noted in the changelog.

Changes are listed in `CHANGELOG.md` in the repository.

## Dependencies

Forkline builds on Tokio, tower, hyper, axum, tonic, reqwest and tracing. Every
dependency is checked in CI with cargo-deny for security advisories and
licences. Forkline itself is licensed under either the MIT licence or the
Apache License 2.0, at your option.
