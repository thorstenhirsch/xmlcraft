# xmlcraft-sample

An XML document generated from an XML Schema or a DTD (layer L4, no UI, no I/O, compiles for the web).

- `sample_from_schema(&Schema, root, &Options)` makes a document from an assembled schema (`xmlcraft_xsd::schema::Schema`), `sample_from_dtd(&Dtd,
  root, &Options)` from a DTD. `schema_roots` and `dtd_roots` name the elements a document can start with (abstract ones are not among them).
  The result is the text and the warnings: what could not be made as asked.
- **Structure**: a sequence is each of its parts, a choice one branch (`ChoicePolicy::First`, or each in turn), an `xs:all` its parts. A
  particle occurs `repeat` times kept between its least and greatest number; `optional: false` leaves out what may be left out. Attributes
  that are not required are made when `optional` is set. Abstract elements are replaced by a member of their substitution group, an
  abstract type by a type derived from it (with `xsi:type`), a wildcard by the shallowest global element it allows that is not being made.
- **Recursion** ends with the *height* of each element, the least depth of a valid tree below it, found by iterating the rule until nothing
  changes. At `max_depth` only what is required is made, and of a choice the branch that is shallowest; an element that requires itself
  has no height and is refused (`SampleError::Unsatisfiable`). The document is also bounded: 300 levels, 200,000 elements, 10,000
  occurrences of one particle.
- **Values** (`value.rs`) come from a fixed value, then a default, then the enumeration of the type (walked through in turn for each
  element or attribute name), then, in order, what suits the kind of type (a different small number each time, inside the bounds; a date, a
  duration, a name, hex and base64 of the length asked for, an item or more for a list, the first member that gives one for a union), and
  last strings made from the syntax tree of the patterns. **Each candidate is checked by the type itself**, so a value that is written is a
  valid one; when none fits, the element or attribute is empty and a warning says so. IDs are `id1`, `id2`, …, and a reference to an ID is
  `id1` (a warning when the document has no ID). `Fill::Named` makes a string the name of its element or attribute.
- **Namespaces** (`tree.rs`): the root declares every namespace used with `ns1`, `ns2`, … (the schema-instance namespace is `xsi`), names in
  no namespace stay bare, `xsi:type` is a qualified name written with the prefix its namespace got, and the location of the schema can be
  said (`xsi:schemaLocation` or `xsi:noNamespaceSchemaLocation`).
- A DTD is generated the same way; its names are written as they are (a prefixed name keeps its prefix, and the `xmlns` attributes the DTD
  declares are made as attributes), attribute defaults are used, and an enumerated attribute takes its values in turn.
- Checked: every global element of the bundled schemas gives a valid document; a schema inferred from random documents (`xmlcraft-infer`)
  gives valid samples from every root in every layout and option set, and so does an inferred DTD.
- Not made: keys, keyrefs and uniqueness (a document with repeated items may repeat a key), mixed content text, `xs:any` with a namespace
  that no global element is in, `nil` elements, `NOTATION` and `ENTITY` values beyond what the DTD declares.
