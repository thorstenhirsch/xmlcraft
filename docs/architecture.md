# XmlCraft architecture

> **Last reviewed:** 2026-11-12 · **Last updated:** 2026-11-12 · **Change:** the Workbench section (M11: docking, the panel table, menus by topic, the adaptive command bar); before that, the layers as built, and the crates planned for the formats beyond XML (2026-11-05); first written as the planned layout · **Target:** the leading commercial XML editors (public documentation)

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
| L0 foundation | `text`, `pattern`, `schemas` | Rope buffer, positions, encodings and undo log; XSD regular expressions; the embedded bundle of standard schemas |
| L1 syntax and datatypes | `syntax`, `datatypes` | Restartable lexer, lossless error-tolerant CST, streaming reader, well-formedness, namespaces, the shared `Diagnostic` type (on `text`); the 44 XSD built-in types and facets (on `pattern`) |
| L2 core model | `grammar`, `catalog` | **The one schema model both DTD and XSD compile into** (declarations, content-model automata, identity constraints, documentation, origin spans); OASIS XML Catalogs |
| L3 core services | `dtd`, `xsd`, `resolve`, `json` (M5) | Schema readers that compile to `grammar`; the resolution chain (open documents → catalogs → local files → cache → network) behind traits; JSON Schema (`json`; the JSON and YAML syntax crates are L1) |
| L4 features | `validate`, `infer`, `sample`, `format`, `complete` | Validation; schema inference; sample generation; formatting as minimal edits; completion, Entry Helpers and quick fixes |
| L5 interaction | `editor` | Headless editor model: multi-cursor selections, folds, editing commands, find/replace |
| L6 façade | `engine` | Sessions and documents, the background analysis scheduler, the command registry |
| L7 platform and frontends | `platform`, `code-editor`, `ui-egui`, `automation` | Network fetcher, disk cache, atomic saves; the egui editor widget; the egui shell; JSON-Schema tools and the opt-in MCP server |
| test support | `testkit`, `oracle` | Synthetic fixtures and generators; black-box wrappers around `xmllint` |
| L8 (exempt) | `apps/*`, `xtask` | Binaries and build tooling |

## Formats beyond XML (planned for 0.2.0, M5–M9)

| Layer | Crate | Role |
|---|---|---|
| L1 | `json-syntax`, `yaml-syntax`, `html`, `bson` (codec), `pdf` | Lossless CST of JSON/JSONC/JSON5/JSON Lines; YAML events with spans and alias caps; HTML5 with void elements, implied end tags and the element and attribute tables; BSON read/write and its Extended JSON text; a minimal PDF object reader and **incremental-update writer** |
| L2 | `datamodel` | One format-neutral tree with spans (object, array, scalar, element, attribute) under Grid, conversion and diff |
| L3 | `json` (JSON Schema) | JSON Schema drafts 04–2020-12 with pointer → span |
| L4 | `openapi` (on top of `json`), `query` (vendored and extended `xee`), `schematron`, `diff`, `convert`, `xbrl`, `einvoice`, SOAP/WSDL message building | Engines and format logic; all pure and wasm-clean |
| L7 | `platform` (HTTP/SOAP client, proxy, webview host — native only), `ui-egui` (Grid, diff view, debugger and profiler panes, browser view) | Everything that needs a socket, a window or a webview |

Two rules for these formats. **A projected document**: BSON is shown and edited as its Extended JSON text, and a PDF with embedded XML is shown as its XML in a linked tab; saving writes the original container back and every byte outside the edited object stays identical (a PDF only ever gets an appended incremental update; encrypted or certified files are refused). **A language abstraction** in `syntax`, `editor` and `engine` (highlight, modes, typing aids, outline, folds, format, analysis) replaces the XML assumptions, so JSON, YAML and the rest are not special cases.

## Workbench (the desktop shell, M11)

The window around the editor is a workbench (`crates/ui-egui/src/workbench.rs`):

- **The panel table is the single source of truth.** `Panel::spec` gives each tool window its key (the name saved in the preferences and in `ui.state`), title, icon, rail zone (Document or Workspace), default place, default visibility and the command that toggles it. The default layout, the activity rail and View ▸ Reset Layout are generated from it.
- **Hosting.** The panels are tabs in `craft_ui::docking` groups, split around the editor. The editor is protected: it cannot be closed, moved or covered, and a panel goes only to its left, right or bottom (never above it, never floating). Every change works on a copy and is refused if it breaks these rules; a saved layout that breaks them gives the default.
- **A tool is a dock panel**, not a modal window (Search is the first; its form lives in the app, its results fill the rest of the panel).
- **Persistence.** The layout is saved with the preferences by panel key; panels this build does not know are dropped, and an older file's `left_open`/`right`/`bottom_open` switches migrate to the matching panels.
- **Menus and the command bar** are drawn from the command registry (`engine::commands`): menus by topic (File, Edit, Document, Schema, Tools, View, Help), and the command bar's actions for the kind of document in front from one table (`bar_actions.rs`), worked out from `DocKind` and cheap looks at the text, cached per document version.

## Key decisions

- **The text is the model.** The buffer's bytes are the source of truth; syntax tree, diagnostics and outline are derived. Edits are minimal and saves are byte-faithful (`CLAUDE.md`).
- **One grammar model.** DTD and XSD compile to the same `grammar` IR, so validation, completion, Entry Helpers, sample generation and the Info pane never care which schema language is in use. Content models are Glushkov automata that can answer "what is allowed next" and "what is the shortest valid completion".
- **Analysis runs off the UI thread**, debounced and tagged with a generation number so stale results are dropped, behind `catch_unwind`.
- **The editor is its own crate** (`code-editor`): it is large, reused by every language view, tested in isolation and kept out of the shell crate where merge conflicts collect.
- **Network is a trait.** `resolve` knows nothing about HTTP; `platform` provides the fetcher and cache, and tests use a mock.
- **Bundled schemas are unmodified upstream files** (`AGENTS.md` §1.6).

## Revision history

- 2026-10-27: created from the approved implementation plan.
- 2026-11-05: the layers as built; the formats beyond XML and the two rules for them (projected documents, the language abstraction).
- 2026-11-12: the Workbench section (M11).
