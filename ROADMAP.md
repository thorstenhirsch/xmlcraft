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
| [parity/xmlspy-features.toml](parity/xmlspy-features.toml) | Machine-readable feature checklist (arrives with M0.5) |

**Keep it current:** every session, update the milestone row in `docs/roadmap.md` and add a line to the progress log below.

## Headline numbers

Nothing is measured yet. Conformance numbers (W3C xmlconf and xsdtests) start with M1 and M3, and feature breadth with `cargo xtask parity` (M0.5).

## Progress log

- 2026-10-27: M0.1 repository skeleton (licences, lint and format configs, NOTICE, README, stub xtask). M0.2 policies (`CLAUDE.md`, `AGENTS.md`, PR template, progress docs).
