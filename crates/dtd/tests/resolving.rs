//! The external parts of a DTD found through the resolver: open documents, catalogs, the bundle, and files.

use std::collections::HashMap;
use std::sync::Arc;

use xmlcraft_dtd::{LoadRequest, MAX_EXTERNAL_BYTES, ResolvingSubsetLoader, SubsetLoader, compile, read_doctype};
use xmlcraft_grammar::Name;
use xmlcraft_resolve::{Buffers, Environment, FileAccess, FixedClock, NoBuffers, Policy, Resolver};
use xmlcraft_syntax::parse;

struct Files(HashMap<String, Vec<u8>>);

impl FileAccess for Files {
    fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        self.0.get(path).cloned().ok_or_else(|| format!("{path}: no such file"))
    }
}

struct Open(HashMap<String, Arc<str>>);

impl Buffers for Open {
    fn text(&self, location: &str) -> Option<Arc<str>> {
        self.0.get(location).cloned()
    }
}

fn resolver(files: &[(&str, &[u8])], open: &[(&str, &str)], catalogs: &[&str]) -> Resolver {
    let buffers: Arc<dyn Buffers> =
        if open.is_empty() { Arc::new(NoBuffers) } else { Arc::new(Open(open.iter().map(|(l, t)| ((*l).to_owned(), Arc::from(*t))).collect())) };
    let env = Environment {
        buffers,
        files: Arc::new(Files(files.iter().map(|(p, b)| ((*p).to_owned(), b.to_vec())).collect())),
        fetcher: None,
        cache: None,
        clock: Arc::new(FixedClock(0)),
    };
    let catalogs: Vec<String> = catalogs.iter().map(|c| (*c).to_owned()).collect();
    Resolver::new(env, Policy { auto_download: false, offline: true, ..Policy::default() }, &catalogs)
}

fn request<'a>(public: Option<&'a str>, system: &'a str, base: &'a str) -> LoadRequest<'a> {
    LoadRequest { kind: "external subset", public, system, base }
}

#[test]
fn a_relative_system_identifier_is_a_file_next_to_the_document() {
    let r = resolver(&[("/d/a.dtd", b"<!ELEMENT a EMPTY>")], &[], &[]);
    let mut loader = ResolvingSubsetLoader::new(&r);
    let loaded = loader.load(&request(None, "a.dtd", "/d/doc.xml")).expect("found");
    assert_eq!((loaded.text.as_str(), loaded.location.as_str()), ("<!ELEMENT a EMPTY>", "/d/a.dtd"));
    assert_eq!(loader.texts(), [("/d/a.dtd".to_owned(), "<!ELEMENT a EMPTY>".to_owned())], "what was read is kept, to say where its problems are");
    let why = loader.load(&request(None, "gone.dtd", "/d/doc.xml")).err().expect("not there");
    assert!(why.contains("gone.dtd"), "{why}");
}

#[test]
fn a_dtd_that_is_open_is_used_as_it_is_being_edited() {
    let r = resolver(&[("/d/a.dtd", b"<!ELEMENT a EMPTY>")], &[("/d/a.dtd", "<!ELEMENT a ANY>")], &[]);
    let loaded = ResolvingSubsetLoader::new(&r).load(&request(None, "a.dtd", "/d/doc.xml")).expect("found");
    assert_eq!(loaded.text, "<!ELEMENT a ANY>", "the open document comes before the file");
}

#[test]
fn a_public_identifier_the_app_knows_is_a_dtd_that_ships_with_it() {
    let r = resolver(&[], &[], &[]);
    let mut loader = ResolvingSubsetLoader::new(&r);
    let strict = loader
        .load(&request(Some("-//W3C//DTD XHTML 1.0 Strict//EN"), "http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd", ""))
        .expect("found in the bundle, with no file and no network");
    assert!(strict.text.contains("<!ELEMENT html"), "{}", &strict.text[..strict.text.len().min(200)]);
    // Its own relative references (the entity files it includes) are found next to it, in the bundle too.
    assert!(loader.texts().len() == 1);
    // The public identifier alone is enough: the system identifier is one nobody knows (a document may say where the DTD was
    // once, and the bundled one is still found).
    let by_public = ResolvingSubsetLoader::new(&r)
        .load(&request(Some("-//W3C//DTD XHTML 1.0 Strict//EN"), "http://example.invalid/old-place.dtd", ""))
        .expect("found");
    assert_eq!(by_public.text, strict.text);
    // A system identifier alone, as the catalogs know it.
    let by_system = ResolvingSubsetLoader::new(&r).load(&request(None, "http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd", "")).expect("found");
    assert_eq!(by_system.text, strict.text);
}

