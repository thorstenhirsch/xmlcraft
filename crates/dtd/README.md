# xmlcraft-dtd

The DTD of a document, read (layer L3, no UI, no I/O, compiles for the web).

- `doctype_parts(tree)` finds what a DOCTYPE says: the name, the public and system identifiers, and where its internal subset is.
- `parse_doctype(input, loader)` reads the internal subset and then the external subset the DOCTYPE names. `SubsetLoader` is where the
  external texts (the subset, external parameter entities) come from; the crate never reads a file or a URL itself. `NoLoader` has nothing; `ResolvingSubsetLoader` asks the
  resolver (`xmlcraft-resolve`: the open documents, the catalogs, the DTDs that ship with the app by public identifier, files), with a budget of its own and no text over
  `MAX_EXTERNAL_BYTES`.
  Declarations of the internal subset come first, so they win for a name declared twice (XML 1.0 §4.2: the first declaration counts).
- What is read is a `Dtd`: element declarations (EMPTY, ANY, mixed and children content models as a tree of particles), attribute-list
  declarations (types and defaults), general and parameter entities (internal values with character references resolved and
  general-entity references kept; external identifiers; NDATA), notations, each with where it is. `Dtd::incomplete` says that a part
  that was needed could not be read; `has_external_parts` that the DOCTYPE names an external subset or an external parameter entity was
  loaded, so that a name which is not declared may be declared in what is outside the document (XML 1.0 §4.1).
- Parameter entities are expanded as §4.4 says: between declarations (also in the internal subset), inside declarations and entity
  values of the external subset (a reference stands for its text with a space at each end), external ones loaded once. Conditional
  sections (INCLUDE, IGNORE, nesting, the keyword from an entity) are for external text only.
- Syntax errors are reported as `DtdDiagnostic`s (a `Code` of `xmlcraft-syntax`, a source and a byte range), up to a limit of 1,000
  (then one note says there are more), and reading goes on after each. `problems_in_text` and `problems_of_document` merge these with what the syntax tree found, leaving out what the tree found
  inside the internal subset (so that nothing is reported twice).
- Bounds: parameter entities nest at most 40 deep, expand to at most 10 million bytes, 256 external texts and 64 MiB are loaded,
  groups nest at most 256 deep, 1,000,000 declarations are read and 1,000 problems plus a note that there are more are listed.
- `check_references(root, dtd, location, loader)` applies the well-formedness rules that tie references to general entities to the
  declarations, in the document and in the default values of attributes: the entity is declared (where the document has all of its
  declarations, and always when it is standalone; in a default value, before), parsed, not external inside an attribute value, with no
  `<` in an attribute value, not recursive, and its replacement text is well-formed content where it is used. An external entity in
  text is read through the loader: its text declaration and its content are checked, and what cannot be read is a warning. Nothing is
  expanded: each entity is analysed once per place it can be used and the verdict is remembered, so a billion-laughs DTD costs what
  its declarations cost; entities refer to entities at most 256 deep. Not done: attribute-value normalisation, and a namespace
  declaration whose value is written with entities.
- Not here: compiling content models to automata and validating (M2.5 and M2.9), and downloading what nothing has (M2.4).

Tests: `tests/declarations.rs`, `tests/references.rs`; the xmlconf run (`cargo xtask conformance xmlconf`) reads every DTD of the W3C suite through it.
