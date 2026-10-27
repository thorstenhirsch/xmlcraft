# XmlCraft parity with Altova XMLSpy

> **Last reviewed:** 2026-10-27 · **Last updated:** 2026-10-27 · **Change:** M1.12: the editor in the app; feature breadth 5.3% shipped of 152 counted · **Target:** Altova XMLSpy 2026 (Professional and Enterprise feature sets as described in Altova's public documentation)

The authoritative assessment of how close XmlCraft is to XMLSpy. [ROADMAP.md](../ROADMAP.md) summarizes it and [gaps.md](gaps.md) lists every shortfall.

**Scope.** Parity means the *desktop editing* feature set. Out of scope and tracked as `na` in `parity/xmlspy-features.toml` (10 entries, excluded from every percentage): the COM/ActiveX API, Visual Studio and Eclipse plug-ins, RaptorXML Server, the Altova AI add-on, database connectors, XBRL, Authentic view, the WSDL/SOAP client, code generation and archive views.

## Headline

| | Value | Kind |
|---|---|---|
| **Feature breadth**, unweighted | **5.3%** shipped, 9.5% with partial counted half (8 shipped + 13 partial of 152 counted; 64 P0, 48 P1, 25 P2, 15 P3) | measured (`cargo xtask parity` arithmetic, 2026-10-27) |
| Must-have (P0) features | 6.2% shipped, 14.8% weighted (4 shipped + 11 partial of 64) | measured |
| W3C xmlconf, not-well-formed | **64.7%** (658 of 1017 applicable tests) | measured (`cargo xtask conformance xmlconf`, 2026-10-27) |
| W3C xmlconf, valid and invalid | **100%** (721 of 721 valid, 227 of 227 invalid): no well-formedness error is reported on a well-formed document | measured (same run) |
| W3C xsdtests (XSD 1.0) | not measured | arrives with M3 |
| Verdict agreement with `xmllint` | not measured | arrives with M2 (`cargo xtask oracle`) |
| **Stage** | **pre-alpha** | |

## How this is measured

- **Feature breadth** is arithmetic over `parity/xmlspy-features.toml`: each feature has a tier (P0–P3), a status (`planned`, `partial`, `shipped`, `na`) and, for `shipped`, evidence that names a passing test. `cargo xtask parity` rejects claims whose commands, tools or tests do not exist.
- **Conformance** is the pass rate of the W3C and OASIS suites, pinned by commit and fetched by `cargo xtask conformance`. The ratchet lists every known failure in `xtask/baselines/`; the table here is regenerated from it, never edited by hand.
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

- 2026-10-27: M1.12: documents in the app; 8 shipped, 13 partial.
- 2026-10-27: M1.5: first conformance numbers.
- 2026-10-27: created.
