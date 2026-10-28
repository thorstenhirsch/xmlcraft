# xmlcraft-resolve

Finding the text that a reference names (layer L3, no UI, no I/O, compiles for the web).

`Resolver::resolve(&Request, &mut Budget)` takes a reference and the location of the document that has it:
- `Request::external_id(public, system, base)`: the external subset or a parameter entity of a DTD, by public and/or system identifier;
- `Request::uri(uri_or_namespace, base)`: a schema import, include or location, looked up in the catalogs' `uri` entries.

It looks, in this order, and stops at the first place that has the text:
1. **the documents that are open** (`Buffers`), by the absolute location the reference makes, so a schema being edited is used as it is;
2. **the user's catalogs** (the locations given to `Resolver::new`; one that cannot be read is skipped and noted in `warnings()`);
3. **the bundled catalogs** (`xmlcraft-schemas`: the schemas that ship with the app), unless the policy turns them off;
4. **the location itself**: a bundled file (`bundled:/…`, also by a relative name from another bundled file), a file or `file:` URI (`FileAccess`), or an `http`/`https` URL through **the cache** and then **the network**.

A location a catalog maps to is read the same way as one a document names, and a catalog entry that leads to something unreadable does not end the search. Relative references are joined against a URI or a path base (`join`); a Windows drive is not a scheme.

Downloading follows the `Policy`: `auto_download` (on by default), `offline` (nothing is downloaded and a copy past its time is still served), `use_bundled`, and the 30 days a copy is current without asking. An `http` URL is tried as `https` first, then as written; the cache is keyed by the URL that was asked for, and the text's location is the one after redirects. A copy past its time is asked about with its validators (`Conditions`: ETag, Last-Modified): *not modified* renews it, a new body replaces it, and a server that cannot be reached leaves the old copy to be served, marked `Cache { stale: true }`. A copy from the future (the clock was set back) is not current.

One load has a `Budget`: 256 resources and 64 MiB in all (a download may have 10 MiB). What does not fit is not read and is not counted.

`Resolved::trail` / `Failure::trail` list every place that was looked in and what it said (found, nothing, not asked and why, failed and why).

The world is traits the caller implements: `Buffers`, `FileAccess` (the implementation confines and caps), `Fetcher` (http), `ResourceCache`, `Clock` (the crate reads no clock). `MemoryCache` and `FixedClock` are there for tests and for the web. The disk cache and the HTTP client are `xmlcraft-platform`'s (M2.4).

Tests: `tests/resolution.rs` (the chain, with a world made of maps), `tests/location.rs` (schemes, joining, `file:` URIs; with property tests), `tests/budget.rs`.
