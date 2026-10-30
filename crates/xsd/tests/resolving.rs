//! Loading the documents of a schema through the resolver: open documents, files, the catalogs and the bundle.

use std::collections::HashMap;
use std::sync::Arc;

use xmlcraft_resolve::{Buffers, Environment, FileAccess, FixedClock, Policy, Resolver};
use xmlcraft_xsd::model::QName;
use xmlcraft_xsd::{LoadKind, LoadRequest, ResolvingLoader, SchemaLoader, XSD_NAMESPACE, assemble, load};

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
    // What was read is kept, to say where in it a problem is.
    assert_eq!(loader.read, [("/s/inc.xsd".to_owned(), inc.clone())]);
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
    let mut loader = ResolvingLoader::new(&r);
    let documents = load(&root, "/s/root.xsd", &mut loader);
    assert_eq!(documents.diagnostics.len(), 1);
    assert_eq!(documents.diagnostics[0].diagnostic.code.key(), "xsd.document-not-loaded");
    assert!(documents.diagnostics[0].diagnostic.message().contains("missing.xsd"), "{}", documents.diagnostics[0].diagnostic.message());
    assert!(loader.log.iter().any(|l| l.starts_with("missing.xsd: ")), "what was asked for and why it was not found is kept: {:?}", loader.log);
    assert!(loader.read.is_empty());
}

#[test]
fn the_location_is_tried_before_the_namespace_and_every_reason_is_given() {
    // The XML namespace is in the bundle, but the document says where its schema is: that is what is read.
    let xml_ns = "http://www.w3.org/XML/1998/namespace";
    let own =
        format!("<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\" targetNamespace=\"{xml_ns}\"><xs:attribute name=\"other\" type=\"xs:string\"/></xs:schema>");
    let root = schema(&format!("<xs:import namespace=\"{xml_ns}\" schemaLocation=\"own.xsd\"/>"));
    let r = resolver(&[], &[("/s/own.xsd", own.as_bytes())]);
    let documents = load(&root, "/s/root.xsd", &mut ResolvingLoader::new(&r));
    let assembly = assemble(&documents);
    assert!(assembly.schema.attribute_by_name(&QName::new(Some(xml_ns), "other")).is_some());
    assert!(assembly.schema.attribute_by_name(&QName::new(Some(xml_ns), "lang")).is_none());
    // Neither a location nor a namespace: nothing to look for. Both given and neither found: both reasons.
    let r = resolver(&[], &[]);
    let mut loader = ResolvingLoader::new(&r);
    let ask = |loader: &mut ResolvingLoader<'_>, namespace, location| {
        loader.load(&LoadRequest { kind: LoadKind::Import, namespace, location, base: "/s/root.xsd" })
    };
    let why = ask(&mut loader, None, None).expect_err("nothing to look for");
    assert_eq!(why, "it names neither a location nor a namespace");
    let why = ask(&mut loader, Some("urn:nowhere"), Some("gone.xsd")).expect_err("not found");
    let (first, second) = why.split_once("; ").expect("one reason for each");
    assert!(first.contains("gone.xsd") && second.contains("urn:nowhere"), "{why}");
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

/// A web that counts what it is asked and has a schema at one address.
struct Web(std::sync::Mutex<Vec<String>>);

impl xmlcraft_resolve::Fetcher for Web {
    fn fetch(
        &self,
        url: &str,
        _: &xmlcraft_resolve::Conditions,
        _: &xmlcraft_resolve::FetchLimits,
    ) -> Result<xmlcraft_resolve::FetchOutcome, xmlcraft_resolve::FetchError> {
        self.0.lock().expect("lock").push(url.to_owned());
        let body = schema("<xs:element name=\"e\" type=\"xs:string\"/>").into_bytes();
        Ok(xmlcraft_resolve::FetchOutcome::Fresh(xmlcraft_resolve::Fetched { body, final_url: url.to_owned(), etag: None, last_modified: None }))
    }
}

fn online(web: &Arc<Web>) -> Resolver {
    let env = Environment {
        buffers: Arc::new(Open(HashMap::new())),
        files: Arc::new(Files(HashMap::new())),
        fetcher: Some(web.clone()),
        cache: None,
        clock: Arc::new(FixedClock(0)),
    };
    Resolver::new(env, Policy::default(), &[])
}

#[test]
fn a_location_is_downloaded_but_a_namespace_is_a_name_that_is_not_looked_for_on_the_web() {
    let web = Arc::new(Web(std::sync::Mutex::new(Vec::new())));
    let r = online(&web);
    let mut loader = ResolvingLoader::new(&r);
    let ask = |loader: &mut ResolvingLoader<'_>, namespace, location| {
        loader.load(&LoadRequest { kind: LoadKind::Import, namespace, location, base: "/s/root.xsd" })
    };
    // The namespace alone, or after a location that could not be read: the catalogs know it or nothing does.
    assert!(ask(&mut loader, Some("http://example.com/ns"), None).is_err());
    let why = ask(&mut loader, Some("http://example.com/ns"), Some("")).expect_err("an empty location names nothing");
    assert!(!why.contains("; "), "there is one reason, for the namespace: {why}");
    assert!(web.0.lock().expect("lock").is_empty(), "{:?}", web.0.lock().expect("lock"));
    // A namespace that the bundle knows is found without a location.
    let found = ask(&mut loader, Some(XSD_NAMESPACE), None).expect("the bundle knows it");
    assert!(found.location.starts_with("bundled:/"), "{}", found.location);
    // A location is an address: downloaded, and the namespace is then not asked for.
    let loaded = ask(&mut loader, Some("http://example.com/ns"), Some("http://example.com/s/ns.xsd")).expect("downloaded");
    assert!(loaded.location.contains("example.com/s/ns.xsd"), "{}", loaded.location);
    assert!(web.0.lock().expect("lock").iter().all(|u| u.contains("/s/ns.xsd")), "{:?}", web.0.lock().expect("lock"));
}
