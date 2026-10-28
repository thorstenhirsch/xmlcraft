# XmlCraft parity with Altova XMLSpy

> **Last reviewed:** 2026-10-27 · **Last updated:** 2026-10-27 · **Change:** M1.12: recent files (partial); feature breadth 6.6% shipped of 152 counted · **Target:** Altova XMLSpy 2026 (Professional and Enterprise feature sets as described in Altova's public documentation)

The authoritative assessment of how close XmlCraft is to XMLSpy. [ROADMAP.md](../ROADMAP.md) summarizes it and [gaps.md](gaps.md) lists every shortfall.

**Scope.** Parity means the *desktop editing* feature set. Out of scope and tracked as `na` in `parity/xmlspy-features.toml` (10 entries, excluded from every percentage): the COM/ActiveX API, Visual Studio and Eclipse plug-ins, RaptorXML Server, the Altova AI add-on, database connectors, XBRL, Authentic view, the WSDL/SOAP client, code generation and archive views.

## Headline

| | Value | Kind |
|---|---|---|
| **Feature breadth**, unweighted | **6.6%** shipped, 16.1% with partial counted half (10 shipped + 29 partial of 152 counted; 64 P0, 48 P1, 25 P2, 15 P3) | measured (`cargo xtask parity` arithmetic, 2026-10-27) |
| Must-have (P0) features | 7.8% shipped, 25.8% weighted (5 shipped + 23 partial of 64) | measured |
| W3C xmlconf, not-well-formed | **64.7%** (658 of 1017 applicable tests) | measured (`cargo xtask conformance xmlconf`, 2026-10-27) |
| W3C xmlconf, valid and invalid | **100%** (721 of 721 valid, 227 of 227 invalid): no well-formedness error is reported on a well-formed document | measured (same run) |
| W3C xsdtests (XSD 1.0) | not measured | arrives with M3 |
| Verdict agreement with `xmllint` | not measured | arrives with M2 (`cargo xtask oracle`) |
| **Stage** | **pre-alpha** | |

## How this is measured

- **Feature breadth** is arithmetic over `parity/xmlspy-features.toml`: each feature has a tier (P0–P3), a status (`planned`, `partial`, `shipped`, `na`) and, for `shipped`, evidence that names a passing test. `cargo xtask parity` rejects claims whose commands, tools or tests do not exist.
- **Conformance** is the pass rate of the W3C and OASIS suites, pinned by commit and fetched by `cargo xtask conformance`. The ratchet lists every known failure in `xtask/baselines/`; the pass-rate tables here are copied from the command's output whenever the baseline changes (the valid and invalid totals cannot be derived from the baseline, which lists failures only).
- **"Shipped" means exists and tested**, not "as good as XMLSpy".

## W3C XML conformance (xmlconf, 23 September 2013)

`cargo xtask conformance xmlconf [--fetch]` runs the applicable tests of the suite through the same path the editor uses (encoding detection and decoding, then the parser) and compares the failures with `xtask/baselines/xmlconf.txt`. Applicable means XML 1.0 fifth edition, non-validating, namespace-aware: XML 1.1 tests (266), tests for earlier editions only (312), processor-dependent `error` tests (33) and namespace-unaware tests (9) are skipped. A test counts as passed when the verdict (well-formed or not) is the one the suite expects.

| Kind | Passed | Of | Rate |
|---|---:|---:|---:|
| not-wf | 658 | 1017 | 64.7% |
| valid | 721 | 721 | 100% |
| invalid | 227 | 227 | 100% |

**What the 359 known failures are.** Every one is a not-well-formed test that needs the DTD, which XmlCraft reads from M2: the syntax of `<!ELEMENT>`, `<!ATTLIST>` and `<!ENTITY>` declarations and conditional sections (about 310 tests), and rules that need the declarations or the files behind them: an entity must be declared, parsed, not external inside an attribute value, not recursive, with a well-formed replacement text, and the external subset must itself be well-formed (about 50). The internal subset is lexed and what does not need its declarations is checked (its extent, comments, processing instructions, public identifiers, `&` and `%` in entity values, parameter-entity references inside declarations, unknown declarations), but the declarations are not parsed. They close with the DTD parser (M2.6), the resolver (M2.3) and entity expansion (M2.8); `docs/gaps.md` tracks them as gap 16.

## Revision history

- 2026-10-27: M1.12: documents in the app; 8 shipped, 13 partial (7 shipped and 14 partial once its review had moved the clipboard back to partial).
- 2026-10-27: M1.13: well-formedness on a worker thread, the Messages and Outline panes, F7 and Go to Line. Go to Line went from planned to shipped; the outline, the messages, the well-formedness check and namespace well-formedness went from planned to partial: 8 shipped and 18 partial, from 7 shipped and 14 partial after M1.12 (the clipboard had gone back to partial in its review).
- 2026-10-27: M1.5: first conformance numbers.
- 2026-10-27: created.
- 2026-10-27: M1.11 and M1.14: pretty-print and minify as minimal edits, the document and XML tools (`doc_*`, `xml_wellformed`, `xml_outline`, `xml_format`) and `xmlcraft-cli check` and `format`; 10 shipped and 18 partial.
- 2026-10-27: M1.8: find and replace (bar, options, counter, highlights, `doc_find`/`doc_replace`) and ⌘D multi-cursor: find/replace is partial; 10 shipped and 19 partial.
- 2026-10-27: M1.9a: matching tag highlight and jump (`editor.matching-tags` planned → partial); 10 shipped and 20 partial, 13.2% weighted.
- 2026-10-27: M1.9b: auto-pairing of attribute quotes (`editor.auto-pairing` planned → partial); 10 shipped and 21 partial, 13.5% weighted.
- 2026-10-27: M1.9c: folding (`editor.code-folding` planned → partial); 10 shipped and 22 partial, 13.8% weighted.
- 2026-10-27: M1.10a: auto-close tags, end-tag completion and smart indent (`editor.auto-close-tags` and `editor.auto-indent` planned → partial); 10 shipped and 24 partial, 14.5% weighted.
- 2026-10-27: M1.10b: toggle comment and expand/shrink selection (`editor.comment-toggle` and `editor.expand-selection` planned → partial); 10 shipped and 26 partial, 15.1% weighted.
- 2026-10-27: M1.15: large files (`editor.large-files` planned → partial): colouring far into a very large document starts from an estimate (first jump 1.2 s → 0.23 ms for 100 MB) and `cargo xtask bench` holds the targets; 10 shipped and 27 partial, 15.5% weighted.
- 2026-10-27: M1.10c: tag-rename sync (`editor.tag-rename-sync` planned → partial); 10 shipped and 28 partial, 15.8% weighted.
- 2026-10-28: M1.12: recent files (`editor.recent-files` planned → partial); 10 shipped and 29 partial, 16.1% weighted.