#[test]
fn a_catalog_of_the_user_maps_an_identifier_to_a_file_of_their_own() {
    let catalog = "<catalog xmlns=\"urn:oasis:names:tc:entity:xmlns:xml:catalog\"><system systemId=\"http://example.org/x.dtd\" uri=\"file:///c/x.dtd\"/></catalog>";
    let r = resolver(&[("/c/catalog.xml", catalog.as_bytes()), ("/c/x.dtd", b"<!ELEMENT x EMPTY>")], &[], &["/c/catalog.xml"]);
    let loaded = ResolvingSubsetLoader::new(&r).load(&request(None, "http://example.org/x.dtd", "/d/doc.xml")).expect("found through the catalog");
    assert_eq!(
        (loaded.text.as_str(), loaded.location.as_str()),
        ("<!ELEMENT x EMPTY>", "file:///c/x.dtd"),
        "found where the catalog said, and that is where its own relative references start"
    );
}

#[test]
fn nothing_is_downloaded_and_what_cannot_be_read_says_so() {
    let r = resolver(&[("/d/bad.dtd", &[0xFF, 0xFE, 0x00][..])], &[], &[]);
    let mut loader = ResolvingSubsetLoader::new(&r);
    let why = loader.load(&request(None, "http://example.org/unknown.dtd", "/d/doc.xml")).err().expect("not downloaded");
    assert!(!why.is_empty() && loader.texts().is_empty(), "{why}");
    let bytes = loader.load(&request(None, "bad.dtd", "/d/doc.xml"));
    assert!(bytes.is_err() && loader.texts().is_empty(), "a text with bytes its encoding does not allow is not used");
    // No system identifier at all (only a public one that nobody knows): refused, not a panic.
    assert!(loader.load(&request(Some("-//Nobody//DTD Nothing//EN"), "", "/d/doc.xml")).is_err());
}

#[test]
fn a_document_that_names_the_xhtml_dtd_by_its_public_identifier_has_its_grammar() {
    let text = "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd\"><html xmlns=\"http://www.w3.org/1999/xhtml\"/>";
    let parsed = parse(text);
    let r = resolver(&[], &[], &[]);
    let mut loader = ResolvingSubsetLoader::new(&r);
    let (dtd, root) = read_doctype(&parsed.syntax(), text, "", &mut loader).expect("a DTD");
    let grammar = compile(&dtd.dtd, root.as_deref()).grammar;
    for name in ["html", "head", "body", "p", "a", "table"] {
        assert!(grammar.element(&Name::local(name)).is_some(), "{name} is declared by the XHTML DTD");
    }
    assert!(
        loader.texts().len() > 1,
        "the entity files the DTD includes were read too: {:?}",
        loader.texts().iter().map(|(l, _)| l).collect::<Vec<_>>()
    );
}

#[test]
fn an_external_text_over_the_limit_is_refused_and_one_at_the_limit_is_not() {
    let at_limit = vec![b' '; MAX_EXTERNAL_BYTES];
    let over = vec![b' '; MAX_EXTERNAL_BYTES + 1];
    let r = resolver(&[("/d/ok.dtd", &at_limit), ("/d/big.dtd", &over)], &[], &[]);
    let mut loader = ResolvingSubsetLoader::new(&r);
    assert_eq!(loader.load(&request(None, "ok.dtd", "/d/doc.xml")).map(|l| l.text.len()), Ok(MAX_EXTERNAL_BYTES));
    let why = loader.load(&request(None, "big.dtd", "/d/doc.xml")).err().expect("too big");
    assert!(why.contains("over") && why.contains("big.dtd"), "{why}");
    assert_eq!(loader.texts().len(), 1, "what was refused is not kept");
}
