# xmlcraft-json-syntax

The JSON syntax layer (layer L1, no UI, no I/O, compiles for the web): a tolerant reader that keeps the place of every value, and a line highlighter.

- **Dialects** (`Dialect`): strict JSON (RFC 8259), JSON with comments and trailing commas (`Jsonc`), and JSON5. What the dialect does not allow is *reported* (`json.comment-not-allowed`, `json.trailing-comma`, `json.single-quotes`, `json.unquoted-key`, `json.invalid-number`, …) and the text is still read, so the editor keeps working on a document with a comment in it.
- **Reading** (`parse`): returns a `Value` tree and the problems. The tree goes as far as the text is readable (an unclosed object keeps its members); every `Value` has its byte `span`, every member `Member::key_span` and `value`. Strings are decoded (`\u` pairs, JSON5 escapes; a lone surrogate becomes U+FFFD and is reported); numbers are kept as written so no digit is lost. A repeated name is a warning at the second name.
- **Places**: `Value::pointer("/a/0")`, `Value::span_of_pointer` (where to put the caret for a JSON Pointer; a member is shown at its name) and `Value::path_at(offset)` (the path of the value under a byte offset).
- **JSON Lines** (`parse_lines`): one value per line, each with spans in the whole text; a blank line is nothing and a broken line does not stop the next.
- **Limits**: nesting is capped (`Options::max_depth`, `json.too-deep`; the rest of a too-deep value is skipped without recursion) and so are the problems (`Options::max_diagnostics`, then `too-many-diagnostics`).
- **Colouring** (`highlight_line`): classes per line (`JsonClass`) with the `Mode` that carries an open `/* … */` to the next line.

Tests: `tests/reading.rs` (the tree and its places, escapes, numbers per dialect, what each dialect refuses, broken text, limits, JSON Lines; properties: whatever `serde_json` writes is read back equal with no problem and with nested spans, and no text panics or yields a place outside the text), `tests/highlighting.rs`.
