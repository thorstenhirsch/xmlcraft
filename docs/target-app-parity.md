# XmlCraft parity with Altova XMLSpy

> **Last reviewed:** 2026-10-29 · **Last updated:** 2026-10-29 · **Change:** M4.4 and M4.6: the two Generate dialogs (commands and evidence for the eight `generate.schema-*` and `generate.sample-*` features; no status changes), M4.5–M4.6 (headless): the four `generate.sample-*` features planned → partial (10 shipped and 54 partial, 24.3% weighted), M4.1–M4.4 (headless): the four `generate.schema-*` features planned → partial (10 shipped and 50 partial, 23.0% weighted), M3.15 (first part): `xml_validate` and `xmlcraft-cli validate --schema` against an XSD (`validate.xsd` and `validate.schema-association` planned → partial; 10 shipped and 46 partial, 21.7% weighted), M3.4–M3.9: the XSD engine (notes of `xsd.*` and `validate.xsd`, no status changes), M3.3: the schema document reader (a note only), M2.3 (first part): the resolver library (notes only), M3.2: XSD datatypes (notes only), M2.11: Entry Helpers from the DTD (`view.entry-helpers` planned → partial), M3.1: XSD regular expressions (a note only), M2.2 (first part): bundled schemas (`catalog.bundled-schemas` planned → partial), M2.9: validation against a DTD (partial); xmlconf now validating: not-wf, valid and invalid all 100%; feature breadth 6.6% shipped of 152 counted · **Target:** Altova XMLSpy 2026 (Professional and Enterprise feature sets as described in Altova's public documentation)

The authoritative assessment of how close XmlCraft is to XMLSpy. [ROADMAP.md](../ROADMAP.md) summarizes it and [gaps.md](gaps.md) lists every shortfall.

**Scope.** Parity means the *desktop editing* feature set. Out of scope and tracked as `na` in `parity/xmlspy-features.toml` (10 entries, excluded from every percentage): the COM/ActiveX API, Visual Studio and Eclipse plug-ins, RaptorXML Server, the Altova AI add-on, database connectors, XBRL, Authentic view, the WSDL/SOAP client, code generation and archive views.

## Headline

| | Value | Kind |
|---|---|---|
| **Feature breadth**, unweighted | **6.6%** shipped, 24.3% with partial counted half (10 shipped + 54 partial of 152 counted; 64 P0, 48 P1, 25 P2, 15 P3) | measured (`cargo xtask parity` arithmetic, 2026-10-29) |
| Must-have (P0) features | 7.8% shipped, 40.6% weighted (5 shipped + 42 partial of 64) | measured |
| W3C xmlconf, not-well-formed | **100%** (1017 of 1017 applicable tests) | measured (`cargo xtask conformance xmlconf`, 2026-10-28) |
| W3C xmlconf, valid and invalid | **100%** (721 of 721 valid, 227 of 227 invalid): a valid document has no well-formedness or validity error, an invalid one is well-formed and has at least one validity error (which one is not compared; 73 of the 227 are documents with no DOCTYPE, invalid because there is nothing to validate against) | measured (same run) |
| W3C xsdtests (XSD 1.0) | not measured | arrives with M3 |
| Verdict agreement with `xmllint` | not measured | arrives with M2 (`cargo xtask oracle`) |
| **Stage** | **pre-alpha** | |

## How this is measured

- **Feature breadth** is arithmetic over `parity/xmlspy-features.toml`: each feature has a tier (P0–P3), a status (`planned`, `partial`, `shipped`, `na`) and, for `shipped`, evidence that names a passing test. `cargo xtask parity` rejects claims whose commands, tools or tests do not exist.
- **Conformance** is the pass rate of the W3C and OASIS suites, pinned by commit and fetched by `cargo xtask conformance`. The ratchet lists every known failure in `xtask/baselines/`; the pass-rate tables here are copied from the command's output whenever the baseline changes (the valid and invalid totals cannot be derived from the baseline, which lists failures only).
- **"Shipped" means exists and tested**, not "as good as XMLSpy".

## W3C XML conformance (xmlconf, 23 September 2013)

`cargo xtask conformance xmlconf [--fetch]` runs the applicable tests of the suite through the same path the editor uses (encoding detection and decoding, then the parser, the DTD reader and the validator, with the external parts of the DTD read from the suite's files) and compares the failures with `xtask/baselines/xmlconf.txt`. Applicable means XML 1.0 fifth edition, namespace-aware: XML 1.1 tests (266), tests for earlier editions only (312), processor-dependent `error` tests (33) and namespace-unaware tests (9) are skipped. A not-well-formed test passes when a well-formedness error is reported; a valid test when no well-formedness or validity error is reported; an invalid test when the document is well-formed and a validity error is reported. External general entities are not read in content (their content is taken as unknown), and the validity errors are not compared with the test's section, only counted.

| Kind | Passed | Of | Rate |
|---|---:|---:|---:|
| not-wf | 1017 | 1017 | 100% |
| valid | 721 | 721 | 100% |
| invalid | 227 | 227 | 100% |

The baseline is empty. How the 359 not-well-formed tests that needed the DTD closed: the DTD reader (M2.6, 306), the rules that tie references to declarations (M2.8, 51: an entity must be declared where the document has all of its declarations and always when standalone, parsed, not external inside an attribute value, without `<` in one, not recursive, with a replacement text that is well-formed where it is used) and the namespace declarations read as the DTD makes them (M2.9, 2: entities in the value of a declaration, and a type that trims it). The invalid tests are caught by: no DOCTYPE (73), an element that is not declared or not complete or not allowed (about 50), attribute declarations and values (about 50: types, defaults, enumerations, ID and IDREF, ENTITY, required, fixed), the standalone document declaration (14), the nesting of parameter entities in declarations, groups and conditional sections (9), and the validity constraints on the declarations (duplicates, ID and NOTATION rules, undeclared notations; about 20).

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
- 2026-10-27: M1.8: column selection (`editor.column-selection` planned → partial); 10 shipped and 29 partial, 16.1% weighted.
- 2026-10-28: M1.12: recent files (`editor.recent-files` planned → partial); 10 shipped and 30 partial, 16.4% weighted.
- 2026-10-28: M2.1: OASIS XML Catalogs 1.1 resolution (`catalog.oasis-catalogs` planned → partial); 10 shipped and 31 partial, 16.8% weighted.
- 2026-10-28: M2.6: the DTD reader (six `dtd.*` features planned → partial); xmlconf not-well-formed 64.7% → 94.8%; 10 shipped and 37 partial, 18.8% weighted.
- 2026-10-28: M2.8: entity references checked against the declarations (`dtd.general-entities` planned → partial); xmlconf not-well-formed 94.8% → 99.8%; 10 shipped and 38 partial, 19.1% weighted.
- 2026-10-28: M2.7: the DTD compiled to a grammar (`dtd.determinism-warning` planned → partial; the notes of `dtd.content-models`, `dtd.attlist` and `dtd.notations` say what the compile checks); 10 shipped and 39 partial, 19.4% weighted.
- 2026-10-28: M2.9: validation against a DTD (`validate.dtd` and `validate.entity-limits` planned → partial); xmlconf now validating, not-well-formed 99.8% → 100%, valid and invalid 100%; 10 shipped and 41 partial, 20.1% weighted.
- 2026-10-28: M2.10 and M2.13 (first parts): Validate (F8) in the app, the tool `xml_validate` and `xmlcraft-cli validate` (`validate.dtd` gets its commands and tools, `validate.error-positions` planned → partial); 10 shipped and 42 partial, 20.4% weighted.
- 2026-10-28: M3.1: `xmlcraft-pattern` (XSD regular expressions); the note of `xsd.regex-patterns` says what exists; the status stays planned (nothing a user can reach uses it yet).
- 2026-10-28: M2.2 (first part): the bundled schemas, ten W3C files with their catalogs (`catalog.bundled-schemas` planned → partial); 10 shipped and 43 partial of 152 counted, 20.7% weighted, P0 33.6%.
- 2026-10-28: M2.11: Entry Helpers from the DTD and the tool `xml_entry_helpers` (`view.entry-helpers` planned → partial); 10 shipped and 44 partial of 152 counted, 21.1% weighted, P0 34.4%.
- 2026-10-28: M2.3 (first part): `xmlcraft-resolve` (the chain, the cache rules, the policy); notes of `catalog.auto-download`, `catalog.disk-cache`, `catalog.offline-mode`, `catalog.user-catalogs` and `catalog.resolution-trail` say what exists; no status changes (nothing is reachable by a user yet).
- 2026-10-28: M3.2: `xmlcraft-datatypes`; the notes of `xsd.simple-types` and `xsd.date-time-types` say what exists; the statuses stay planned (nothing a user can reach uses it yet).
- 2026-10-28: M3.3: `xmlcraft-xsd::read`; the note of `xsd.read-1-0` says what exists; the status stays planned (no tool or command reaches it yet).
- 2026-10-28: M3.4–M3.9 (first part): the XSD engine (`xmlcraft-xsd` assembles and compiles, `xmlcraft-validate` validates against a schema); notes of `xsd.complex-types`, `xsd.groups`, `xsd.wildcards`, `xsd.substitution-groups`, `xsd.identity-constraints`, `xsd.include-import-redefine`, `xsd.upa-check`, `xsd.component-constraints` and `validate.xsd` say what exists and what is missing; no status changes (nothing a user can reach calls it yet).
- 2026-10-29: M3.15 (first part): validation against an XML Schema from a tool (`validate.xsd` and `validate.schema-association` planned → partial); 10 shipped and 46 partial, 21.7% weighted.
- 2026-10-29: M4.1–M4.4 (headless): schema inference (`generate.schema-xsd`, `generate.schema-dtd`, `generate.schema-options` and `generate.schema-multi-file` planned → partial); 10 shipped and 50 partial, 23.0% weighted.
- 2026-10-29: M4.5–M4.6 (headless): sample generation (`generate.sample-xsd`, `generate.sample-dtd`, `generate.sample-options` and `generate.sample-pattern-values` planned → partial); 10 shipped and 54 partial, 24.3% weighted.
- 2026-10-29: M4.4 and M4.6 (the dialogs): the `generate.schema-*` and `generate.sample-*` features get their commands (`xml.generate_schema`, `xml.generate_sample`) and UI evidence; still partial (what is missing is in their notes); 10 shipped and 54 partial, 24.3% weighted.
