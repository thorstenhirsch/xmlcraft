# xmlcraft-yaml-syntax

The YAML syntax layer (layer L1, no UI, no I/O, compiles for the web): a tolerant reader that keeps the place of every value, and a line highlighter.

- **Reading** (`parse`): the events of `saphyr-parser` (YAML 1.2) become the value tree of `xmlcraft-json-syntax` (`Value`, `Data`, `Member`), so pointers, spans, JSON Schema, conversion and the outline treat YAML and JSON alike. A stream gives one value per document. A text that cannot be read to the end still gives the part that was read, and a `yaml.syntax-error` where it stopped (the byte span, not the parser's character index).
- **Scalars** follow the YAML 1.2 core schema: `null`/`~`/empty, `true`/`false`, integers (also `0x` and `0o`, written in decimal), floats, `.inf`, `.nan`; everything else, and every quoted or block scalar, is a string (`yes` and `on` are strings). The tags `!!str`, `!!int`, `!!float`, `!!bool` and `!!null` are applied.
- **Aliases** are copies of the anchored value with every place set to the alias, and the expansion is bounded (`Options::max_nodes`, the defence against nested-alias bombs). **Merge keys** (`<<`) bring in the members a mapping does not have; its own keys win whatever the order.
- **Limits:** nesting (`max_depth`), values made by aliases (`max_nodes`) and the number of problems (`max_diagnostics`) are capped, each with a problem that says so; repeated keys are a warning.
- **Colouring** (`highlight_line`): classes per line (`JsonClass`: key, string, number, keyword for null/booleans/anchors/aliases/tags, comment, punctuation) with the `Mode` that carries an open block scalar or an unfinished quoted scalar to the next line.

Tests: `tests/reading.rs` (places, scalars, aliases, merges, errors, streams, the caps; random text and text made of YAML pieces never panic and keep places inside the text), `tests/highlighting.rs`.
