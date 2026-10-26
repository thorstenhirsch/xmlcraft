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
| 11 | Text outside Latin, Greek and Cyrillic has no interface or editor font (CJK, Arabic, Hebrew, Indic scripts draw as boxes) | P1 | M1 |
| 12 | The tab strip neither scrolls nor reorders by drag when many documents are open | P1 | M1.12 |
| 13 | No Preferences ▸ Animation effects, and the system "reduce motion" request is not read | P3 | M6+ |
| 14 | No window buttons drawn by the app on GNOME/Wayland (the desktop frame is used everywhere except macOS's integrated bar) | P2 | M6+ |
| 15 | No log file, only standard error | P2 | M1 |

## The gaps

**11. Fonts for other scripts.** The interface and the editor embed Inter and JetBrains Mono (Latin, Greek, Cyrillic) behind egui's own fallback faces. XML documents routinely contain CJK, Arabic, Hebrew and Indic text, and those characters have no face yet. The plan is PdfCraft's approach: one font already installed on the user's machine, read at run time as the last fallback (never embedded or shipped, AGENTS.md §1.4), with its glyphs moved onto Inter's baseline. Measured by a glyph-coverage test and a screenshot with synthetic multi-script text.

**12. Tab strip.** Tabs have a fixed natural width; many open documents will overflow. PdfCraft's strip shrinks tabs, then scrolls with arrows, and lets a tab be dragged to a new position. It arrives with the document lifecycle (M1.12), when there are documents to drag.

Details are added here as each gap is worked, including known approximations (for example in XSD particle restriction) and the test that measures them. Nothing may be marked closed here without a cited test.

## Revision history

- 2026-10-27: created.
