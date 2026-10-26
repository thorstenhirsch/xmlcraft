# Where XmlCraft falls short of XMLSpy

> **Last reviewed:** 2026-10-27 · **Last updated:** 2026-10-27 · **Change:** initial list from the approved plan; nothing is built yet · **Target:** Altova XMLSpy 2026 (public documentation)

The ranked gaps. Prefer fixing these over adding new features. Every gap states what is missing, why it matters and how it will be measured. At 0.0.1 the application does not exist yet, so the first entries are simply "the MVP".

## Summary

| # | Gap | Priority | Closes with |
|---|---|---|---|
| 1 | No editor: nothing can be opened, edited or saved | P0 | M1 |
| 2 | No DTD or catalog support, no offline standard schemas | P0 | M2 |
| 3 | No XSD engine, so no schema validation | P0 | M3 |
| 4 | No schema inference or sample generation | P0 | M4 |
| 5 | No JSON or YAML support | P1 | M5 |
| 6 | No XPath/XQuery, XSLT or debugger | P2 | M6+ |
| 7 | No Grid view, graphical schema editor or Authentic-style forms view | P2 | M6+ |
| 8 | No diff/merge, projects, database or web-service tooling | P3 | M6+ |
| 9 | No XSD 1.1 (assertions, alternatives, openContent) | P2 | M6+ |
| 10 | English only, no translations | P3 | M6+ |

## The gaps

Details are added here as each gap is worked, including known approximations (for example in XSD particle restriction) and the test that measures them. Nothing may be marked closed here without a cited test.

## Revision history

- 2026-10-27: created.
