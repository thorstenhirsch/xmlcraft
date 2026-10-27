# Where XmlCraft falls short of XMLSpy

> **Last reviewed:** 2026-10-27 · **Last updated:** 2026-10-27 · **Change:** gap 1 reworded (tag-rename sync shipped), gap 18 narrowed (large-file colouring, M1.15) · **Target:** Altova XMLSpy 2026 (public documentation)

The ranked gaps. Prefer fixing these over adding new features. Every gap states what is missing, why it matters and how it will be measured. At 0.0.1 the application does not exist yet, so the first entries are simply "the MVP".

## Summary

| # | Gap | Priority | Closes with |
|---|---|---|---|
| 1 | The editor lacks recent files, autosave recovery, file watching, column selection and name completion (open, edit, colour, save, undo, find/replace, folding, matching tags, typing aids, tag-rename sync, comment toggle, selection expansion and formatting exist) | P0 | M1 |
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
| 16 | 359 of 1017 not-well-formed xmlconf tests are accepted (DTD declaration syntax and entity rules) | P0 | M2.3, M2.6, M2.8 |
| 17 | The analysis is whole-document and per edit: results can be a little behind the text, underlines are clamped (not shifted) through later edits, and a document over 32 MB is not analysed | P1 | M1.15 |
| 18 | Colour at a far place in a text over 1 MiB is an estimate: a comment or CDATA section that opens more than 256 KiB before it is coloured as content until reading reaches its start | P2 | M1.15 |

## The gaps

**11. Fonts for other scripts.** The interface and the editor embed Inter and JetBrains Mono (Latin, Greek, Cyrillic) behind egui's own fallback faces. XML documents routinely contain CJK, Arabic, Hebrew and Indic text, and those characters have no face yet. The plan is PdfCraft's approach: one font already installed on the user's machine, read at run time as the last fallback (never embedded or shipped, AGENTS.md §1.4), with its glyphs moved onto Inter's baseline. Measured by a glyph-coverage test and a screenshot with synthetic multi-script text.

**12. Tab strip.** Tabs have a fixed natural width; many open documents will overflow. PdfCraft's strip shrinks tabs, then scrolls with arrows, and lets a tab be dragged to a new position. It arrives with the document lifecycle (M1.12), when there are documents to drag.

**16. DTD declarations and entities in the well-formedness check.** `cargo xtask conformance xmlconf` accepts 359 of the 1017 applicable not-well-formed tests (64.7% rejected, 100% of valid and invalid documents accepted). All are in the DTD: declaration syntax (`<!ELEMENT>` content models, `<!ATTLIST>` types and defaults, `<!ENTITY>` forms, conditional sections, name tokens), entity rules (an entity must be declared, parsed, not external inside an attribute value, not recursive, with well-formed replacement text) and external subsets (which must be read, through the resolver of M2.3, and be well-formed themselves). The lexer and parser keep the internal subset's extent and check what does not need the declarations; the declarations are read by the DTD parser (M2.6) and the entity expansion with its caps (M2.8), whose diagnostics join the document's. The ratchet file `xtask/baselines/xmlconf.txt` lists them and shrinks as they close; the M1 exit criterion "100% of not-well-formed" moves to M2 for these reasons.

**17. Incremental analysis.** Every edit asks for a full parse of the text on a worker thread (only the newest request per document is kept). Until the answer arrives the window shows the previous result, and its underlines are clamped to the text rather than moved through the edits made since, so a squiggle can sit beside the wrong word for a moment. A document over 32 MB is not analysed at all. Large-file mode and an incremental reparse (M1.15, driven by `cargo xtask bench`) close it; measured by the keystroke-to-paint and reparse targets in `docs/target-app-parity.md`.

**18. Colouring far into a very large document.** The colour of a line depends on where the lexer is at its start (inside a comment, a tag, a value), which is known only by reading everything before it; the first jump to the end of a 100 MB document used to read it all (1.2 s against the target of 16 ms). Above 1 MiB the highlighter now starts a far place from a guess: it looks 256 KiB back for a `<!--` or `<![CDATA[` that has no closer after it, and lexes from there. The guess is exact when no comment or CDATA section runs over lines for more than that window (property test `estimating_agrees_with_reading_everything_when_no_construct_goes_over_lines`); otherwise the lines are coloured wrongly until reading reaches the opener, and then they are corrected. The first jump is 0.23 ms for 100 MB (`scroll_100mb_far`, budget 16 ms), and the first frame reads the line count and the longest line in 30 ms (`first_frame_100mb`). What is left is the wrong colour in that rare case, and the other half of large-file mode: the outline and the validation of a document over 32 MB, which are not streamed yet (gap 17).

Details are added here as each gap is worked, including known approximations (for example in XSD particle restriction) and the test that measures them. Nothing may be marked closed here without a cited test.

## Revision history

- 2026-10-27: created.
- 2026-10-27: gap 16 added with the first xmlconf numbers (M1.5); the M1 exit criterion for not-well-formed tests now covers only what needs no DTD.
- 2026-10-27: gap 17 added with M1.13 (analysis is whole-document and may lag the text).
- 2026-10-27: gap 18 added with `cargo xtask bench` (colouring far into a very large document takes 1.2 s for 100 MB).
- 2026-10-27: gap 1 reworded (find/replace, folding, matching tags, typing aids and formatting have shipped); gap 18 narrowed to the estimate's rare wrong colour after the far-jump start (M1.15).
