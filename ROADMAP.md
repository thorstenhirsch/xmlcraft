# XmlCraft roadmap

**Stage: pre-alpha** · next: M0 bootstrap (workspace, gates, app shell, CI, packaging)

> **Last reviewed:** 2026-10-27 · **Last updated:** 2026-10-27 · **Change:** initial plan · **Target:** Altova XMLSpy (feature set described in its public documentation)

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

- **Feature breadth: 0%** (0 shipped of 152 counted features; 64 P0, 48 P1, 25 P2, 15 P3; 10 out of scope), from `cargo xtask parity`.
- Conformance numbers (W3C xmlconf and xsdtests) start with M1 and M3; nothing is measured yet.

## Progress log

- 2026-10-27: M0.5 parity: `cargo xtask parity` and `parity/xmlspy-features.toml` (162 features across 11 areas, all `planned` except 10 out-of-scope `na`), with consistency, evidence and registry checks and tests.
- 2026-10-27: M0.4 assets: `cargo xtask assets` (policy check against `ATTRIBUTION.toml`, separate schema licence allowlist, Altova rule, generated `ATTRIBUTION.md`), Inter and JetBrains Mono, a 38-icon Lucide starter set (hash-verified against PdfCraft's attribution), and an original placeholder app icon with `packaging/icons.sh`.
- 2026-10-27: M0.3 xtask gates: `version`, `layers` (layer table, sideways edges, standalone rules, with tests), `wasm`, `test`, `deny` and `ci` (fmt, clippy, test, layers, wasm, deny; green). `datatypes` moved to L1 because it builds on the L0 `pattern` crate.
- 2026-10-27: M0.1 repository skeleton (licences, lint and format configs, NOTICE, README, stub xtask). M0.2 policies (`CLAUDE.md`, `AGENTS.md`, PR template, progress docs).
