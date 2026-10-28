//! `catalog_lookup` with real catalog files: the answer, the catalogs a catalog names, and what the root confinement keeps out.

use std::path::Path;

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    a.call("catalog_lookup", &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

/// A `file:` URI for a path, written the way the tool writes one: every byte that is not a letter, a digit, one of `-._~` or a separator
/// is a percent-escape (so a temporary folder with a `#`, a `?`, a `%` or a `&` in its name is still named right).
fn file_uri(path: &Path) -> String {
    let text = path.to_str().expect("utf-8").replace('\\', "/");
    let text = if text.starts_with('/') { text } else { format!("/{text}") };
    let mut uri = String::from("file://");
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') || (cfg!(windows) && byte == b':') {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

fn write(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().expect("a folder")).expect("folder");
    std::fs::write(&path, format!("<catalog xmlns=\"urn:oasis:names:tc:entity:xmlns:xml:catalog\">{body}</catalog>")).expect("writes");
    path.to_str().expect("utf-8").to_owned()
}

#[test]
fn identifiers_and_uris_are_mapped_through_the_catalogs_in_order() {
    let dir = tempfile::tempdir().expect("dir");
    std::fs::write(dir.path().join("a.dtd"), "<!ELEMENT a EMPTY>").expect("writes");
    let first = write(
        dir.path(),
        "one/catalog.xml",
        r#"<system systemId="http://x.org/a.dtd" uri="../a.dtd"/><public publicId="-//X//DTD B//EN" uri="missing.dtd"/><nextCatalog catalog="../two.xml"/>"#,
    );
    let second = write(dir.path(), "two.xml", r#"<uri name="http://x.org/ns" uri="schema.xsd"/><system systemId="only-in-two" uri="two.dtd"/>"#);
    let mut a = Automation::new();
    let named = call(&mut a, json!({ "catalogs": [first], "system": "http://x.org/a.dtd" })).expect("resolves");
    assert!(named["resolved"].as_str().is_some_and(|r| r.starts_with("file://") && r.ends_with("/a.dtd")), "{named}");
    assert_eq!(named["exists"], true, "the file is there");
    // A catalog that is named is read too, right after the one that names it.
    let next = call(&mut a, json!({ "catalogs": [first], "system": "only-in-two" })).expect("resolves");
    assert!(next["resolved"].as_str().is_some_and(|r| r.ends_with("/two.dtd")), "{next}");
    assert_eq!((next["catalogs_read"].as_u64(), next["exists"].as_bool()), (Some(2), Some(false)));
    let uri = call(&mut a, json!({ "catalogs": [first], "uri": "http://x.org/ns" })).expect("resolves");
    assert!(uri["resolved"].as_str().is_some_and(|r| r.ends_with("/schema.xsd")), "{uri}");
    // A public identifier whose file is missing is still the answer, and says so; nothing found is null.
    let public = call(&mut a, json!({ "catalogs": [first, second], "public": "-//X//DTD B//EN" })).expect("resolves");
    assert_eq!((public["resolved"].as_str().is_some(), public["exists"].as_bool()), (true, Some(false)));
    let none = call(&mut a, json!({ "catalogs": [first], "system": "http://nowhere/" })).expect("resolves");
    assert_eq!((none["resolved"].clone(), none["exists"].clone()), (Value::Null, Value::Null));
    // `prefer_public: false` lets a system identifier outrank a public entry that no `prefer` attribute protects.
    let both =
        call(&mut a, json!({ "catalogs": [first], "public": "-//X//DTD B//EN", "system": "elsewhere", "prefer_public": false })).expect("resolves");
    assert_eq!(both["resolved"], Value::Null);
}

#[test]
fn the_arguments_are_checked_and_a_file_that_is_not_a_catalog_is_an_error() {
    let dir = tempfile::tempdir().expect("dir");
    let good = write(dir.path(), "c.xml", "");
    let not_a_catalog = dir.path().join("x.xml");
    std::fs::write(&not_a_catalog, "<a/>").expect("writes");
    let mut a = Automation::new();
    for (args, needle) in [
        (json!({ "catalogs": [good] }), "uri, or a public"),
        (json!({ "catalogs": [good], "uri": "u", "system": "s" }), "uri, or a public"),
        (json!({ "catalogs": [] , "uri": "u" }), "catalogs"),
        (json!({ "catalogs": [not_a_catalog.to_str().expect("utf-8")], "uri": "u" }), "catalog"),
        (json!({ "catalogs": [dir.path().join("none.xml").to_str().expect("utf-8")], "uri": "u" }), "none.xml"),
    ] {
        let error = call(&mut a, args.clone()).expect_err(&args.to_string()).to_string();
        assert!(error.contains(needle), "{args}: {error}");
    }
}

#[test]
fn the_root_keeps_the_catalogs_and_the_ones_they_name_inside() {
    let outside = tempfile::tempdir().expect("outside");
    let secret = write(outside.path(), "secret.xml", r#"<system systemId="s" uri="leaked.dtd"/>"#);
    let root = tempfile::tempdir().expect("root");
    // The catalog is named by a `file:` URI, which looks the same on every platform (`file:///C:/…` on Windows).
    let uri = file_uri(Path::new(&secret));
    let inner = write(root.path(), "inner.xml", &format!(r#"<nextCatalog catalog="{uri}"/>"#));
    let mut a = Automation::with_root(root.path()).expect("a root");
    // The catalog inside is read; the one it names outside the root is not (it is skipped, as one that cannot be read is).
    let answer = call(&mut a, json!({ "catalogs": [inner], "system": "s" })).expect("resolves");
    assert_eq!((answer["resolved"].clone(), answer["catalogs_read"].as_u64()), (Value::Null, Some(1)), "{answer}");
    // And naming it directly is refused.
    assert!(call(&mut a, json!({ "catalogs": [secret], "system": "s" })).is_err());
    // Without the root it is read, which shows the test is about the root.
    let mut free = Automation::new();
    let free_answer = call(&mut free, json!({ "catalogs": [inner], "system": "s" })).expect("resolves");
    assert!(free_answer["resolved"].as_str().is_some_and(|r| r.ends_with("leaked.dtd")), "{free_answer}");
}

#[test]
fn what_is_wrong_with_the_catalogs_that_were_named_is_reported_too() {
    let dir = tempfile::tempdir().expect("dir");
    let named = dir.path().join("named.xml");
    std::fs::write(
        &named,
        r#"<catalog xmlns="urn:oasis:names:tc:entity:xmlns:xml:catalog"><frobnicate/><system systemId="s" uri="found.dtd"/></catalog>"#,
    )
    .expect("writes");
    let torn = dir.path().join("torn.xml");
    std::fs::write(&torn, "<catalog").expect("writes");
    let first = write(
        dir.path(),
        "first.xml",
        &format!(
            r#"<nextCatalog catalog="{}"/><nextCatalog catalog="{}"/><nextCatalog catalog="{}"/>"#,
            file_uri(&torn),
            file_uri(&dir.path().join("gone.xml")),
            file_uri(&named)
        ),
    );
    let mut a = Automation::new();
    let answer = call(&mut a, json!({ "catalogs": [first], "system": "s" })).expect("resolves");
    assert!(answer["resolved"].as_str().is_some_and(|r| r.ends_with("/found.dtd")), "{answer}");
    let warnings: Vec<&str> = answer["warnings"].as_array().expect("a list").iter().filter_map(Value::as_str).collect();
    assert_eq!(warnings.len(), 3, "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("torn.xml") && w.contains("not well-formed")), "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("gone.xml") && w.contains("cannot be read")), "{warnings:?}");
    assert!(warnings.iter().any(|w| w.contains("named.xml") && w.contains("frobnicate")), "{warnings:?}");
}

#[test]
fn the_list_of_problems_with_the_named_catalogs_is_cut_off_and_says_so() {
    let dir = tempfile::tempdir().expect("dir");
    // Two catalogs with 60 unknown elements each: more problems than the list holds.
    let noisy = "<frobnicate/>".repeat(60);
    for name in ["one.xml", "two.xml"] {
        write(dir.path(), name, &noisy);
    }
    let first = write(
        dir.path(),
        "first.xml",
        &format!(
            r#"<nextCatalog catalog="{}"/><nextCatalog catalog="{}"/>"#,
            file_uri(&dir.path().join("one.xml")),
            file_uri(&dir.path().join("two.xml"))
        ),
    );
    let mut a = Automation::new();
    let answer = call(&mut a, json!({ "catalogs": [first], "system": "nowhere" })).expect("resolves");
    let warnings: Vec<&str> = answer["warnings"].as_array().expect("a list").iter().filter_map(Value::as_str).collect();
    assert_eq!(warnings.len(), 100, "the list is bounded");
    assert!(warnings.last().is_some_and(|w| w.contains("not listed")), "its last place says so: {:?}", warnings.last());
    assert!(warnings.iter().filter(|w| w.contains("not listed")).count() == 1, "and only once");
}

#[cfg(unix)]
#[test]
fn a_catalog_over_the_size_limit_is_refused_without_being_read() {
    let dir = tempfile::tempdir().expect("dir");
    let big = dir.path().join("big.xml");
    let file = std::fs::File::create(&big).expect("creates");
    file.set_len(5 * 1024 * 1024).expect("sets the length (a sparse file)");
    let mut a = Automation::new();
    let error = call(&mut a, json!({ "catalogs": [big.to_str().expect("utf-8")], "uri": "u" })).expect_err("too big").to_string();
    assert!(error.contains("big.xml") && error.contains("over"), "{error}");
}
