# Developing XmlCraft

> **Last reviewed:** 2026-10-27 · **Last updated:** 2026-10-27 · **Change:** initial

Working instructions for agents and contributors are in `AGENTS.md` and `CLAUDE.md`. This page collects practical build notes.

## Toolchain

XmlCraft needs Rust 1.95 or later (the workspace `rust-version`; the `egui` and `craft-ui` dependencies need it). For the wasm check, install the target once:

```sh
rustup target add wasm32-unknown-unknown
```

## Low-disk Cargo profile

When local build storage is constrained, select the opt-in `low-disk` profile explicitly:

```sh
cargo build --profile low-disk -p xmlcraft-cli
cargo test --profile low-disk -p xmlcraft-text --lib
```

The profile inherits the normal development settings and disables incremental compilation for this profile only. The development profile keeps only line tables for workspace code and no debug information for dependencies, so backtraces show file and line numbers but not local variables; for a debugger, build with `CARGO_PROFILE_DEV_DEBUG=true`. Remove only the low-disk artifacts with `cargo clean --profile low-disk`.

## Quality gates

```sh
cargo fmt --check
cargo clippy -p <crate> -j 2 --all-targets -- -D warnings
cargo test -p <crate> -j 2
cargo xtask ci        # the full gate; CI runs it on every PR
```

Scope builds to the crates you touched; a full `--workspace` build is CI's job.

## External tools used by tests

- `xmllint` (libxml2-utils) is a black-box oracle for `cargo xtask oracle` and some tests. It is optional locally: tests that need it skip with a notice unless `XMLCRAFT_ORACLES_REQUIRED=1` is set (CI sets it on Linux).
- The W3C and OASIS conformance suites are fetched by `cargo xtask conformance --fetch` into the gitignored `conformance/` directory.

## Revision history

- 2026-10-27: created.
