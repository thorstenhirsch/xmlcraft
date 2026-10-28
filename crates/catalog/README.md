# xmlcraft-catalog

OASIS XML Catalogs 1.1 for XmlCraft (layer L2, no UI, no I/O, compiles for the web).

- `Catalog::parse(text, location)` reads one catalog file: `public`, `system`, `rewriteSystem`, `systemSuffix`, `delegatePublic`,
  `delegateSystem`, `uri`, `rewriteURI`, `uriSuffix`, `delegateURI`, `nextCatalog`, `group`, `xml:base` and `prefer`. Elements of other
  namespaces are skipped with what is inside them; an entry that lacks an attribute, or an unknown element, is left out **and noted** in
  `Catalog::warnings`. A file that is not well-formed or whose root is not a catalog is an error. At most 100,000 entries are read.
- `Catalogs` is the catalog entry file list. `resolve(public, system, loader)` and `resolve_uri(uri, loader)` follow §7 of the
  specification: the entries in the spec's order (system, rewriteSystem, systemSuffix, delegateSystem, public, delegatePublic,
  nextCatalog), the longest match, delegation (the delegated catalogs answer alone, and only about the identifier that matched), and
  `nextCatalog` catalogs consulted right after the one that names them. `urn:publicid:` identifiers are unwrapped.
- The catalogs that are named but not given come from a `Loader` that the caller provides (a file reader, the open buffers, the
  cache): this crate never reads a file or the network. A resolution loads at most 64 catalogs, each once, and a chain or a cycle ends.
- `prefer`: where no attribute says, a public identifier counts even when a system identifier is given too (the spec leaves the
  default to the application; `Catalogs::set_prefer_public(false)` changes it).
- Not supported: the TR9401 `catalog.cat` text syntax and its entries (`DOCTYPE`, `ENTITY`, …).

Specification: <https://www.oasis-open.org/committees/download.php/14809/xml-catalogs.html> (read for the behaviour; no implementation was
read). Tests: `tests/resolution.rs`.
