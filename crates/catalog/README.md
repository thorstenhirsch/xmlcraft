# xmlcraft-catalog

OASIS XML Catalogs 1.1 for XmlCraft (layer L2, no UI, no I/O, compiles for the web).

- `Catalog::parse(text, location)` reads one catalog file: `public`, `system`, `rewriteSystem`, `systemSuffix`, `delegatePublic`,
  `delegateSystem`, `uri`, `rewriteURI`, `uriSuffix`, `delegateURI`, `nextCatalog`, `group`, `xml:base` and `prefer`. Elements of other
  namespaces are skipped silently, with what is inside them; an entry that lacks an attribute, an unknown element of the catalog
  namespace, and an element that uses an entity of the catalog's own (`&name;`, which is not expanded) are left out **and noted** in
  `Catalog::warnings` (the first 99 problems are listed, then one line says that there were more). A file that is not well-formed or
  whose root is not a catalog is an error. At most 100,000 entries are read, and groups nest at most 64 deep.
- `Catalogs` is the catalog entry file list. `resolve(public, system, loader)` and `resolve_uri(uri, loader)` follow §7 of the
  specification: the entries in the spec's order (system, rewriteSystem, systemSuffix, delegateSystem, public, delegatePublic,
  nextCatalog), the longest match, delegation (the delegated catalogs answer alone, and only about the identifier that matched), and
  `nextCatalog` catalogs consulted right after the one that names them (one that is waiting in the list is moved up; one that was
  consulted is not followed again). `urn:publicid:` identifiers are unwrapped. Identifiers are compared as they are normalised (§6.3:
  what may not stand in a URI is percent-escaped, an escape already there stays as written), and a rewritten URI is built from the
  normalised identifier, which is a valid URI.
- The catalogs that are named but not given come from a `Loader` that the caller provides (a file reader, the open buffers, the
  cache): this crate never reads a file or the network. A resolution loads at most 64 catalogs, each once, follows at most 64
  delegations, and a chain or a cycle ends. The loader is told (`Loader::note`) of a named catalog that could not be read, is not a
  catalog, or had entries left out.
- `prefer`: where no attribute says, a public identifier counts even when a system identifier is given too (the spec leaves the
  default to the application; `Catalogs::set_prefer_public(false)` changes it).
- Not supported: the TR9401 `catalog.cat` text syntax and its entries (`DOCTYPE`, `ENTITY`, …).

Specification: <https://www.oasis-open.org/committees/download.php/14809/xml-catalogs.html> (read for the behaviour; no implementation was
read). Tests: `tests/resolution.rs`.
