# xmlcraft-grammar

The grammar model that DTDs and XML Schemas compile into (layer L2, no UI, no I/O, compiles for the web). This first part is the content
model: what a parent element may contain, as an automaton that validation, completion and sample generation all ask.

- `Particle` is a content model as it is written: a `Term` (an element with a `Name`, or a wildcard with a `NamespaceConstraint`), a
  sequence or a choice of particles, each with `Occurs { min, max }`. Every term carries a `token`, the caller's own number for the
  declaration it stands for; it comes back with every step.
- `Automaton::compile(&particle)` builds a Glushkov (position) automaton. Counted occurrences are unrolled into copies, up to
  `MAX_POSITIONS` (100,000) positions; a bigger model, one that nests deeper than `MAX_DEPTH` (256), one whose construction takes more than
  `MAX_WORK`, and a count with `min` over `max` are errors (`CompileError`), not a hang. Positions that cannot be reached or cannot lead
  to the end are dropped, so a step that works is always on the way to a valid content.
- Reading the children of an element: `start()`, then `step(&state, &name)` for each child, which says which particle took it (its
  `token`) or refuses it, and `is_accepting(&state)` at the end. A deterministic model is at one position all the way
  (`State::alternatives()`); one that is not is read at several, so it is still read right.
- `expected(&state)` says what may come next, each with `mandatory` set when the content cannot be finished without it (every way on
  passes through it; worked out where there are at most 32 different things to choose from). `shortest_completion(&state)` is the
  shortest way to finish what was read, with the tokens of its particles.
- `check_deterministic()` is "Unique Particle Attribution" (XML Schema) and the determinism rule of XML 1.0: two particles that can take
  the same element are an `Ambiguity` naming both tokens. Wildcards compete with the elements they let in and with each other.
- Not here yet: the `all` group of XML Schema, counters instead of unrolling for very large counts, and the declarations themselves
  (elements, attributes, types, entities), which arrive with the DTD and XSD compilers.

Tests: `tests/automaton.rs` (behaviour, limits) and `tests/reference.rs`, which checks the automaton against a matcher written from the
meaning of a content model (the places where a particle can end, counting by iteration) on random models and words: acceptance, which
prefixes can still be completed, what is expected and mandatory, the shortest completion, and one position for a deterministic model.
