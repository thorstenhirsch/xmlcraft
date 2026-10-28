# Where XmlCraft falls short of XMLSpy

> **Last reviewed:** 2026-10-28 · **Last updated:** 2026-10-28 · **Change:** gap 2: the resolver exists as a library, gap 2 reworded (catalog rules exist), gap 16 closed (xmlconf 100%, M2.9), gap 18 narrowed (large-file colouring, M1.15), gap 19 added (content-model automata, M2.5), gap 20 added (Entry Helpers, M2.11) · **Target:** Altova XMLSpy 2026 (public documentation)

The ranked gaps. Prefer fixing these over adding new features. Every gap states what is missing, why it matters and how it will be measured. At 0.0.1 the application does not exist yet, so the first entries are simply "the MVP".

## Summary

| # | Gap | Priority | Closes with |
|---|---|---|---|
| 1 | The editor lacks autosave recovery, file watching while the window is not focused and name completion (open, edit, colour, save, undo, find/replace, folding, matching tags, typing aids, tag-rename sync, comment toggle, selection expansion and formatting exist) | P0 | M1 |
| 2 | No resolver that asks the catalogs (the catalog rules exist: M2.1, and ten W3C files with catalogs are bundled: M2.2), so a document's external DTD is found only as a file next to it; most of the core standard schemas are not bundled; nothing is downloaded or cached | P0 | M2 |
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
| 17 | The analysis is whole-document and per edit: results can be a little behind the text, underlines are clamped (not shifted) through later edits, and a document over 32 MB is not analysed | P1 | M1.15 |
| 18 | Colour at a far place in a text over 1 MiB is an estimate: a construct (comment, CDATA section, tag, …) that opens more than 256 KiB before it is coloured as content until it ends | P2 | M1.15 |
| 19 | Content-model automata unroll counted occurrences (at most 100,000 positions), have no XML Schema `all` group, and say what is mandatory only where there are at most 32 things to choose from | P1 | M3.7 |
| 20 | Entry Helpers offer what the DTD allows after the children before the caret, from the DOCTYPE only; there is no completion popup while typing, and a DTD file that changed on disk is not read again | P1 | M1.10d, M3.12 |

## The gaps

**11. Fonts for other scripts.** The interface and the editor embed Inter and JetBrains Mono (Latin, Greek, Cyrillic) behind egui's own fallback faces. XML documents routinely contain CJK, Arabic, Hebrew and Indic text, and those characters have no face yet. The plan is PdfCraft's approach: one font already installed on the user's machine, read at run time as the last fallback (never embedded or shipped, AGENTS.md §1.4), with its glyphs moved onto Inter's baseline. Measured by a glyph-coverage test and a screenshot with synthetic multi-script text.

**12. Tab strip.** Tabs have a fixed natural width; many open documents will overflow. PdfCraft's strip shrinks tabs, then scrolls with arrows, and lets a tab be dragged to a new position. It arrives with the document lifecycle (M1.12), when there are documents to drag.
**17. Incremental analysis.** Every edit asks for a full parse of the text on a worker thread (only the newest request per document is kept). Until the answer arrives the window shows the previous result, and its underlines are clamped to the text rather than moved through the edits made since, so a squiggle can sit beside the wrong word for a moment. A document over 32 MB is not analysed at all. Large-file mode and an incremental reparse (M1.15, driven by `cargo xtask bench`) close it; measured by the keystroke-to-paint and reparse targets in `docs/target-app-parity.md`.

**18. Colouring far into a very large document.** The colour of a line depends on where the lexer is at its start (inside a comment, a tag, a value), which is known only by reading everything before it; the first jump to the end of a 100 MB document used to read it all (1.2 s against the target of 16 ms). Above 1 MiB the highlighter now starts a far place from a guess: it runs the lexer over the 256 KiB before the line (counted in bytes, from the first line that starts inside that, so a very long line cannot widen it), from the state of content. That finds everything the lexer knows that begins in the window: a comment, a CDATA section, a processing instruction, a tag, an attribute value or a declaration that goes over lines, and what a comment inside a DOCTYPE returns to. For a text that fits the window it is exact (property test `estimating_in_a_text_that_fits_the_window_is_exact_whatever_goes_over_lines`). What is left: a construct that began before the window and has not ended (a comment or CDATA section of more than 256 KiB, say) is not seen, and the lines in it are coloured as content until it ends, and then they are right again. The first jump is 0.9 ms for 100 MB (`scroll_100mb_far`, budget 16 ms). The other half of large-file mode is not done: the outline and the validation of a document over 32 MB are not streamed (gap 17), and the view finds the width of a document for its horizontal scroll in one pass up to 256 MB (45 ms for 100 MB, `widest_line_100mb`) and by the lines the carets visit beyond that.

