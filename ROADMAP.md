# XmlCraft roadmap

**Stage: pre-alpha** · now: M1 (text editor and XML syntax), M0 release dry run finishing

> **Last reviewed:** 2026-10-27 · **Last updated:** 2026-10-27 · **Change:** M1.1 and M1.2 · **Target:** Altova XMLSpy (feature set described in its public documentation)

XmlCraft is a clean-room, open-source XML editor in the spirit of Altova XMLSpy, built in Rust with egui, like its sibling [PdfCraft](https://github.com/storytold/pdfcraft). The first release (0.1.0) is an XML editor with XSD and DTD support, validation, schema generation from XML, and sample XML generation from schemas. JSON and YAML follow.

| Document | What it holds |
|---|---|
| [docs/roadmap.md](docs/roadmap.md) | Milestones, exit criteria and decisions |
| [docs/gaps.md](docs/gaps.md) | Ranked list of where XmlCraft falls short of XMLSpy |
| [docs/target-app-parity.md](docs/target-app-parity.md) | The numbers (feature breadth, conformance) and how they are measured |
| [docs/architecture.md](docs/architecture.md) | The layered workspace |
| [parity/xmlspy-features.toml](parity/xmlspy-features.toml) | Machine-readable feature checklist |

**Keep it current:** every session, update the milestone row in `docs/roadmap.md` and add a line to the progress log below.

## Headline numbers

- **Feature breadth: 3.3% shipped** (5 shipped and 4 partial of 152 counted features; 64 P0, 48 P1, 25 P2, 15 P3; 10 out of scope), from `cargo xtask parity`. Everything shipped so far is the application shell and agent control; nothing edits XML yet.
- Conformance numbers (W3C xmlconf and xsdtests) start with M1 and M3; nothing is measured yet.

## Progress log

- 2026-10-27: M1.1 + M1.2 `xmlcraft-text` (L0): rope buffer with checked byte offsets and edits that return their inverse, undo history that merges typing and trims its oldest steps against a memory budget (the newest step is always kept), and that refuses to undo onto text that changed behind its back, line-ending counts and conversion, and encodings (byte-order mark, then XML declaration, then UTF-8; UTF-16; legacy encodings; unmappable characters are reported, never replaced). Property tests: buffer against a `String`, undo/redo through every text, and "any bytes are either flagged lossy or written back identically". M0.11 dry run: macOS and Linux built; Windows failed on a WiX component rule, fixed in PR #4.
- 2026-10-27: M0.9 automation, CLI and control channel: `xmlcraft-automation` (JSON-Schema checked tool table, `--root` confinement that follows symbolic links, scripts, an opt-in MCP server, the control channel transport with token, deadline and connection limits), `xmlcraft-cli` (`tools`, `run`, `mcp`, `ui`), the egui control plugin (`xmlcraft --control FILE`; state, inspect, click, type, key, command, screenshot, quit), accessibility-tree text snapshots. Driven against the real window end to end.
- 2026-10-27: M0.10 CI: `ci.yml` on Linux, Windows and macOS (first run found a lint only the newer CI compiler reports; fixed). Repo `thorstenhirsch/xmlcraft` created by the owner; history rebased onto its initial commit.
- 2026-10-27: M0.8 engine and commands: `xmlcraft-engine` with the command registry (ids, labels, menus, shortcuts, kinds, when-enabled), a session and the panic guard; the app runs every command through one guarded `execute`, handles registry shortcuts, builds its Menu from the registry, and has the ⌘K palette.
- 2026-10-27: M0.6 + M0.7 app shell: vendored `winit` and `egui-wgpu` (PdfCraft's patches, with a test that the build really resolves them), `xmlcraft-ui-egui` (PdfCraft's theme tokens, fonts, icons, tab strip, mode bar, panes and rail with Outline, Entry Helpers and Messages, status bar, home card, persisted preferences) and the `xmlcraft` app (renderer fallback, panic hook, logger). A headless `shot` example renders the real shell; `tests/shell.rs` drives it.
- 2026-10-27: M0.5 parity: `cargo xtask parity` and `parity/xmlspy-features.toml` (162 features across 11 areas, all `planned` except 10 out-of-scope `na`), with consistency, evidence and registry checks and tests.
- 2026-10-27: M0.4 assets: `cargo xtask assets` (policy check against `ATTRIBUTION.toml`, separate schema licence allowlist, Altova rule, generated `ATTRIBUTION.md`), Inter and JetBrains Mono, a 46-icon Lucide starter set (hash-verified against PdfCraft's attribution), and an original placeholder app icon with `packaging/icons.sh`.
- 2026-10-27: M0.3 xtask gates: `version`, `layers` (layer table, sideways edges, standalone rules, with tests), `wasm`, `test`, `deny` and `ci` (fmt, clippy, test, layers, wasm, deny; green). `datatypes` moved to L1 because it builds on the L0 `pattern` crate.
- 2026-10-27: M0.1 repository skeleton (licences, lint and format configs, NOTICE, README, stub xtask). M0.2 policies (`CLAUDE.md`, `AGENTS.md`, PR template, progress docs).
