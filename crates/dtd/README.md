# xmlcraft-dtd

The DTD of a document, read (layer L3, no UI, no I/O, compiles for the web).

- `doctype_parts(tree)` finds what a DOCTYPE says: the name, the public and system identifiers, and where its internal subset is.
- `parse_doctype(input, loader)` reads the internal subset and then the external subset the DOCTYPE names. `SubsetLoader` is where the
  external texts (the subset, external parameter entities) come from; the crate never reads a file or a URL. `NoLoader` has nothing.
  Declarations of the internal subset come first, so they win for a name declared twice (XML 1.0 §4.2: the first declaration counts).
- What is read is a `Dtd`: element declarations (EMPTY, ANY, mixed and children content models as a tree of particles), attribute-list
  declarations (types and defaults), general and parameter entities (internal values with character references resolved and
  general-entity references kept; external identifiers; NDATA), notations, each with where it is. `Dtd::incomplete` says that a part
  that was needed could not be read; `has_external_parts` that an external subset or entity exists (so that an undeclared name may be
  declared in what was not read, XML 1.0 §4.1).
- Parameter entities are expanded as §4.4 says: between declarations (also in the internal subset), inside declarations and entity
  values of the external subset (a reference stands for its text with a space at each end), external ones loaded once. Conditional
  sections (INCLUDE, IGNORE, nesting, the keyword from an entity) are for external text only.
- Every syntax error is reported as a `DtdDiagnostic` (a `Code` of `xmlcraft-syntax`, a source and a byte range) and reading goes on
  after it. `problems_in_text` and `problems_of_document` merge these with what the syntax tree found, leaving out what the tree found
  inside the internal subset (so that nothing is reported twice).
- Bounds: parameter entities nest at most 40 deep, expand to at most 10 million characters, 256 external texts and 64 MiB are loaded,
  groups nest at most 256 deep, 1,000,000 declarations are read and 1,000 problems are listed.
- Not here: compiling content models to automata and validating (M2.5 and M2.9), checking entity references against the declarations
  (M2.8), and finding the files (the resolver, M2.3).

Tests: `tests/declarations.rs`; the xmlconf run (`cargo xtask conformance xmlconf`) reads every DTD of the W3C suite through it.
