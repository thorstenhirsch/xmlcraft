//! Following includes, imports and redefinitions: each document once, in the right namespace, within limits.

use std::collections::HashMap;

use xmlcraft_syntax::Severity;
use xmlcraft_xsd::{Documents, LoadKind, LoadRequest, Loaded, MAX_DOCUMENTS, SchemaLoader, load};

const XS: &str = "http://www.w3.org/2001/XMLSchema";

/// Documents by location; an import without a location is found by its namespace.
#[derive(Default)]
struct Files {
    files: HashMap<String, String>,
    by_namespace: HashMap<String, String>,
    asked: Vec<(LoadKind, String)>,
}

impl Files {
    fn new(files: &[(&str, String)]) -> Self {
        Self { files: files.iter().map(|(l, t)| ((*l).to_owned(), t.clone())).collect(), ..Self::default() }
    }
}

fn join(base: &str, relative: &str) -> String {
    if relative.starts_with('/') {
        return relative.to_owned();
    }
    match base.rfind('/') {
        Some(i) => format!("{}{relative}", &base[..=i]),
        None => relative.to_owned(),
    }
}

impl SchemaLoader for Files {
    fn load(&mut self, request: &LoadRequest<'_>) -> Result<Loaded, String> {
        let location = match (request.location, request.namespace) {
            (Some(l), _) => join(request.base, l),
            (None, Some(ns)) => self.by_namespace.get(ns).cloned().ok_or_else(|| format!("no document for {ns}"))?,
            (None, None) => return Err("nothing to look for".into()),
        };
        self.asked.push((request.kind, location.clone()));
        self.files
            .get(&location)
            .map(|text| Loaded { text: text.clone(), location: location.clone() })
            .ok_or_else(|| format!("{location}: no such file"))
    }
}

fn schema(target: Option<&str>, body: &str) -> String {
    let tns = target.map(|t| format!(" targetNamespace=\"{t}\"")).unwrap_or_default();
    format!("<xs:schema xmlns:xs=\"{XS}\"{tns}>{body}</xs:schema>")
}

fn locations(d: &Documents) -> Vec<&str> {
    d.list.iter().map(|d| d.location.as_str()).collect()
}

fn keys(d: &Documents) -> Vec<&'static str> {
    d.diagnostics.iter().map(|d| d.diagnostic.code.key()).collect()
}

fn run(root: &str, files: &mut Files) -> Documents {
    load(root, "/s/root.xsd", files)
}

#[test]
fn a_schema_of_one_document() {
    let docs = run(&schema(Some("urn:a"), "<xs:element name=\"e\"/>"), &mut Files::default());
    assert_eq!(locations(&docs), ["/s/root.xsd"]);
    assert!(docs.diagnostics.is_empty());
    assert_eq!((docs.list[0].namespace.as_deref(), docs.list[0].chameleon, docs.list[0].asked_by), (Some("urn:a"), false, None));
}

#[test]
fn a_root_that_is_not_a_schema_has_no_documents_and_says_why() {
    let docs = run("<a/>", &mut Files::default());
    assert!(docs.list.is_empty());
    assert_eq!(keys(&docs), ["xsd.not-a-schema"]);
}

#[test]
fn includes_are_followed_in_the_namespace_of_the_includer() {
    let mut files = Files::new(&[
        ("/s/b.xsd", schema(Some("urn:a"), "<xs:include schemaLocation=\"sub/c.xsd\"/><xs:element name=\"b\"/>")),
        ("/s/sub/c.xsd", schema(Some("urn:a"), "<xs:element name=\"c\"/>")),
    ]);
    let docs = run(&schema(Some("urn:a"), "<xs:include schemaLocation=\"b.xsd\"/>"), &mut files);
    assert_eq!(locations(&docs), ["/s/root.xsd", "/s/b.xsd", "/s/sub/c.xsd"], "relative to the document that includes");
    assert!(docs.diagnostics.is_empty(), "{:?}", keys(&docs));
    assert_eq!((docs.list[1].asked_by, docs.list[2].asked_by), (Some(0), Some(1)));
    assert_eq!((docs.list[0].includes.clone(), docs.list[1].includes.clone()), (vec![1], vec![2]));
    assert!(docs.list.iter().all(|d| d.namespace.as_deref() == Some("urn:a") && !d.chameleon));
}

