//! Loading the documents of a schema through the resolver: open documents, files, the catalogs and the bundle.

use std::collections::HashMap;
use std::sync::Arc;

use xmlcraft_resolve::{Buffers, Environment, FileAccess, FixedClock, Policy, Resolver};
use xmlcraft_xsd::model::QName;
use xmlcraft_xsd::{ResolvingLoader, XSD_NAMESPACE, assemble, load};

struct Open(HashMap<String, Arc<str>>);

impl Buffers for Open {
    fn text(&self, location: &str) -> Option<Arc<str>> {
        self.0.get(location).cloned()
    }
}

struct Files(HashMap<String, Vec<u8>>);

impl FileAccess for Files {
    fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        self.0.get(path).cloned().ok_or_else(|| format!("{path}: no such file"))
    }
}

fn resolver(open: &[(&str, &str)], files: &[(&str, &[u8])]) -> Resolver {
    let env = Environment {
        buffers: Arc::new(Open(open.iter().map(|(l, t)| ((*l).to_owned(), Arc::from(*t))).collect())),
        files: Arc::new(Files(files.iter().map(|(p, b)| ((*p).to_owned(), b.to_vec())).collect())),
        fetcher: None,
        cache: None,
        clock: Arc::new(FixedClock(0)),
    };
    Resolver::new(env, Policy { offline: true, ..Policy::default() }, &[])
}

fn schema(body: &str) -> String {
    format!("<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\">{body}</xs:schema>")
}

#[test]
fn an_included_document_is_found_next_to_the_one_that_includes_it() {
    let inc = schema("<xs:simpleType name=\"code\"><xs:restriction base=\"xs:string\"><xs:length value=\"3\"/></xs:restriction></xs:simpleType>");
    let root = schema("<xs:include schemaLocation=\"inc.xsd\"/><xs:element name=\"e\" type=\"code\"/>");
    let r = resolver(&[], &[("/s/inc.xsd", inc.as_bytes())]);
    let mut loader = ResolvingLoader::new(&r);
    let documents = load(&root, "/s/root.xsd", &mut loader);
    assert!(documents.diagnostics.is_empty(), "{:?}", documents.diagnostics);
    assert_eq!(documents.list.len(), 2);
    assert!(assemble(&documents).diagnostics.is_empty());
    assert!(loader.log.iter().any(|l| l.contains("inc.xsd: found at /s/inc.xsd")), "{:?}", loader.log);
}

#[test]
fn a_document_that_is_open_is_used_as_it_is_not_as_it_was_saved() {
    let saved = schema("<xs:simpleType name=\"code\"><xs:restriction base=\"xs:string\"/></xs:simpleType>");
    let edited = schema(
        "<xs:simpleType name=\"code\"><xs:restriction base=\"xs:string\"><xs:length value=\"2\"/></xs:restriction></xs:simpleType><xs:simpleType name=\"other\"><xs:restriction base=\"xs:int\"/></xs:simpleType>",
    );
    let root = schema("<xs:include schemaLocation=\"inc.xsd\"/>");
    let r = resolver(&[("/s/inc.xsd", &edited)], &[("/s/inc.xsd", saved.as_bytes())]);
    let documents = load(&root, "/s/root.xsd", &mut ResolvingLoader::new(&r));
    let assembly = assemble(&documents);
    assert!(assembly.schema.type_by_name(&QName::new(None, "other")).is_some(), "the unsaved text is what counts");
}

#[test]
fn an_import_without_a_location_is_found_by_its_namespace_in_the_bundle() {
    let root = format!(
        "<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\" xmlns:xml=\"http://www.w3.org/XML/1998/namespace\">\
         <xs:import namespace=\"http://www.w3.org/XML/1998/namespace\"/>\
         <xs:complexType name=\"t\"><xs:attribute ref=\"xml:lang\"/></xs:complexType></xs:schema>"
    );
    let r = resolver(&[], &[]);
    let mut loader = ResolvingLoader::new(&r);
    let documents = load(&root, "/s/root.xsd", &mut loader);
    assert!(documents.diagnostics.is_empty(), "{:?} {:?}", documents.diagnostics, loader.log);
    let assembly = assemble(&documents);
    assert!(assembly.diagnostics.is_empty(), "{:?}", assembly.diagnostics.iter().map(|d| d.diagnostic.message()).collect::<Vec<_>>());
    assert!(assembly.schema.attribute_by_name(&QName::new(Some("http://www.w3.org/XML/1998/namespace"), "lang")).is_some());
}

#[test]
fn a_document_that_cannot_be_found_is_reported_with_why() {
    let root = schema("<xs:include schemaLocation=\"missing.xsd\"/>");
    let r = resolver(&[], &[]);
    let documents = load(&root, "/s/root.xsd", &mut ResolvingLoader::new(&r));
    assert_eq!(documents.diagnostics.len(), 1);
    assert_eq!(documents.diagnostics[0].diagnostic.code.key(), "xsd.document-not-loaded");
    assert!(documents.diagnostics[0].diagnostic.message().contains("missing.xsd"), "{}", documents.diagnostics[0].diagnostic.message());
}

#[test]
fn an_encoding_is_read_by_its_byte_order_mark_and_a_broken_one_is_refused() {
    let inc = schema("<xs:simpleType name=\"code\"><xs:restriction base=\"xs:string\"/></xs:simpleType>");
    let mut utf16: Vec<u8> = vec![0xFF, 0xFE];
    for unit in inc.encode_utf16() {
        utf16.extend(unit.to_le_bytes());
    }
    let root = schema("<xs:include schemaLocation=\"inc.xsd\"/>");
    let r = resolver(&[], &[("/s/inc.xsd", &utf16)]);
    let documents = load(&root, "/s/root.xsd", &mut ResolvingLoader::new(&r));
    assert!(documents.diagnostics.is_empty(), "{:?}", documents.diagnostics);
    // A UTF-8 file with a byte that is not UTF-8 is not read as if it were.
    let r = resolver(&[], &[("/s/inc.xsd", b"<xs:schema xmlns:xs=\"x\">\xff</xs:schema>")]);
    let documents = load(&root, "/s/root.xsd", &mut ResolvingLoader::new(&r));
    assert_eq!(documents.diagnostics.len(), 1, "{:?}", documents.diagnostics);
    assert!(documents.diagnostics[0].diagnostic.message().contains("not valid in its encoding"));
}
