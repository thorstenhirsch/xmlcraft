# XmlCraft roadmap: milestones and what's next

> **Last reviewed:** 2026-10-27 · **Last updated:** 2026-10-27 · **Change:** M1.1 and M1.2 done · **Target:** Altova XMLSpy (feature set described in its public documentation)

Each milestone is a series of one-PR tasks with ids like `M1.4`. A milestone is done when its **exit criteria** are met and measured, not when its tasks are merged. [ROADMAP.md](../ROADMAP.md) is the one-page summary.

| Milestone | Scope | Version | Status |
|---|---|---|---|
| **M0** Bootstrap | Workspace, policies, xtask gates, assets, app shell with the PdfCraft look, automation/CLI skeleton, CI, packaging for all six targets | 0.0.x | **in progress** (M0.1–M0.11 done, M0.6 folded into M0.7; the release dry run builds macOS and Linux, the Windows MSI fix is in review) |
| **M1** Text editor and XML syntax | Rope buffer, encodings, lossless CST, well-formedness, the code-editor widget (highlighting, folding, multi-cursor, find/replace, XML typing aids), formatting, outline, large files | 0.0.x | **in progress** (M1.1 buffer and undo, M1.2 encodings and line endings done; lexer and CST next) |
| **M2** Catalogs, resolver and DTD | OASIS catalogs, bundled-schema crate, resolver with disk cache and network fetcher, the shared grammar model, DTD parsing and validation, Entry Helpers | 0.0.x | not started |
| **M3** XSD engine and validation UX | XSD regex and datatypes, schema compilation, XSD 1.0 validation, identity constraints, the W3C xsdtests ratchet, schema-aware completion and Entry Helpers | 0.0.x | not started |
| **M4** Generators and bundled set | Schema inference (XSD and DTD), sample XML generation, the full bundled schema set, Home page, About — **MVP release** | **0.1.0** | not started |
| **M5** JSON and YAML | JSON/YAML editing, JSON Schema validation for both, SchemaStore catalog, inference and sample generation | 0.2.0 | not started |
| **M6+** Later | XPath 3.1 window, Grid view, graphical schema view, XSLT/XQuery, diff, projects, XML↔JSON, XSD 1.1, web build, translations | — | not started |

## Exit criteria (MVP path)

- **M0:** `cargo xtask ci` green on Ubuntu and Windows; a dry-run release builds, installs and launches on all six targets (win x64/arm64, macOS universal, linux x64/arm64).
- **M1:** editor performance targets met (1 MB: keystroke-to-paint p95 under 8 ms; 100 MB: opens in under 3 s, scrolls at 60 fps); xmlconf not-well-formed 100%; byte-identical open/save round-trip property holds; an hour of fuzzing finds no panic.
- **M2:** xmlconf valid and invalid at least 99%; DocBook 4.5 and XHTML documents validate offline; at least 99% verdict agreement with `xmllint --valid`.
- **M3:** xsdtests at least 90%; at least 98% agreement with `xmllint --schema` on the curated set; a 10 MB document validates in under 1 s.
- **M4:** every corpus document validates against both its inferred XSD and DTD; at least 95% of generated samples validate; xsdtests at least 95%; 0.1.0 released for all six targets.

## Decisions that shape the roadmap

- Own clean-room XSD 1.0 and DTD engine (no third-party validator at runtime): one grammar model feeds validation, completion, Entry Helpers and sample generation.
- Standard schemas are bundled (about 150 files, hard cap 250) *and* downloaded and cached on demand.
- JSON and YAML follow the MVP; YAML is validated with JSON Schema because YAML has no schema language of its own.
- The first release ships unsigned unless signing secrets are configured.

## Revision history

- 2026-10-27: created from the approved implementation plan.
- 2026-10-27: M1.1 and M1.2 (`xmlcraft-text`) done.
