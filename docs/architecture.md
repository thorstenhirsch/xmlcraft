# XmlCraft architecture

> **Last reviewed:** 2026-11-18 · **Last updated:** 2026-11-19 · **Change:** the XBRL tool window; "Query engines": XQuery 3.1 as our own front-end on `xee` (`xquery`, M7.17); before that, the Preview tool window and the webview host and the interpreter hook (M7.12, M7.13) · **Target:** the leading commercial XML editors (public documentation)

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
| L4 features | `validate`, `infer`, `sample`, `format`, `complete`, `query`, `xquery` | Validation; schema inference; sample generation; formatting as minimal edits; completion, Entry Helpers and quick fixes; XPath and XSLT on `xee`; XQuery 3.1 compiled to `xee`'s IR |
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
| L4 | `openapi` (on top of `json`), `query` (the published `xee` with a vendored, patched `xee-interpreter`: NaN comparisons, and a hook before every instruction for the XPath profiler and debugger, whose sessions run on a worker thread natively and are refused on the web), `schematron`, `diff`, `convert`, `xbrl`, `einvoice` (detection, validation, the neutral invoice model), `invoice-pdf` (the visual invoice as a PDF with the XML embedded: templates, layout, writing through PdfCraft's `pdfcraft-cos`, a git dependency at a pinned revision), SOAP/WSDL message building | Engines and format logic; all pure and wasm-clean |
| L7 | `platform` (HTTP/SOAP client, proxy, webview host — native only), `ui-egui` (Grid, diff view, debugger and profiler panes, browser view) | Everything that needs a socket, a window or a webview |
| L4 | `openapi` (on top of `json`), `query` (the published `xee` with a vendored, patched `xee-interpreter`), `schematron`, `diff`, `convert`, `xbrl`, `einvoice` (detection, validation, the neutral invoice model), `invoice-pdf` (the visual invoice as a PDF with the XML embedded: templates, layout, writing through PdfCraft's `pdfcraft-cos`, a git dependency at a pinned revision), SOAP/WSDL message building | Engines and format logic; all pure and wasm-clean |
| L7 | `platform` (HTTP/SOAP client, proxy — native only), `ui-egui` (Grid, diff view, debugger and profiler panes, the browser view and its webview host) | Everything that needs a socket, a window or a webview |

Two rules for these formats. **A projected document**: BSON is shown and edited as its Extended JSON text, and a PDF with embedded XML is shown as its XML in a linked tab; saving writes the original container back and every byte outside the edited object stays identical (a PDF only ever gets an appended incremental update; encrypted or certified files are refused). **A language abstraction** in `syntax`, `editor` and `engine` (highlight, modes, typing aids, outline, folds, format, analysis) replaces the XML assumptions, so JSON, YAML and the rest are not special cases.

## Workbench (the desktop shell, M11)

The window around the editor is a workbench (`crates/ui-egui/src/workbench.rs`):

- **The panel table is the single source of truth.** `Panel::spec` gives each tool window its key (the name saved in the preferences and in `ui.state`), title, icon, rail zone (Document or Workspace), default place, default visibility and the command that toggles it. The default layout, the activity rail and View ▸ Reset Layout are generated from it.
- **Hosting.** The panels are tabs in `craft_ui::docking` groups, split around the editor. The editor is protected: it cannot be closed, moved or covered, and a panel goes only to its left, right or bottom (never above it, never floating). Every change works on a copy and is refused if it breaks these rules; a saved layout that breaks them gives the default.
- **A tool is a dock panel**, not a modal window; its form lives in the app (`crates/ui-egui/src/tools/`), so a hidden panel keeps it, worker answers reach it whatever else is open, and it follows the document in front (a `→ name` line, an empty state for what it cannot use; no snapshot of a document is held). The tools: **Search** (Find in Files, bottom), **Query** (XPath and XSLT tabs, right), **Compare** (Compare and Merge tabs, bottom), **Transform** (convert between formats with a preview, right), **HTTP & OpenAPI** (right), **SOAP** (Request and Debugger tabs, right; the debugger's proxy belongs to the app and outlives the panel) and **E-Rechnung** (Overview, Validate and PDF tabs, right; its Validate is F8, whose validator runs the e-invoice rules when asked by F8 and not while typing) and **XBRL** (Facts, Contexts, Units and Findings tabs, bottom; it reads an instance or Inline XBRL document and its taxonomy on a thread with the resolver F8 uses, `xmlcraft_engine::xbrl::report`, a moment after the text stood still). Tools placed on the right are the tabs of one group in one column; those below the editor are tabs next to Problems; all start hidden. Each has a `view.tool.<key>` toggle command, a rail icon dimmed when the document in front is not one it works on, and Escape in one of its fields returns the keyboard to the editor. `Dialog` is left with questions and one-shot forms.
- **The webview host** (`crates/ui-egui/src/preview_host.rs`, M8.12) is where the Preview tool's page is to be shown in a locked-down native child webview: `wry` (WKWebView, WebView2) as a dependency of `ui-egui` on macOS and Windows only (`cfg(any(target_os = "macos", target_os = "windows"))`), so Linux CI needs no GTK or WebKitGTK and the web build compiles without it; `cargo xtask layers` keeps `wry` out of every crate below L7. The native part is built and was seen working on macOS, but `wry` brings MPL-2.0 crates, so it waits for the owner's licence decision; until then `Host::EMBEDDED` is false and every platform uses the system browser. The page is pure data from `engine::preview` (`prepare`: the HTML as it is, the HTML an XSLT stylesheet makes, SVG, or a tree of other XML), served from memory under the `xmlcraft-preview:` scheme; the policy is pure functions there too (`content_security_policy`, `response_headers`, `serve`, `allows_navigation`, `fallback_page`), so it is tested exactly and the headless tool `html_preview` reports it. The view sits above the egui surface, so a pure `Lifecycle` decides per frame from where the panel drew its place: made lazily, moved and resized in physical pixels, hidden while an egui window, menu, popup, dialog, the palette or a drag is over it or its tab is not in front, destroyed when the panel closes; a view that cannot be made (no window handle, no WebView2 runtime) falls back to the system browser, as Linux and the web always do (an owner-only temporary copy with the policy as a `<meta>`, removed on exit).
- **Persistence.** The layout is saved with the preferences by panel key; panels this build does not know are dropped, and an older file's `left_open`/`right`/`bottom_open` switches migrate to the matching panels.
- **Menus and the command bar** are drawn from the command registry (`engine::commands`): menus by topic (File, Edit, Document, Schema, Tools, View, Help), and the command bar's actions for the kind of document in front from one table (`bar_actions.rs`), worked out from `DocKind` and cheap looks at the text, cached per document version.

## Query engines

XPath 3.1 and XSLT run on Paligo's `xee` (MIT): the published crates and a vendored, patched `xee-interpreter` (`vendor/README.md`). `xee` has no XQuery, and no Rust crate offers a usable XQuery 3.1 (researched 2026-11-18: `xrust` lists maps, arrays, inline functions, quantified expressions, `instance of`, casts and the 2.0–3.1 functions as not implemented, and XQuery as a goal). So XQuery is our own front-end on `xee`'s machinery, **`xquery`** (L4, no I/O, wasm-clean; decision of 2026-11-18, task M7.17):

- **Route.** A hand-written, scannerless parser (`parse.rs`) reads the whole XQuery 3.1 grammar except Full-Text and Update into our own tree (`ast.rs`, a span on every node, names resolved while parsing, types and node tests as `xee`'s own). Our compiler (`compile.rs`, modelled on `xee-xpath-compiler`'s converter) emits **`xee-ir`** directly (its types are public), and `xee_ir::compile_xpath` turns that into bytecode for the same interpreter XPath uses, so results, functions, errors and the profiler/debugger hook are shared. Rejected: desugaring XQuery into XPath text (there is no XPath form of constructors, `order by` with mixed directions, `group by`, windows or `try`/`catch`) and patching `xee-xpath-ast`/`xee-xpath-compiler` for XQuery nodes (two more vendored crates to keep in step).
- **How the parts map.** The prolog's functions and global variables live in an environment array `[functions, $g1, …]` passed to every declared function as a hidden first argument, so functions recurse and call each other; FLWOR `for`/`let`/`where` nest directly; `order by`, `group by` and `count` turn the tuple stream into arrays and back, sorted and grouped by a **prelude** of helper functions written in XQuery (`prelude.rs`; `order by` is stable, by one stable `fn:sort` pass per key, descending as reverse–sort–reverse); windows are found by prelude functions from the start and end conditions compiled as functions; node constructors use the instructions `xee` has for XSLT (`XmlElement`, `XmlAttribute`, `XmlAppend`, …); `try`/`catch` calls `fn:xmlcraft-try`.
- **Vendored patches it needs** (`xee-interpreter`, `vendor/README.md` patches 4–7): the content rules of constructors without panics (document children, array flattening, XQTY0024, XQDY0025, XQTY0105, nodes always copied); `fn:xmlcraft-try` (run a function, and on a dynamic error put the interpreter back and call a handler; XmlCraft's limit errors are not caught); deeper frames (4096 instead of 64) and a bound on library-to-inline call nesting (the Rust stack); `QNameOrString` public (to choose the serialization method).
- **Limits.** The query text (4 MB); the parser's and the compiler's recursion are bounded by the bytes of stack they take, measured as they go (80 KiB and 96 KiB: the same on every platform, whatever its frames weigh), the tree's depth by 40, and a chain of operators that lean left, a path and a FLWOR by 128 operators, steps and clauses (`xee`'s code generator recurses once per link of the code they make); a chain of one associative operator (`or`, `and`, `||`, `union`) and a path of axis steps are built balanced, chains and runs of `let`/`for`/`where` are compiled in loops, `xee`'s code generator panicking on a too-large loop is caught (XMLC0004), and a tree is dropped with a heap stack, not by recursion; the run has a step budget (a hook, `XMLC0001`, not catchable), the frames and the nesting bounds above; results are cut like XPath's.
- **Grammar coverage.** Version and module declarations, the whole prolog (namespaces, defaults, boundary-space, base-uri, construction, ordering, empty order, copy-namespaces, decimal formats, schema and module imports, variables with `external` defaults, context item, annotated functions, options), FLWOR with every clause (`for` with `allowing empty` and `at`, `let`, `where`, `group by`, `order by` with all modifiers, `count`, tumbling and sliding windows), `switch`, `typeswitch`, `if`, `try`/`catch`, quantified expressions, `instance of`/`treat`/`cast`/`castable`, `=>`, `!`, `||`, lookups, maps and arrays, inline functions and partial application, direct and computed constructors (element, attribute, document, text, comment, PI, namespace; CDATA, references, `{{`/`}}`, boundary whitespace), string constructors, nested comments and pragmas.
- **Out of scope** (refused with their codes, never ignored): static typing and schema awareness (`import schema` XQST0009, `validate` XQST0075), module import across files (XQST0059), external functions, collations other than the codepoint collation (XQST0038), Full-Text, Update (a later task), serialization parameters beyond the XML, text, JSON and adaptive methods.

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
- 2026-11-12: `invoice-pdf` (L4) added, writing through PdfCraft's `pdfcraft-cos` (git dependency, pinned revision); the query engine's `xee-interpreter` is vendored with the NaN comparison fix.
- 2026-11-12: the tool windows (M11.8–M11.12) listed in the Workbench section.
- 2026-11-18: the webview host of the Preview tool window (M8.11–M8.13); the L7 row puts it in `ui-egui`, not `platform`.
- 2026-11-13: the E-Rechnung tool window (M13) listed in the Workbench section.
- 2026-11-18: the vendored `xee-interpreter` has a hook before every instruction (patch 3), used by `query`'s profiler and debugger (M7.12, M7.13).
- 2026-11-19: "Query engines": XQuery 3.1 as our own front-end (`xquery`, L4) compiling to `xee-ir`; the vendored interpreter's patches 4–7 for it.
- 2026-11-18: the XBRL tool window (M9.20) listed in the Workbench section.
