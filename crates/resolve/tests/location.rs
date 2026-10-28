//! Locations: schemes, joining, and the file a location names.

use proptest::prelude::*;
use xmlcraft_resolve::{file_path, is_path, join, scheme_of};

#[test]
fn a_scheme_is_letters_digits_plus_minus_and_dots_before_a_colon_and_a_drive_is_not_one() {
    for (location, scheme) in [
        ("http://x/y", Some("http")),
        ("HTTPS://x", Some("https")),
        ("file:///a", Some("file")),
        ("bundled:/w3c/x", Some("bundled")),
        ("urn:publicid:a", Some("urn")),
        ("a+b-c.d:rest", Some("a+b-c.d")),
        ("C:/dir/a.xml", None),
        ("c:\\dir\\a.xml", None),
        ("/dir/a.xml", None),
        ("a.xml", None),
        ("dir/a:b.xml", None),
        ("1http://x", None),
        (":x", None),
        ("", None),
    ] {
        assert_eq!(scheme_of(location).as_deref(), scheme, "{location:?}");
        assert_eq!(is_path(location), scheme.is_none(), "{location:?}");
    }
}

#[test]
fn a_relative_reference_is_resolved_against_a_uri_or_a_path() {
    for (base, reference, expected) in [
        ("http://x.org/a/b.xml", "c.dtd", "http://x.org/a/c.dtd"),
        ("http://x.org/a/b.xml", "../c.dtd", "http://x.org/c.dtd"),
        ("http://x.org/a/b.xml", "/c.dtd", "http://x.org/c.dtd"),
        ("bundled:/w3c/xmlschema/XMLSchema.xsd", "XMLSchema.dtd", "bundled:/w3c/xmlschema/XMLSchema.dtd"),
        ("file:///docs/a.xml", "a.dtd", "file:///docs/a.dtd"),
        ("/docs/a.xml", "a.dtd", "/docs/a.dtd"),
        ("/docs/a.xml", "sub/a.dtd", "/docs/sub/a.dtd"),
        ("/docs/a.xml", "../shared/a.dtd", "/shared/a.dtd"),
        ("/docs/a.xml", "./a.dtd", "/docs/a.dtd"),
        ("/docs/a.xml", "/abs/a.dtd", "/abs/a.dtd"),
        ("C:\\docs\\a.xml", "a.dtd", "C:/docs/a.dtd"),
        ("C:/docs/a.xml", "..\\b\\a.dtd", "C:/b/a.dtd"),
        ("/docs/a.xml", "C:\\x\\a.dtd", "C:\\x\\a.dtd"),
        ("docs/a.xml", "a.dtd", "docs/a.dtd"),
        ("a.xml", "a.dtd", "a.dtd"),
        ("", "a.dtd", "a.dtd"),
        ("", "/abs/a.dtd", "/abs/a.dtd"),
    ] {
        assert_eq!(join(base, reference), expected, "{base:?} + {reference:?}");
    }
}

#[test]
fn a_reference_with_a_scheme_stands_as_it_is() {
    assert_eq!(join("/docs/a.xml", "http://x.org/a.dtd"), "http://x.org/a.dtd");
    assert_eq!(join("http://other/", "ftp://x.org/a/../b.dtd"), "ftp://x.org/b.dtd");
    assert_eq!(join("", "file:///x/y.dtd"), "file:///x/y.dtd");
}

#[test]
fn an_empty_reference_names_nothing() {
    assert_eq!(join("/docs/a.xml", ""), "");
}

#[test]
fn a_file_uri_is_the_path_it_encodes() {
    for (location, path) in [
        ("/docs/a.dtd", Some("/docs/a.dtd")),
        ("file:///docs/a.dtd", Some("/docs/a.dtd")),
        ("file://localhost/docs/a.dtd", Some("/docs/a.dtd")),
        ("file:///docs/a%20b.dtd", Some("/docs/a b.dtd")),
        ("file:///docs/%C3%A4.dtd", Some("/docs/ä.dtd")),
        ("file:///C:/docs/a.dtd", Some("C:/docs/a.dtd")),
        ("file:/docs/a.dtd", Some("/docs/a.dtd")),
        ("file:///docs/a.dtd#frag", Some("/docs/a.dtd")),
        ("file:///docs/a.dtd?q", Some("/docs/a.dtd")),
        ("C:\\docs\\a.dtd", Some("C:\\docs\\a.dtd")),
        ("file://server/share/a.dtd", None),
        ("file:///docs/%FF.dtd", None),
        ("file:///docs/%zz.dtd", None),
        ("file:///docs/%4", None),
        ("http://x/a.dtd", None),
        ("bundled:/a", None),
    ] {
        assert_eq!(file_path(location).as_deref(), path, "{location:?}");
    }
}

proptest! {
    #[test]
    fn nothing_here_panics(base in "\\PC{0,40}", reference in "\\PC{0,40}") {
        let joined = join(&base, &reference);
        let _ = (scheme_of(&joined), file_path(&joined), file_path(&base), is_path(&reference));
    }

    #[test]
    fn a_plain_name_next_to_a_uri_stays_in_its_folder(folder in "[a-z]{1,6}(/[a-z]{1,6}){0,3}", name in "[a-z]{1,8}\\.(dtd|xsd)") {
        let joined = join(&format!("http://example.org/{folder}/doc.xml"), &name);
        prop_assert_eq!(joined, format!("http://example.org/{folder}/{name}"));
    }

    #[test]
    fn a_plain_name_next_to_a_path_stays_in_its_folder(folder in "[a-z]{1,6}(/[a-z]{1,6}){0,3}", name in "[a-z]{1,8}\\.(dtd|xsd)") {
        let joined = join(&format!("/{folder}/doc.xml"), &name);
        prop_assert_eq!(joined, format!("/{folder}/{name}"));
    }
}
