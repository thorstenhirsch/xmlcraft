#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-schemas: the standard schemas, DTDs and catalogs that ship with XmlCraft (`schemas/` in the repository), packed into the
//! binary at build time and inflated one file at a time when asked for.
//!
//! - **Layer:** L0, no UI and no I/O, so it compiles for the web too.
//! - The files are byte for byte the ones their publishers issue (AGENTS.md section 1.6); `cargo xtask schemas` checks them against
//!   `schemas/manifest.toml`. What is in the bundle besides schemas: the licence texts, and the catalogs that say which public
//!   identifier, system identifier or namespace is which file.
//! - A bundled file has a path (`w3c/xmlschema/XMLSchema.xsd`) and a URI (`bundled:/w3c/xmlschema/XMLSchema.xsd`, see [`uri_of`]); the catalogs are read
//!   from the URI of their file, so their relative entries are URIs of the bundle too.

use std::sync::OnceLock;

/// The scheme (and root) of the URI of a bundled file.
pub const URI_PREFIX: &str = "bundled:/";
/// The catalog that leads to all the others.
pub const ROOT_CATALOG: &str = "catalog.xml";

static BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bundle.bin"));

struct Entry {
    path: &'static str,
    len: usize,
    deflated: &'static [u8],
}

/// The index of the blob. A blob that cannot be read as the build script writes it is an empty bundle.
fn index() -> &'static [Entry] {
    static INDEX: OnceLock<Vec<Entry>> = OnceLock::new();
    INDEX.get_or_init(|| read_index(BLOB).unwrap_or_default())
}

fn read_index(blob: &'static [u8]) -> Option<Vec<Entry>> {
    let mut at = 0usize;
    let mut take = |n: usize| -> Option<&'static [u8]> {
        let part = blob.get(at..at.checked_add(n)?)?;
        at += n;
        Some(part)
    };
    if take(4)? != b"XSB1" {
        return None;
    }
    let count = u32::from_le_bytes(take(4)?.try_into().ok()?) as usize;
    let mut raw = Vec::new();
    for _ in 0..count {
        let path_len = u16::from_le_bytes(take(2)?.try_into().ok()?) as usize;
        let path = std::str::from_utf8(take(path_len)?).ok()?;
        let len = u32::from_le_bytes(take(4)?.try_into().ok()?) as usize;
        let deflated = u32::from_le_bytes(take(4)?.try_into().ok()?) as usize;
        let offset = u32::from_le_bytes(take(4)?.try_into().ok()?) as usize;
        raw.push((path, len, deflated, offset));
    }
    let data = blob.get(at..)?;
    let mut entries: Vec<Entry> = raw
        .into_iter()
        .map(|(path, len, deflated, offset)| Some(Entry { path, len, deflated: data.get(offset..offset.checked_add(deflated)?)? }))
        .collect::<Option<_>>()?;
    // Looked up by bisection: the order is the paths' own, whatever order the build wrote them in.
    entries.sort_by(|a, b| a.path.cmp(b.path));
    Some(entries)
}

/// The paths of the bundled files, in order.
pub fn paths() -> impl Iterator<Item = &'static str> {
    index().iter().map(|e| e.path)
}

/// Whether `path` is a bundled file.
pub fn contains(path: &str) -> bool {
    find(path).is_some()
}

fn find(path: &str) -> Option<&'static Entry> {
    let entries = index();
    entries.binary_search_by(|e| e.path.cmp(path)).ok().and_then(|i| entries.get(i))
}

/// The length in bytes of the bundled file `path`.
pub fn size(path: &str) -> Option<usize> {
    find(path).map(|e| e.len)
}

/// The bytes of the bundled file `path`; `None` when there is no such file.
pub fn get(path: &str) -> Option<Vec<u8>> {
    let entry = find(path)?;
    let bytes = miniz_oxide::inflate::decompress_to_vec_with_limit(entry.deflated, entry.len).ok()?;
    (bytes.len() == entry.len).then_some(bytes)
}

/// The bundled file `path` as text; `None` when there is no such file or it is not UTF-8.
pub fn get_text(path: &str) -> Option<String> {
    String::from_utf8(get(path)?).ok()
}

