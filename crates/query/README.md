# xmlcraft-query

XPath 3.1 evaluation for XmlCraft on the `xee` engine (layer L4, no UI, no I/O, compiles for the web).

- `evaluate(xml, expression, &Options)` runs an expression with the document as the context item and returns the items of the result (`ResultItem`): a node has its `kind`, `name` (`Q{uri}local` when it has a namespace), its path as `fn:path` writes it (`/r[1]/i[2]/@n`), its place in the text (`SourceMap`, from the syntax tree of the text; text, references and CDATA are one text node as in the data model) and its text as written; a value has its type (`xs:integer`, …, `map`, `array`, `function`) and its XPath representation. Results are cut at `Options::max_items` (the rest are counted) and each text at `max_text` characters.
- Errors (`QueryError`) have the W3C code (`XPST0003`, `FOAR0001`, …), the phase (`Document`, `Compile`, `Run`) and the byte range in the expression.
- XPath 1.0, 2.0 and 3.0 are subsets of the engine's 3.1 and run under its rules (no 1.0 compatibility mode). XQuery and XSLT are not here yet (the engine's XSLT is too young; the debugger and the profiler need hooks in a vendored copy).
- The engine does not read a DOCTYPE: it is blanked (byte for byte) before the document is read, so internal entities are not expanded.
- On wasm the engine's hash maps get their seeds from the browser (`.cargo/config.toml` selects getrandom's `wasm_js` backend).

Tests: `tests/xpath.rs` (nodes with kinds, paths and places for elements, attributes, text with CDATA, comments, processing instructions, namespaced names, the document; values and their types; errors with phases and places; limits; a document with a DOCTYPE; properties: any expression on any text answers or errors without a panic, every node of a generated document is at the place its text says). The tool is tested in `crates/automation/tests/xpath.rs`.
