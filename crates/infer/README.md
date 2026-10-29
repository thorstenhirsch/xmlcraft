# xmlcraft-infer

An XML Schema or a DTD generated from one or more documents (layer L4, no UI, no I/O, compiles for the web).

- `infer(documents, &Options) -> Result<Generated, InferError>`: the documents must be well-formed; the result is a list of files. An
  XML Schema is one file per namespace (`schema.xsd` when there is one; otherwise named after the namespaces, importing each other), a DTD is
  one file (`schema.dtd`).
- **Layouts** of an XML Schema: *Russian doll* (roots are global, the rest is declared inside the parents; an element that is a root, has
  more than one parent or is in another namespace than its parent is global and referred to, which also keeps a recursive element finite),
  *Salami slice* (every element global, every use a reference) and *Venetian blind* (a named complex type for each element that has
  attributes or children, local elements of those types, global elements for the roots and for other namespaces).
- **Content models** (`content.rs`): for each element, the names of its children are put in a sequence of *factors*, each a choice of
  names repeated between a least and a most number of times. The order is the transitive closure of "an `a` comes before a `b` in some
  element"; names that come before each other are one factor (`(a|b)+`), names that never occur together are alternatives (`(a|b)`), the
  bounds are the least and greatest number of the factor's names in one element. Every name is in one factor, so the model is deterministic
  (no Unique Particle Attribution problem), and every element of the documents matches: **a schema inferred from documents validates
  each of them** (a property test over random documents, every layout, every option set, and the DTD). A least number of occurrences above
  one is never claimed, whatever was seen. More than 64 different child names in one kind of element give "any of them, any number of
  times".
- **Values** (`values.rs`): the narrowest of boolean (`true` and `false`; 0 and 1 are numbers), integer, decimal, double, date,
  dateTime and time that all the values are lexical forms of, else string; at most 512 different values are kept (past that they are still
  counted and still decide the type). A few values (default: at most 10, each seen at least twice; never booleans, never long values) are an
  enumeration.
- **Attributes** are required when every element of that name has them. Attributes of the schema-instance namespace are read by the
  validator and not declared in an XML Schema; the `xml:` ones (`lang`, `space`, `base`, `id`) are referred to through `xml.xsd`; other
  namespaced attributes are global declarations in their namespace's schema. In a DTD every attribute that was written is declared,
  namespace declarations included, and `xml:space` has the type a valid document must give it.
- **Mixed content** (text next to child elements) is `mixed="true"` with a repeated choice of the children (XSD) and `(#PCDATA|a|b)*` (DTD).
- Not inferred: keys and keyrefs, `xs:all`, derived types, patterns and length facets, `xsi:type`, DTD ID/IDREF/NMTOKEN types and entities.
- Bounds: 256 MiB of documents in a run, 10,000 different counts of children per element name, 2,000,000 elements per document, elements
  nested at most 256 deep.