**19. Content-model automata.** `xmlcraft-grammar` (M2.5) compiles a content model to a Glushkov automaton that steps, says what may come next, finishes the shortest way and checks determinism, and agrees with a reference matcher on random models. Three things are missing for XML Schema: a counted occurrence (`maxOccurs="1000"`) is unrolled into copies, so a model over 100,000 positions (or one whose follow lists would be huge, such as 50,000 optional copies of an optional element) is refused, where XSDs in the wild sometimes use `maxOccurs="100000"` or more; the `all` group (each child at most once, in any order) is not a particle yet; and `mandatory` in `expected` is worked out only where there are at most 32 different things to choose from. Counters instead of unrolling, the `all` automaton and an exact `mandatory` come with the XSD compiler (M3.7). Measured by the reference test in `crates/grammar/tests/reference.rs` and by xsdtests from M3.

**20. Entry Helpers.** The pane (M2.11) lists, from the DTD of the DOCTYPE, the elements the content model allows after the children that come before the caret, the attributes of the element the caret is in and the entities. Three things are missing. The list looks only at what comes before the caret: an element that fits there may still make what follows invalid (an element is offered between two siblings without asking whether the second would still be accepted). The DTD is read when the DOCTYPE (or the document's place) changes, not when a file it names changes on disk, and it is not cached between documents. There is no popup in the editor while typing (`<`, a space in a tag, `="`, `&`): that is the name-completion work of M1.10d, which will share `xmlcraft-complete`. The XSD as a source comes with M3.12. Measured by `crates/complete/tests/choices.rs`, `crates/ui-egui/tests/entry_helpers.rs` and `crates/automation/tests/entry_helpers.rs`.

Details are added here as each gap is worked, including known approximations (for example in XSD particle restriction) and the test that measures them. Nothing may be marked closed here without a cited test.

## Revision history

- 2026-10-27: created.
- 2026-10-27: gap 16 added with the first xmlconf numbers (M1.5); the M1 exit criterion for not-well-formed tests now covers only what needs no DTD.
- 2026-10-27: gap 17 added with M1.13 (analysis is whole-document and may lag the text).
- 2026-10-27: gap 18 added with `cargo xtask bench` (colouring far into a very large document takes 1.2 s for 100 MB).
- 2026-10-27: gap 1 reworded (find/replace, folding, matching tags, typing aids and formatting have shipped); gap 18 narrowed to the estimate's rare wrong colour after the far-jump start (M1.15).
- 2026-10-27: gap 1: tag-rename sync shipped (M1.10c).
- 2026-10-27: gap 1: column selection shipped (M1.8).
- 2026-10-28: gap 1: recent files shipped (M1.12); a changed file is noticed when the window gets the focus back, not while it is watched.
- 2026-10-28: gap 2: the OASIS catalog resolution rules exist (`xmlcraft-catalog`, M2.1), but nothing asks them yet.
- 2026-10-28: gap 16 narrowed from 359 to 53 tests by the DTD reader (M2.6); the rest need entity references checked against the declarations (M2.8).
- 2026-10-28: gap 16 narrowed from 53 to 2 tests by the entity-reference rules (M2.8); the two left need attribute normalisation (M2.9).
- 2026-10-28: gap 19 added: the content-model automata (M2.5) have the three limits it names.
- 2026-10-28: gap 16 closed: the last two xmlconf not-well-formed tests (namespace declarations read as the DTD makes them) pass with M2.9; the numbers are 100% for not-wf, valid and invalid.
- 2026-10-28: gap 2 reworded: DTD support exists (M2.6–M2.9) and ten W3C files are bundled with catalogs (M2.2); what is left is the resolver, the rest of the bundle and the download.
- 2026-10-28: gap 20 added: Entry Helpers from the DTD (M2.11) and what they do not do yet.
- 2026-10-28: gap 2: `xmlcraft-resolve` (M2.3) is the resolver, with the cache rules and the download policy, but no HTTP client, no disk cache and no wiring into the DTD loader yet (M2.4).
