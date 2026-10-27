# Developing XmlCraft

> **Last reviewed:** 2026-10-27 · **Last updated:** 2026-10-27 · **Change:** `cargo xtask fuzz`

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

## Match CI's compiler

CI builds with the newest stable Rust, whose clippy can report lints an older local toolchain does not (the first CI run failed on one). Install the same toolchain next to yours and use it for the gate:

```sh
rustup toolchain install 1.99.1 --profile minimal --component clippy --component rustfmt --target wasm32-unknown-unknown
RUSTUP_TOOLCHAIN=1.99.1 cargo xtask ci
```

## Quality gates

```sh
cargo fmt --check
cargo clippy -p <crate> -j 2 --all-targets -- -D warnings
cargo test -p <crate> -j 2
cargo xtask ci        # the full gate; CI runs it on every PR
```

Scope builds to the crates you touched; a full `--workspace` build is CI's job.

## Measuring performance

- `cargo xtask bench` measures the editor's performance targets on synthetic documents (it re-runs itself in release mode; `--list`, `--only NAME`, `--json`, `--no-release`; with `--json` the table goes to standard error and standard output is the JSON). It fails when a scenario is over its budget in `xtask/baselines/bench.toml`; a budget above its target is a known shortfall in `docs/gaps.md` and is only ever lowered. It is not part of `xtask ci` (timings on shared runners are noise).

## Fuzzing

- `cargo xtask fuzz [--time 300] [--iterations N] [--seed 1] [--jobs N] [--timeout 10]` mutates seeds (synthetic documents of every kind of markup, encoding and line ending, plus the files of the W3C suite under 16 KB when `conformance/xmlconf` is fetched) and runs `xmlcraft-cli check-one FILE --edit` on each in a child process. That opens the bytes as a document and holds the engine to its promises (`xmlcraft_engine::selfcheck`): an unedited document is saved as the bytes it was opened from, the ranges of an analysis are inside the text, formatting a well-formed document keeps it well-formed and changes only white space and is idempotent, and after a run of every kind of editing the carets are in the text and undoing all of it gives back the original. A child that dies or fails a promise is a crash, one that runs past the timeout is a hang; each is minimised and kept in `fuzz-out/findings/` (git-ignored; never commit a finding derived from a corpus file: turn each real one into a small synthetic regression test). It exits non-zero on a crash. The `Fuzz` workflow runs it nightly.

## External tools used by tests

- `xmllint` (libxml2-utils) is a black-box oracle for `cargo xtask oracle` and some tests. It is optional locally: tests that need it skip with a notice unless `XMLCRAFT_ORACLES_REQUIRED=1` is set (CI sets it on Linux).
- The W3C and OASIS conformance suites are fetched by `cargo xtask conformance --fetch` into the gitignored `conformance/` directory.

## Revision history

- 2026-10-27: created.
- 2026-10-27: `cargo xtask bench` described (under "Measuring performance").
- 2026-10-27: `cargo xtask fuzz` and `xmlcraft-cli check-one` described.
