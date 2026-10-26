# XmlCraft parity with Altova XMLSpy

> **Last reviewed:** 2026-10-27 · **Last updated:** 2026-10-27 · **Change:** initial; no measurements yet · **Target:** Altova XMLSpy 2026 (Professional and Enterprise feature sets as described in Altova's public documentation)

The authoritative assessment of how close XmlCraft is to XMLSpy. [ROADMAP.md](../ROADMAP.md) summarizes it and [gaps.md](gaps.md) lists every shortfall.

**Scope.** Parity means the *desktop editing* feature set. Out of scope and tracked as `na` in `parity/xmlspy-features.toml` (excluded from every percentage): Windows-only integration (COM/ActiveX, Visual Studio and Eclipse plug-ins), commercial server products (RaptorXML Server, FlowForce), the Altova AI add-on, database connectors and XBRL.

## Headline

| | Value | Kind |
|---|---|---|
| Feature breadth, tier-weighted | not measured | arrives with `cargo xtask parity` (M0.5) |
| W3C xmlconf, not-well-formed | not measured | arrives with M1 |
| W3C xmlconf, valid and invalid | not measured | arrives with M2 |
| W3C xsdtests (XSD 1.0) | not measured | arrives with M3 |
| Verdict agreement with `xmllint` | not measured | arrives with M2 (`cargo xtask oracle`) |
| **Stage** | **pre-alpha** | |

## How this is measured

- **Feature breadth** is arithmetic over `parity/xmlspy-features.toml`: each feature has a tier (P0–P3), a status (`planned`, `partial`, `shipped`, `na`) and, for `shipped`, evidence that names a passing test. `cargo xtask parity` rejects claims whose commands, tools or tests do not exist.
- **Conformance** is the pass rate of the W3C and OASIS suites, pinned by commit and fetched by `cargo xtask conformance`. The ratchet lists every known failure in `xtask/baselines/`; the table here is regenerated from it, never edited by hand.
- **"Shipped" means exists and tested**, not "as good as XMLSpy".

## Revision history

- 2026-10-27: created.