#[test]
fn a_document_without_a_namespace_takes_that_of_the_document_that_includes_it() {
    let mut files = Files::new(&[("/s/c.xsd", schema(None, "<xs:element name=\"c\"/>"))]);
    let docs = run(&schema(Some("urn:a"), "<xs:include schemaLocation=\"c.xsd\"/>"), &mut files);
    assert!(docs.diagnostics.is_empty(), "{:?}", keys(&docs));
    assert_eq!((docs.list[1].namespace.as_deref(), docs.list[1].chameleon), (Some("urn:a"), true));
    // Included into a schema with no namespace either, it is not a chameleon.
    let docs = run(&schema(None, "<xs:include schemaLocation=\"c.xsd\"/>"), &mut files);
    assert_eq!((docs.list[1].namespace.as_deref(), docs.list[1].chameleon), (None, false));
}

#[test]
fn a_chameleon_document_is_read_once_for_each_namespace_it_is_included_into() {
    let mut files = Files::new(&[
        ("/s/c.xsd", schema(None, "<xs:element name=\"c\"/>")),
        ("/s/a.xsd", schema(Some("urn:a"), "<xs:include schemaLocation=\"c.xsd\"/>")),
        ("/s/a2.xsd", schema(Some("urn:a"), "<xs:include schemaLocation=\"c.xsd\"/>")),
        ("/s/b.xsd", schema(Some("urn:b"), "<xs:include schemaLocation=\"c.xsd\"/>")),
    ]);
    let root = schema(
        Some("urn:a"),
        "<xs:include schemaLocation=\"a.xsd\"/><xs:include schemaLocation=\"a2.xsd\"/><xs:import namespace=\"urn:b\" schemaLocation=\"b.xsd\"/>",
    );
    let docs = run(&root, &mut files);
    assert!(docs.diagnostics.is_empty(), "{:?}", keys(&docs));
    let chameleons: Vec<_> = docs.list.iter().filter(|d| d.chameleon).map(|d| d.namespace.clone().unwrap_or_default()).collect();
    assert_eq!(chameleons, ["urn:a", "urn:b"], "once for urn:a (included twice) and once for urn:b");
}

#[test]
fn a_document_in_another_namespace_cannot_be_included() {
    let mut files = Files::new(&[("/s/b.xsd", schema(Some("urn:other"), ""))]);
    let docs = run(&schema(Some("urn:a"), "<xs:include schemaLocation=\"b.xsd\"/>"), &mut files);
    assert_eq!(keys(&docs), ["xsd.include-namespace"]);
    assert_eq!(docs.list.len(), 1);
    let docs = run(&schema(None, "<xs:include schemaLocation=\"b.xsd\"/>"), &mut files);
    assert_eq!(keys(&docs), ["xsd.include-namespace"]);
}

#[test]
fn includes_that_go_round_in_a_circle_end() {
    let mut files = Files::new(&[
        ("/s/a.xsd", schema(Some("urn:a"), "<xs:include schemaLocation=\"root.xsd\"/><xs:include schemaLocation=\"b.xsd\"/>")),
        ("/s/b.xsd", schema(Some("urn:a"), "<xs:include schemaLocation=\"a.xsd\"/>")),
    ]);
    let root = schema(Some("urn:a"), "<xs:include schemaLocation=\"a.xsd\"/><xs:include schemaLocation=\"root.xsd\"/>");
    files.files.insert("/s/root.xsd".into(), root.clone());
    let docs = run(&root, &mut files);
    assert_eq!(locations(&docs), ["/s/root.xsd", "/s/a.xsd", "/s/b.xsd"]);
    assert!(docs.diagnostics.is_empty(), "{:?}", keys(&docs));
    assert!(docs.list[1].includes.contains(&0), "a document that includes the root is linked to it");
    assert_eq!(files.asked.iter().filter(|(_, l)| l == "/s/b.xsd").count(), 1);
}

#[test]
fn a_document_reached_two_ways_is_read_once() {
    let mut files = Files::new(&[
        ("/s/b.xsd", schema(Some("urn:a"), "<xs:include schemaLocation=\"d.xsd\"/>")),
        ("/s/c.xsd", schema(Some("urn:a"), "<xs:include schemaLocation=\"d.xsd\"/>")),
        ("/s/d.xsd", schema(Some("urn:a"), "<xs:element name=\"d\"/>")),
    ]);
    let docs = run(&schema(Some("urn:a"), "<xs:include schemaLocation=\"b.xsd\"/><xs:include schemaLocation=\"c.xsd\"/>"), &mut files);
    assert_eq!(locations(&docs).iter().filter(|l| l.ends_with("d.xsd")).count(), 1);
    assert_eq!((docs.list[1].includes.clone(), docs.list[2].includes.clone()), (vec![3], vec![3]));
}

