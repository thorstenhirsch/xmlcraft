# xmlcraft-xquery

XQuery 3.1 for XmlCraft (layer L4, no UI, no I/O, compiles for the web). `xee` has XPath 3.1 but no XQuery, so this crate reads
XQuery itself and compiles it to `xee`'s intermediate representation; the vendored `xee-interpreter` runs it like an XPath
expression. The decision and the vendored patches it needs are in `docs/architecture.md` ▸ Query engines.

- `parse(query, &Options)` reads a module into the tree of `ast` (a byte span on every expression and clause, names resolved,
  types and node tests as `xee`'s own) or returns the first static error with its W3C code and place (`XPST0003` with the tokens
  that would have fitted).
- `evaluate(query, context, &Options)` compiles and runs a query, with an XML document as the context item when there is one.
  The result is described as XPath's is (`xmlcraft_query::ResultItem`): nodes of the document with their path and place, nodes
  the query built with neither (serialized). `Options::serialize` also writes the result with the XML, text, JSON or adaptive
  method. External variables come from `Options::variables`, documents `fn:doc` may open from `Options::documents` (nothing is
  read from files or the network). A run is stopped after `Options::max_steps` interpreter steps (`XMLC0001`).
- `check(query, &Options)` compiles without running.
- `prelude.rs` holds helper functions in XQuery (keys, sort passes, grouping, windows, constructor checks) compiled with every
  query in an internal namespace.

Tests: `tests/parse.rs` (every construct of the grammar, written from the specification's prose; static errors with their codes; places and expected tokens of syntax errors; spans nested in their parents and parsing again; the limits; properties: no input and no mutation of a valid query panics), `tests/slice.rs` (XPath through XQuery, a FLWOR, a direct constructor, error codes and places).
