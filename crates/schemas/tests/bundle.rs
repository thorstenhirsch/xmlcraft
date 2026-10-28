//! The bundle holds the files of `schemas/` as they are on disk.

use std::path::{Path, PathBuf};

use xmlcraft_schemas::{ROOT_CATALOG, contains, get, get_text, path_of, paths, root_catalog, size, uri_of};

fn schemas_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas")
}

fn on_disk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).expect("a folder").map(|e| e.expect("an entry")).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = format!("{prefix}{name}");
        if entry.file_type().expect("a type").is_dir() {
            on_disk(&entry.path(), &format!("{path}/"), out);
        } else if path != "manifest.toml" && !name.starts_with('.') {
            out.push(path);
        }
    }
}

#[test]
fn every_file_of_the_folder_is_in_the_bundle_byte_for_byte_and_nothing_else_is() {
    let mut expected = Vec::new();
    on_disk(&schemas_dir(), "", &mut expected);
    expected.sort();
    let bundled: Vec<&str> = paths().collect();
    assert_eq!(bundled, expected, "the same files, in the order of their paths");
    assert!(!bundled.is_empty());
    for path in &bundled {
        let disk = std::fs::read(schemas_dir().join(path)).expect("the file");
        assert_eq!(get(path).as_deref(), Some(disk.as_slice()), "{path}");
        assert_eq!(size(path), Some(disk.len()), "{path}");
        assert!(contains(path));
    }
}

#[test]
fn the_manifest_is_about_the_bundle_and_not_in_it() {
    assert!(!contains("manifest.toml"));
    assert_eq!(get("manifest.toml"), None);
}

#[test]
fn a_path_that_is_not_a_file_of_the_bundle_gives_nothing() {
    for path in ["", "/", "w3c", "w3c/", "nope.xsd", "../Cargo.toml", "w3c/../catalog.xml", "/catalog.xml", "CATALOG.XML"] {
        assert_eq!((get(path), size(path), contains(path)), (None, None, false), "{path:?}");
    }
}

#[test]
fn the_root_catalog_is_a_catalog_that_leads_on() {
    let text = root_catalog().expect("a root catalog");
    assert_eq!(Some(text.clone()), get_text(ROOT_CATALOG));
    assert!(text.contains("urn:oasis:names:tc:entity:xmlns:xml:catalog") && text.contains("nextCatalog"), "{text}");
}

#[test]
fn text_is_text() {
    let schema = get_text("w3c/xmlschema/XMLSchema.xsd").expect("the schema for schemas");
    assert!(schema.starts_with("<?xml"), "{}", &schema[..40]);
    // A licence page is text too; what is not UTF-8 would give nothing.
    assert!(get_text("w3c/LICENSE-W3C-20150513.html").is_some_and(|t| t.contains("W3C")));
}

#[test]
fn the_uri_of_a_file_names_it_and_only_such_a_uri_does() {
    assert_eq!(uri_of("w3c/catalog.xml"), "bundled:/w3c/catalog.xml");
    assert_eq!(path_of(&uri_of("w3c/catalog.xml")), Some("w3c/catalog.xml"));
    for not in ["", "bundled:/", "bundled:", "file:///w3c/catalog.xml", "http://www.w3.org/2001/XMLSchema.xsd", "BUNDLED:/a"] {
        assert_eq!(path_of(not), None, "{not:?}");
    }
}