#[test]
fn imports_name_the_namespace_of_the_document() {
    let mut files = Files::new(&[("/s/o.xsd", schema(Some("urn:o"), "<xs:element name=\"o\"/>"))]);
    let docs = run(&schema(Some("urn:a"), "<xs:import namespace=\"urn:o\" schemaLocation=\"o.xsd\"/>"), &mut files);
    assert!(docs.diagnostics.is_empty(), "{:?}", keys(&docs));
    assert_eq!(locations(&docs), ["/s/root.xsd", "/s/o.xsd"]);
    assert_eq!(docs.list[0].imports, vec![Some("urn:o".to_owned())]);
    assert!(docs.list[0].includes.is_empty(), "an import is not an include");
    assert_eq!(docs.list[1].namespace.as_deref(), Some("urn:o"));
    // Found by its namespace when there is no location.
    let mut files = Files::new(&[("/lib/o.xsd", schema(Some("urn:o"), ""))]);
    files.by_namespace.insert("urn:o".into(), "/lib/o.xsd".into());
    let docs = run(&schema(Some("urn:a"), "<xs:import namespace=\"urn:o\"/>"), &mut files);
    assert_eq!(locations(&docs), ["/s/root.xsd", "/lib/o.xsd"]);
    // A schema without a namespace imports the namespace of another.
    let docs = run(&schema(None, "<xs:import namespace=\"urn:o\"/>"), &mut files);
    assert!(docs.diagnostics.is_empty(), "{:?}", keys(&docs));
}

#[test]
fn mistakes_in_imports() {
    let mut files = Files::new(&[("/s/o.xsd", schema(Some("urn:o"), "")), ("/s/n.xsd", schema(None, ""))]);
    let docs = run(&schema(Some("urn:a"), "<xs:import namespace=\"urn:x\" schemaLocation=\"o.xsd\"/>"), &mut files);
    assert_eq!(keys(&docs), ["xsd.import-namespace-mismatch"]);
    assert_eq!(docs.list.len(), 1);
    let docs = run(&schema(Some("urn:a"), "<xs:import namespace=\"urn:a\" schemaLocation=\"o.xsd\"/>"), &mut files);
    assert_eq!(keys(&docs), ["xsd.import-own-namespace"]);
    let docs = run(&schema(None, "<xs:import schemaLocation=\"o.xsd\"/>"), &mut files);
    assert!(keys(&docs).contains(&"xsd.import-without-namespace"), "{:?}", keys(&docs));
    // Without a namespace given, the document must have none.
    let docs = run(&schema(Some("urn:a"), "<xs:import schemaLocation=\"n.xsd\"/>"), &mut files);
    assert!(docs.diagnostics.is_empty(), "{:?}", keys(&docs));
    assert_eq!(docs.list.len(), 2);
    assert_eq!((docs.list[1].namespace.clone(), docs.list[1].chameleon), (None, false), "an imported document keeps its (absent) namespace");
    let docs = run(&schema(Some("urn:a"), "<xs:import schemaLocation=\"o.xsd\"/>"), &mut files);
    assert_eq!(keys(&docs), ["xsd.import-namespace-mismatch"]);
    // An import with nothing to look for loads nothing and is no mistake.
    let docs = run(&schema(Some("urn:a"), "<xs:import/>"), &mut files);
    assert!(docs.diagnostics.is_empty() && docs.list.len() == 1, "{:?}", keys(&docs));
}

#[test]
fn a_document_that_cannot_be_had_is_reported_and_an_import_only_as_a_warning() {
    let mut files = Files::default();
    let docs = run(&schema(Some("urn:a"), "<xs:include schemaLocation=\"gone.xsd\"/>"), &mut files);
    assert_eq!(keys(&docs), ["xsd.document-not-loaded"]);
    assert!(docs.diagnostics[0].diagnostic.message().contains("no such file"), "{}", docs.diagnostics[0].diagnostic.message());
    let docs = run(&schema(Some("urn:a"), "<xs:redefine schemaLocation=\"gone.xsd\"/>"), &mut files);
    assert_eq!(keys(&docs), ["xsd.document-not-loaded"]);
    let docs = run(&schema(Some("urn:a"), "<xs:import namespace=\"urn:o\" schemaLocation=\"gone.xsd\"/>"), &mut files);
    assert_eq!(keys(&docs), ["xsd.import-not-loaded"]);
    assert_eq!(docs.diagnostics[0].diagnostic.severity(), Severity::Warning);
    // Not a schema.
    let mut files = Files::new(&[("/s/x.xsd", "<other/>".to_owned())]);
    let docs = run(&schema(Some("urn:a"), "<xs:include schemaLocation=\"x.xsd\"/>"), &mut files);
    assert_eq!(keys(&docs), ["xsd.not-a-schema-document"]);
}

