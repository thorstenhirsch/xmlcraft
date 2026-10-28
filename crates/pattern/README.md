# xmlcraft-pattern

XML Schema regular expressions, the language of the `pattern` facet (layer L0, no UI, no I/O, compiles for the web).

`Pattern::new(source)` reads a pattern as XML Schema Part 2, appendix F defines it and builds a matcher: `is_match(value)` is true when the **whole** value matches.

- **What differs from the expressions most programmers know:** no anchors (`^` and `$` are ordinary characters), no lazy quantifiers, groups that do not capture, no look-around, no back references; `.` is anything but `\n` and `\r`; a class can have another taken out of it (`[a-z-[aeiou]]`, also nested and after a negation); `\p{Lu}`/`\P{Lu}` name the 36 general categories (and only those); `\p{IsBasicLatin}` names a Unicode block by the names of the schema language (`IsLatin-1Supplement`, `IsPrivateUse`, `IsSpecials`, …, with `IsGreekandCoptic` as another name of `IsGreek`); `\s`, `\d`, `\w` and their complements are XML's (`\w` leaves out punctuation, separators and "other", so `_` is not a `\w`), and `\i`, `\c`, `\I`, `\C` are the characters XML names start with or are made of (`NameStartChar`, `NameChar` of XML 1.0 fifth edition).
- **Strict about `{` and `}`:** they have to be escaped when they are not a quantifier (the XML Schema 1.1 grammar), and the single-character escapes are the ones the spec lists.
- **Errors** carry the position (the number of the character) and a kind (`ErrorKind`); `message()` says what is wrong in words.
- **Limits** (a pattern from a schema is untrusted): 100,000 characters, groups and classes nested 128 deep, counts up to 1,000,000, and an automaton of at most 32 MiB (`ErrorKind::TooComplex`).
- **How it works:** the pattern is parsed into a small tree of character sets (`regex-syntax` sets, so categories and set arithmetic are the library's), turned into a `regex-syntax` `Hir`, anchored with `\A…\z` and handed to the `regex-automata` engine as it is. (It is not printed as text and read again: the printer writes a repetition of a repetition as a lazy or invalid quantifier; a property test found that.) `Pattern::hir()` is the tree without the anchors, for generating values that match.
- **Not supported:** Unicode versions other than the one `regex-syntax` carries (XML Schema 1.0 was written against Unicode 3.1; the categories here follow the library, which only differs for characters added since), case-insensitive matching (there is none in the language), and the XPath/XQuery flags.

Tests: `tests/syntax.rs` (what matches and what is refused, where), `tests/properties.rs` (arbitrary text never panics; a pattern agrees with an independent reading of the same expression on random values).
