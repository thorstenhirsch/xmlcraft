# XmlCraft architecture

> **Last reviewed:** 2026-10-27 · **Last updated:** 2026-10-27 · **Change:** initial; describes the planned layout, only `xtask` exists so far · **Target:** Altova XMLSpy (public documentation)

This file describes the layered workspace. Until a crate's milestone lands, the row below is the plan, not the code; each crate gets a `README.md` when it is created.

## Shape

One Cargo workspace (edition 2024, Rust ≥ 1.95, `MIT OR Apache-2.0`): library crates in `crates/` named `xmlcraft-<name>`, binaries in `apps/`, build tooling in `xtask/`. Rust only: no handwritten JS/TS, in the product or the build.

| Binary | What it is |
|---|---|
| `apps/xmlcraft` | The desktop app (macOS, Windows, Linux): eframe/egui, single instance, Apple events, update check on request |
| `apps/xmlcraft-cli` | `check`, `validate`, `format`, `infer`, `sample`, `resolve`, `tools`, `run` (any automation tool or a JSON script), `ui` (drive the running app) and the opt-in `mcp` server |

## Layers

`cargo xtask layers` enforces the layering. A crate may depend only on crates in lower layers; nothing below L7 may depend on egui, winit, eframe, rfd or craft-ui; every crate in L0–L7 must compile for `wasm32-unknown-unknown` (`cargo xtask wasm`).

| Layer | Crates | Role |
|---|---|---|
| L0 foundation | `text`, `pattern`, `datatypes`, `schemas` | Rope buffer, positions, encodings and undo log; XSD regular expressions; the 44 XSD built-in types and facets; the embedded bundle of standard schemas |
| L1 syntax | `syntax` | Restartable lexer, lossless error-tolerant CST, streaming reader, well-formedness, namespaces, the shared `Diagnostic` type |
| L2 core model | `grammar`, `catalog` | **The one schema model both DTD and XSD compile into** (declarations, content-model automata, identity constraints, documentation, origin spans); OASIS XML Catalogs |
| L3 core services | `dtd`, `xsd`, `resolve`, `json` (M5) | Schema readers that compile to `grammar`; the resolution chain (open documents → catalogs → local files → cache → network) behind traits; JSON/YAML syntax and JSON Schema |
| L4 features | `validate`, `infer`, `sample`, `format`, `complete` | Validation; schema inference; sample generation; formatting as minimal edits; completion, Entry Helpers and quick fixes |
| L5 interaction | `editor` | Headless editor model: multi-cursor selections, folds, editing commands, find/replace |
| L6 façade | `engine` | Sessions and documents, the background analysis scheduler, the command registry |
| L7 platform and frontends | `platform`, `code-editor`, `ui-egui`, `automation` | Network fetcher, disk cache, atomic saves; the egui editor widget; the egui shell; JSON-Schema tools and the opt-in MCP server |
| test support | `testkit`, `oracle` | Synthetic fixtures and generators; black-box wrappers around `xmllint` |
| L8 (exempt) | `apps/*`, `xtask` | Binaries and build tooling |

## Key decisions

- **The text is the model.** The buffer's bytes are the source of truth; syntax tree, diagnostics and outline are derived. Edits are minimal and saves are byte-faithful (`CLAUDE.md`).
- **One grammar model.** DTD and XSD compile to the same `grammar` IR, so validation, completion, Entry Helpers, sample generation and the Info pane never care which schema language is in use. Content models are Glushkov automata that can answer "what is allowed next" and "what is the shortest valid completion".
- **Analysis runs off the UI thread**, debounced and tagged with a generation number so stale results are dropped, behind `catch_unwind`.
- **The editor is its own crate** (`code-editor`): it is large, reused by every language view, tested in isolation and kept out of the shell crate where merge conflicts collect.
- **Network is a trait.** `resolve` knows nothing about HTTP; `platform` provides the fetcher and cache, and tests use a mock.
- **Bundled schemas are unmodified upstream files** (`AGENTS.md` §1.6).

## Revision history

- 2026-10-27: created from the approved implementation plan.