/// The text of the root catalog.
pub fn root_catalog() -> Option<String> {
    get_text(ROOT_CATALOG)
}

/// The URI of the bundled file `path`.
pub fn uri_of(path: &str) -> String {
    format!("{URI_PREFIX}{path}")
}

/// The path of the bundled file that `uri` names, if it is the URI of one (see [`uri_of`]).
pub fn path_of(uri: &str) -> Option<&str> {
    uri.strip_prefix(URI_PREFIX).filter(|p| !p.is_empty())
}

/// The JSON Schema documents that are bundled, by the URI they are known by (their `$id`, or `id` in draft 4, without a trailing `#`):
/// the meta-schemas of the drafts and their vocabularies, and the schemas of OpenAPI 2.0, 3.0, 3.1 and 3.2 with what they refer to.
pub const JSON_SCHEMAS: &[(&str, &str)] = &[
    ("http://json-schema.org/draft-04/schema", "json-schema/draft-04/schema.json"),
    ("http://json-schema.org/draft-06/schema", "json-schema/draft-06/schema.json"),
    ("http://json-schema.org/draft-07/schema", "json-schema/draft-07/schema.json"),
    ("https://json-schema.org/draft/2019-09/schema", "json-schema/draft-2019-09/schema.json"),
    ("https://json-schema.org/draft/2019-09/meta/core", "json-schema/draft-2019-09/meta/core.json"),
    ("https://json-schema.org/draft/2019-09/meta/applicator", "json-schema/draft-2019-09/meta/applicator.json"),
    ("https://json-schema.org/draft/2019-09/meta/validation", "json-schema/draft-2019-09/meta/validation.json"),
    ("https://json-schema.org/draft/2019-09/meta/meta-data", "json-schema/draft-2019-09/meta/meta-data.json"),
    ("https://json-schema.org/draft/2019-09/meta/format", "json-schema/draft-2019-09/meta/format.json"),
    ("https://json-schema.org/draft/2019-09/meta/content", "json-schema/draft-2019-09/meta/content.json"),
    ("https://json-schema.org/draft/2020-12/schema", "json-schema/draft-2020-12/schema.json"),
    ("https://json-schema.org/draft/2020-12/meta/core", "json-schema/draft-2020-12/meta/core.json"),
    ("https://json-schema.org/draft/2020-12/meta/applicator", "json-schema/draft-2020-12/meta/applicator.json"),
    ("https://json-schema.org/draft/2020-12/meta/unevaluated", "json-schema/draft-2020-12/meta/unevaluated.json"),
    ("https://json-schema.org/draft/2020-12/meta/validation", "json-schema/draft-2020-12/meta/validation.json"),
    ("https://json-schema.org/draft/2020-12/meta/meta-data", "json-schema/draft-2020-12/meta/meta-data.json"),
    ("https://json-schema.org/draft/2020-12/meta/format-annotation", "json-schema/draft-2020-12/meta/format-annotation.json"),
    ("https://json-schema.org/draft/2020-12/meta/content", "json-schema/draft-2020-12/meta/content.json"),
    ("http://swagger.io/v2/schema.json", "openapi/v2.0/schema.json"),
    ("https://spec.openapis.org/oas/3.0/schema/2024-10-18", "openapi/v3.0/schema-2024-10-18.json"),
    ("https://spec.openapis.org/oas/3.1/schema/2025-09-15", "openapi/v3.1/schema-2025-09-15.json"),
    ("https://spec.openapis.org/oas/3.1/schema-base/2025-09-15", "openapi/v3.1/schema-base-2025-09-15.json"),
    ("https://spec.openapis.org/oas/3.1/dialect/2024-11-10", "openapi/v3.1/dialect-2024-11-10.json"),
    ("https://spec.openapis.org/oas/3.1/meta/2024-11-10", "openapi/v3.1/meta-2024-11-10.json"),
    ("https://spec.openapis.org/oas/3.2/schema/2025-09-17", "openapi/v3.2/schema-2025-09-17.json"),
    ("https://spec.openapis.org/oas/3.2/schema-base/2025-09-17", "openapi/v3.2/schema-base-2025-09-17.json"),
    ("https://spec.openapis.org/oas/3.2/dialect/2025-09-17", "openapi/v3.2/dialect-2025-09-17.json"),
    ("https://spec.openapis.org/oas/3.2/meta/2025-09-17", "openapi/v3.2/meta-2025-09-17.json"),
];