#[test]
fn redefinitions_load_the_document_like_includes() {
    let mut files =
        Files::new(&[("/s/b.xsd", schema(Some("urn:a"), "<xs:simpleType name=\"s\"><xs:restriction base=\"xs:string\"/></xs:simpleType>"))]);
    let docs = run(
        &schema(
            Some("urn:a"),
            "<xs:redefine schemaLocation=\"b.xsd\"><xs:simpleType name=\"s\"><xs:restriction base=\"s\"><xs:maxLength value=\"3\"/></xs:restriction></xs:simpleType></xs:redefine>",
        ),
        &mut files,
    );
    assert!(docs.diagnostics.is_empty(), "{:?}", keys(&docs));
    assert_eq!(locations(&docs), ["/s/root.xsd", "/s/b.xsd"]);
    assert_eq!(docs.list[0].includes, vec![1]);
    assert_eq!(files.asked[0].0, LoadKind::Redefine);
}

#[test]
fn the_mistakes_of_every_document_are_kept_with_its_number() {
    let mut files = Files::new(&[("/s/b.xsd", schema(Some("urn:a"), "<xs:element/>"))]);
    let docs = run(&schema(Some("urn:a"), "<xs:include schemaLocation=\"b.xsd\"/><xs:bogus/>"), &mut files);
    let found: Vec<(usize, &str)> = docs.diagnostics.iter().map(|d| (d.source, d.diagnostic.code.key())).collect();
    assert_eq!(found, [(0, "xsd.unexpected-element"), (1, "xsd.missing-attribute")]);
}

#[test]
fn a_chain_of_documents_ends_at_the_limit() {
    let mut files = Files::default();
    for i in 0..(MAX_DOCUMENTS + 50) {
        files.files.insert(format!("/s/d{i}.xsd"), schema(Some("urn:a"), &format!("<xs:include schemaLocation=\"d{}.xsd\"/>", i + 1)));
    }
    let docs = run(&schema(Some("urn:a"), "<xs:include schemaLocation=\"d0.xsd\"/>"), &mut files);
    assert_eq!(docs.list.len(), MAX_DOCUMENTS);
    assert!(keys(&docs).contains(&"xsd.too-many-documents"), "{:?}", keys(&docs));
}

#[test]
fn a_schema_may_not_be_made_of_too_much_text() {
    let big = schema(Some("urn:a"), &format!("<xs:annotation><xs:documentation>{}</xs:documentation></xs:annotation>", "x".repeat(20 * 1024 * 1024)));
    let mut files = Files::new(&[("/s/1.xsd", big.clone()), ("/s/2.xsd", big.clone()), ("/s/3.xsd", big.clone()), ("/s/4.xsd", big)]);
    let root = schema(
        Some("urn:a"),
        "<xs:include schemaLocation=\"1.xsd\"/><xs:include schemaLocation=\"2.xsd\"/><xs:include schemaLocation=\"3.xsd\"/><xs:include schemaLocation=\"4.xsd\"/>",
    );
    let docs = run(&root, &mut files);
    assert_eq!(docs.list.len(), 4, "three fit in 64 MiB, the fourth does not");
    assert!(keys(&docs).contains(&"xsd.too-many-documents"), "{:?}", keys(&docs));
}

#[test]
fn including_a_document_twice_links_it_once() {
    let mut files = Files::new(&[("/s/b.xsd", schema(Some("urn:a"), ""))]);
    let docs = run(
        &schema(Some("urn:a"), "<xs:include schemaLocation=\"b.xsd\"/><xs:include schemaLocation=\"b.xsd\"/><xs:include schemaLocation=\"b.xsd\"/>"),
        &mut files,
    );
    assert_eq!(docs.list.len(), 2);
    assert_eq!(docs.list[0].includes, vec![1]);
}