/// The text of the bundled JSON Schema document known by `uri` (a trailing `#` does not matter), if there is one.
pub fn json_schema(uri: &str) -> Option<String> {
    let uri = uri.trim_end_matches('#');
    let (_, path) = JSON_SCHEMAS.iter().find(|(known, _)| *known == uri)?;
    get_text(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A blob in the build script's layout, with the files in the order given.
    fn blob(files: &[(&str, &[u8])]) -> &'static [u8] {
        let mut index = b"XSB1".to_vec();
        index.extend_from_slice(&(files.len() as u32).to_le_bytes());
        let mut data = Vec::new();
        for (path, bytes) in files {
            let deflated = miniz_oxide::deflate::compress_to_vec(bytes, 6);
            index.extend_from_slice(&(path.len() as u16).to_le_bytes());
            index.extend_from_slice(path.as_bytes());
            index.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            index.extend_from_slice(&(deflated.len() as u32).to_le_bytes());
            index.extend_from_slice(&(data.len() as u32).to_le_bytes());
            data.extend_from_slice(&deflated);
        }
        index.extend_from_slice(&data);
        Box::leak(index.into_boxed_slice())
    }

    #[test]
    fn every_bundled_json_schema_is_there_under_the_uri_it_says_it_is_and_nothing_is_left_out() {
        for (uri, path) in JSON_SCHEMAS {
            let text = get_text(path).unwrap_or_else(|| panic!("{path} is bundled"));
            // The `$id` (`id` in draft 4) of the document, found in its text.
            let key = if text.contains("\"$id\"") { "\"$id\"" } else { "\"id\"" };
            let at = text.find(key).unwrap_or_else(|| panic!("{path} has an id"));
            let rest = &text[at + key.len()..];
            let start = rest.find('"').expect("a string") + 1;
            let id = &rest[start..start + rest[start..].find('"').expect("its end")];
            assert_eq!(id.trim_end_matches('#'), *uri, "{path}");
            assert_eq!(json_schema(uri).as_deref(), Some(text.as_str()));
            assert_eq!(json_schema(&format!("{uri}#")).as_deref(), Some(text.as_str()), "a trailing # does not matter");
        }
        // Every JSON file of the two sets is in the table.
        for path in paths().filter(|p| p.starts_with("json-schema/") || p.starts_with("openapi/")) {
            if path.ends_with(".json") {
                assert!(JSON_SCHEMAS.iter().any(|(_, known)| *known == path), "{path} is bundled but not in JSON_SCHEMAS");
            }
        }
        assert_eq!(json_schema("https://example.com/nope"), None);
    }

    #[test]
    fn the_index_is_in_the_order_of_the_paths_whatever_order_it_was_written_in() {
        // "a-y" comes before "a/z" as text, but a folder walk visits the folder `a` first.
        let index = read_index(blob(&[("b/x", b"1"), ("a/z", b"2"), ("a-y", b"3")])).expect("a bundle");
        assert_eq!(index.iter().map(|e| e.path).collect::<Vec<_>>(), ["a-y", "a/z", "b/x"]);
        let inflated: Vec<Vec<u8>> =
            index.iter().map(|e| miniz_oxide::inflate::decompress_to_vec_with_limit(e.deflated, e.len).expect("inflates")).collect();
        assert_eq!(inflated, [b"3".to_vec(), b"2".to_vec(), b"1".to_vec()], "each path keeps its own bytes");
    }

    #[test]
    fn a_blob_that_is_not_one_is_no_bundle() {
        assert!(read_index(b"").is_none());
        assert!(read_index(b"XSB2\0\0\0\0").is_none());
        assert!(read_index(b"XSB1\x01\0\0\0").is_none(), "an entry is announced and missing");
        // An entry whose bytes lie outside the data.
        let good = blob(&[("a", b"1")]);
        assert!(read_index(&good[..good.len() - 1]).is_none());
        assert_eq!(read_index(blob(&[])).map(|i| i.len()), Some(0));
    }
}
