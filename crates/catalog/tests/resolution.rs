//! XML Catalogs 1.1 resolution: the order of the entries, the longest match, `prefer`, delegation and `nextCatalog`, `urn:publicid:`,
//! and reading catalogs that are odd or hostile.

use std::collections::HashMap;

use xmlcraft_catalog::{CATALOG_NS, Catalog, CatalogError, Catalogs, Entry, Loader, NoLoader};

fn catalog(attrs: &str, body: &str, location: &str) -> Catalog {
    let text = format!("<?xml version=\"1.0\"?>\n<catalog xmlns=\"{CATALOG_NS}\"{attrs}>{body}</catalog>");
    Catalog::parse(&text, location).unwrap_or_else(|e| panic!("{e}: {text}"))
}

fn catalogs(list: &[Catalog]) -> Catalogs {
    let mut all = Catalogs::new();
    for c in list {
        all.push(c.clone());
    }
    all
}

/// Catalogs by location, and how often each was asked for.
#[derive(Default)]
struct Files {
    files: HashMap<String, String>,
    asked: Vec<String>,
    /// What the loader was told about the catalogs that could not be used.
    notes: Vec<(String, String)>,
}

impl Files {
    fn with(mut self, location: &str, attrs: &str, body: &str) -> Self {
        self.files.insert(location.to_owned(), format!("<catalog xmlns=\"{CATALOG_NS}\"{attrs}>{body}</catalog>"));
        self
    }

    /// A file that is whatever it is.
    fn raw(mut self, location: &str, text: &str) -> Self {
        self.files.insert(location.to_owned(), text.to_owned());
        self
    }
}

impl Loader for Files {
    fn load(&mut self, location: &str) -> Option<String> {
        self.asked.push(location.to_owned());
        self.files.get(location).cloned()
    }

    fn note(&mut self, location: &str, problem: &str) {
        self.notes.push((location.to_owned(), problem.to_owned()));
    }
}

const HERE: &str = "file:///etc/xml/catalog.xml";

#[test]
fn a_system_entry_maps_its_identifier_and_the_uri_is_made_absolute() {
    let c = catalog("", r#"<system systemId="http://x.org/a.dtd" uri="dtds/a.dtd"/><system systemId="http://x.org/b.dtd" uri="/abs/b.dtd"/>"#, HERE);
    let all = catalogs(&[c]);
    assert_eq!(all.resolve(None, Some("http://x.org/a.dtd"), &mut NoLoader).as_deref(), Some("file:///etc/xml/dtds/a.dtd"));
    assert_eq!(all.resolve(None, Some("http://x.org/b.dtd"), &mut NoLoader).as_deref(), Some("file:///abs/b.dtd"));
    assert_eq!(all.resolve(None, Some("http://x.org/c.dtd"), &mut NoLoader), None);
    // Identifiers are compared as they are normalised: what may not stand in a URI is escaped, and an escape already there stays.
    assert_eq!(all.resolve(None, Some("http://x.org/%61.dtd"), &mut NoLoader), None, "`%61` and `a` are two names");
    let spaced = catalog("", r#"<system systemId="http://x.org/a b.dtd" uri="spaced.dtd"/>"#, HERE);
    assert_eq!(catalogs(&[spaced]).resolve(None, Some("http://x.org/a%20b.dtd"), &mut NoLoader).as_deref(), Some("file:///etc/xml/spaced.dtd"));
    // The first of two entries for one identifier wins.
    let twice = catalog("", r#"<system systemId="s" uri="first"/><system systemId="s" uri="second"/>"#, HERE);
    assert_eq!(catalogs(&[twice]).resolve(None, Some("s"), &mut NoLoader).as_deref(), Some("file:///etc/xml/first"));
}

#[test]
fn xml_base_changes_what_references_are_resolved_against_for_what_it_encloses() {
    let c = catalog(
        r#" xml:base="http://base.example/dir/""#,
        r#"<system systemId="a" uri="a.dtd"/><group xml:base="sub/"><system systemId="b" uri="b.dtd"/><system xml:base="/top/" systemId="c" uri="c.dtd"/></group><system systemId="d" uri="d.dtd"/>"#,
        HERE,
    );
    let all = catalogs(&[c]);
    let get = |id: &str| all.resolve(None, Some(id), &mut NoLoader);
    assert_eq!(get("a").as_deref(), Some("http://base.example/dir/a.dtd"));
    assert_eq!(get("b").as_deref(), Some("http://base.example/dir/sub/b.dtd"));
    assert_eq!(get("c").as_deref(), Some("http://base.example/top/c.dtd"));
    assert_eq!(get("d").as_deref(), Some("http://base.example/dir/d.dtd"), "the group's base ends with the group");
}

#[test]
fn the_entries_are_tried_in_the_order_of_the_spec_and_the_longest_match_wins() {
    let body = r#"
        <systemSuffix systemIdSuffix="a.dtd" uri="by-suffix"/>
        <rewriteSystem systemIdStartString="http://x.org/dtds/" rewritePrefix="/rewritten/"/>
        <rewriteSystem systemIdStartString="http://x.org/dtds/v2/" rewritePrefix="/v2/"/>
        <system systemId="http://x.org/dtds/exact.dtd" uri="by-system"/>
        <systemSuffix systemIdSuffix="/special/a.dtd" uri="by-longer-suffix"/>"#;
    let all = catalogs(&[catalog("", body, "file:///c/catalog.xml")]);
    let get = |id: &str| all.resolve(None, Some(id), &mut NoLoader);
    assert_eq!(get("http://x.org/dtds/exact.dtd").as_deref(), Some("file:///c/by-system"), "system before rewriteSystem");
    assert_eq!(get("http://x.org/dtds/other.dtd").as_deref(), Some("file:///rewritten/other.dtd"), "the prefix is replaced, the rest kept");
    assert_eq!(get("http://x.org/dtds/v2/x.dtd").as_deref(), Some("file:///v2/x.dtd"), "the longest start string");
    assert_eq!(get("http://y.org/a.dtd").as_deref(), Some("file:///c/by-suffix"));
    assert_eq!(get("http://y.org/special/a.dtd").as_deref(), Some("file:///c/by-longer-suffix"), "the longest suffix");
    assert_eq!(get("http://x.org/dtds/v2/a.dtd").as_deref(), Some("file:///v2/a.dtd"), "rewriteSystem before systemSuffix");
}

#[test]
fn a_public_identifier_is_compared_with_its_white_space_collapsed() {
    let all = catalogs(&[catalog("", r#"<public publicId="-//OASIS//DTD  DocBook XML V4.5//EN" uri="docbook.dtd"/>"#, HERE)]);
    assert_eq!(all.resolve(Some("-//OASIS//DTD DocBook\n XML V4.5//EN"), None, &mut NoLoader).as_deref(), Some("file:///etc/xml/docbook.dtd"));
    assert_eq!(all.resolve(Some("  -//OASIS//DTD DocBook XML V4.5//EN "), None, &mut NoLoader).as_deref(), Some("file:///etc/xml/docbook.dtd"));
    assert_eq!(all.resolve(Some("-//OASIS//DTD DocBook XML V4.4//EN"), None, &mut NoLoader), None);
    assert_eq!(all.resolve(None, None, &mut NoLoader), None);
    assert_eq!(all.resolve(Some("  "), Some(""), &mut NoLoader), None, "nothing to look up");
}

#[test]
fn prefer_decides_whether_a_public_entry_counts_when_a_system_identifier_is_given_too() {
    let public = r#"<public publicId="pid" uri="p.dtd"/>"#;
    let by_system = r#"<system systemId="sid" uri="s.dtd"/>"#;
    let ask = |all: &Catalogs| (all.resolve(Some("pid"), Some("other"), &mut NoLoader), all.resolve(Some("pid"), None, &mut NoLoader));
    // The application's choice where nothing says: public (the default here), or system.
    let mut unset = catalogs(&[catalog("", public, HERE)]);
    assert_eq!(ask(&unset), (Some("file:///etc/xml/p.dtd".into()), Some("file:///etc/xml/p.dtd".into())));
    unset.set_prefer_public(false);
    assert_eq!(ask(&unset), (None, Some("file:///etc/xml/p.dtd".into())), "without a system identifier a public entry always counts");
    // An attribute says: on the catalog, or on a group (which overrides it for what it holds).
    let system = catalogs(&[catalog(r#" prefer="system""#, public, HERE)]);
    assert_eq!(ask(&system).0, None);
    let public_attr = catalogs(&[catalog(r#" prefer="public""#, public, HERE)]);
    assert!(ask(&public_attr).0.is_some());
    let mut mixed = catalogs(&[catalog(r#" prefer="system""#, &format!(r#"<group prefer="public">{public}</group>"#), HERE)]);
    assert!(ask(&mixed).0.is_some(), "the group's prefer");
    mixed.set_prefer_public(false);
    assert!(ask(&mixed).0.is_some(), "an attribute beats the application's choice");
    // A matching system entry beats a public one whatever prefer says.
    let both = catalogs(&[catalog(r#" prefer="public""#, &format!("{public}{by_system}"), HERE)]);
    assert_eq!(both.resolve(Some("pid"), Some("sid"), &mut NoLoader).as_deref(), Some("file:///etc/xml/s.dtd"));
}

#[test]
fn a_delegated_catalog_answers_alone_and_a_failed_delegation_does_not_go_back_to_the_list() {
    let files = || {
        Files::default()
            .with("file:///d/short.xml", "", r#"<system systemId="http://x.org/dtds/a.dtd" uri="short-answer"/>"#)
            .with("file:///d/long.xml", "", r#"<system systemId="http://x.org/dtds/a.dtd" uri="long-answer"/>"#)
            .with("file:///d/empty.xml", "", "")
    };
    // The longest start string is asked first.
    let first = catalog(
        "",
        r#"<delegateSystem systemIdStartString="http://x.org/" catalog="file:///d/short.xml"/><delegateSystem systemIdStartString="http://x.org/dtds/" catalog="file:///d/long.xml"/>"#,
        HERE,
    );
    let all = catalogs(&[first]);
    assert_eq!(all.resolve(None, Some("http://x.org/dtds/a.dtd"), &mut files()).as_deref(), Some("file:///d/long-answer"));
    // When the delegates have nothing, a later catalog of the original list is not asked.
    let delegating = catalog("", r#"<delegateSystem systemIdStartString="http://x.org/" catalog="file:///d/empty.xml"/>"#, HERE);
    let later = catalog("", r#"<system systemId="http://x.org/a.dtd" uri="later"/>"#, "file:///later.xml");
    let all = catalogs(&[delegating, later]);
    assert_eq!(all.resolve(None, Some("http://x.org/a.dtd"), &mut files()), None);
    // A system identifier that no delegate entry matches goes on to the later catalog.
    assert_eq!(all.resolve(None, Some("http://other/a.dtd"), &mut files()), None);
    let all = catalogs(&[
        catalog("", r#"<delegateSystem systemIdStartString="http://x.org/" catalog="file:///d/empty.xml"/>"#, HERE),
        catalog("", r#"<system systemId="http://y.org/a.dtd" uri="later"/>"#, "file:///later.xml"),
    ]);
    assert_eq!(all.resolve(None, Some("http://y.org/a.dtd"), &mut files()).as_deref(), Some("file:///later"));
    // A delegate that cannot be loaded, or is not a catalog, or is not well-formed, is skipped (and the loader is told).
    let broken = Files::default()
        .with("file:///d/short.xml", "", r#"<system systemId="s" uri="ok"/>"#)
        .raw("file:///d/schema.xml", "<schema xmlns=\"http://www.w3.org/2001/XMLSchema\"/>")
        .raw("file:///d/torn.xml", "<catalog");
    let all = catalogs(&[catalog(
        "",
        r#"<delegateSystem systemIdStartString="s" catalog="file:///d/missing.xml"/><delegateSystem systemIdStartString="s" catalog="file:///d/schema.xml"/><delegateSystem systemIdStartString="s" catalog="file:///d/torn.xml"/><delegateSystem systemIdStartString="s" catalog="file:///d/short.xml"/>"#,
        HERE,
    )]);
    let mut broken = broken;
    assert_eq!(all.resolve(None, Some("s"), &mut broken).as_deref(), Some("file:///d/ok"));
    let told: Vec<&str> = broken.notes.iter().map(|(location, _)| location.rsplit('/').next().unwrap_or_default()).collect();
    assert_eq!(told, ["missing.xml", "schema.xml", "torn.xml"], "{:?}", broken.notes);
    assert!(broken.notes[1].1.contains("catalog"), "{:?}", broken.notes[1]);
    // A delegation that goes through many catalogs is followed (the limit is the number of loads).
    let mut chain = Files::default();
    for i in 0..40 {
        chain =
            chain.with(&format!("file:///h/{i}.xml"), "", &format!(r#"<delegateSystem systemIdStartString="s" catalog="file:///h/{}.xml"/>"#, i + 1));
    }
    chain = chain.with("file:///h/40.xml", "", r#"<system systemId="s" uri="end"/>"#);
    let start = catalogs(&[catalog("", r#"<delegateSystem systemIdStartString="s" catalog="file:///h/0.xml"/>"#, HERE)]);
    assert_eq!(start.resolve(None, Some("s"), &mut chain).as_deref(), Some("file:///h/end"));
    // Two catalogs that delegate to each other end, with no answer.
    let circle = Files::default().with("file:///h/a.xml", "", r#"<delegateSystem systemIdStartString="s" catalog="file:///h/b.xml"/>"#).with(
        "file:///h/b.xml",
        "",
        r#"<delegateSystem systemIdStartString="s" catalog="file:///h/a.xml"/>"#,
    );
    let start = catalogs(&[catalog("", r#"<delegateSystem systemIdStartString="s" catalog="file:///h/a.xml"/>"#, HERE)]);
    assert_eq!(start.resolve(None, Some("s"), &mut { circle }), None);
}

#[test]
fn delegation_restarts_with_only_the_identifier_that_matched() {
    // The delegated catalog has a system entry for the request's system identifier; a restart with the public identifier alone must
    // not see it, and one with the system identifier alone must not see the public entry.
    let delegated = Files::default().with(
        "file:///d/inner.xml",
        r#" prefer="public""#,
        r#"<system systemId="sid" uri="by-system"/><public publicId="pid" uri="by-public"/>"#,
    );
    let by_public = catalogs(&[catalog("", r#"<delegatePublic publicIdStartString="p" catalog="file:///d/inner.xml"/>"#, HERE)]);
    assert_eq!(by_public.resolve(Some("pid"), Some("sid"), &mut { delegated }).as_deref(), Some("file:///d/by-public"));
    let delegated = Files::default().with(
        "file:///d/inner.xml",
        r#" prefer="public""#,
        r#"<public publicId="pid" uri="by-public"/><system systemId="sid" uri="by-system"/>"#,
    );
    let by_system = catalogs(&[catalog("", r#"<delegateSystem systemIdStartString="s" catalog="file:///d/inner.xml"/>"#, HERE)]);
    assert_eq!(by_system.resolve(Some("pid"), Some("sid"), &mut { delegated }).as_deref(), Some("file:///d/by-system"));
    // And the identifier that did not match is not asked about at all: a delegated catalog with an entry for it alone has no answer.
    let only_public = Files::default().with("file:///d/inner.xml", r#" prefer="public""#, r#"<public publicId="pid" uri="by-public"/>"#);
    assert_eq!(
        by_system.resolve(Some("pid"), Some("sid"), &mut { only_public }),
        None,
        "the public identifier is dropped when a system one is delegated"
    );
    let only_system = Files::default().with("file:///d/inner.xml", "", r#"<system systemId="sid" uri="by-system"/>"#);
    assert_eq!(
        by_public.resolve(Some("pid"), Some("sid"), &mut { only_system }),
        None,
        "the system identifier is dropped when a public one is delegated"
    );
}

#[test]
fn next_catalogs_are_consulted_right_after_the_catalog_that_names_them_and_cycles_end() {
    let files = || {
        Files::default()
            .with("file:///n/one.xml", "", r#"<nextCatalog catalog="file:///n/three.xml"/><system systemId="only-in-one" uri="one"/>"#)
            .with("file:///n/two.xml", "", r#"<system systemId="both" uri="two"/><system systemId="only-in-two" uri="two"/>"#)
            .with("file:///n/three.xml", "", r#"<system systemId="both" uri="three"/><nextCatalog catalog="file:///n/one.xml"/>"#)
    };
    let first = catalog("", r#"<nextCatalog catalog="file:///n/one.xml"/><nextCatalog catalog="file:///n/two.xml"/>"#, "file:///n/first.xml");
    let all =
        catalogs(&[first, catalog("", r#"<system systemId="both" uri="last"/><system systemId="only-last" uri="last"/>"#, "file:///n/last.xml")]);
    let get = |id: &str| all.resolve(None, Some(id), &mut files());
    // one, then what one names (three), then two, then the later catalog of the list.
    assert_eq!(get("both").as_deref(), Some("file:///n/three"));
    assert_eq!(get("only-in-one").as_deref(), Some("file:///n/one"));
    assert_eq!(get("only-in-two").as_deref(), Some("file:///n/two"));
    assert_eq!(get("only-last").as_deref(), Some("file:///n/last"));
    assert_eq!(get("nowhere"), None, "three names one again: it is not read twice");
    // Each catalog is read once in a resolution.
    let mut f = files();
    let _ = all.resolve(None, Some("nowhere"), &mut f);
    let mut asked = f.asked.clone();
    asked.sort();
    asked.dedup();
    assert_eq!(f.asked.len(), asked.len(), "{:?}", f.asked);
}

#[test]
fn a_catalog_that_is_waiting_in_the_list_is_moved_up_to_follow_the_catalog_that_names_it() {
    // [first, middle, named]: `first` names `named`, which is the third of the list; it is consulted right after `first`.
    let first = catalog("", r#"<nextCatalog catalog="file:///n/named.xml"/>"#, "file:///n/first.xml");
    let middle = catalog("", r#"<system systemId="s" uri="from-middle"/>"#, "file:///n/middle.xml");
    let named = catalog("", r#"<system systemId="s" uri="from-named"/>"#, "file:///n/named.xml");
    let all = catalogs(&[first, middle, named]);
    assert_eq!(all.resolve(None, Some("s"), &mut NoLoader).as_deref(), Some("file:///n/from-named"));
    // One that has been consulted already is not followed again, and a delegated catalog does not read an original one anew.
    let a = catalog("", r#"<nextCatalog catalog="file:///n/b.xml"/>"#, "file:///n/a.xml");
    let b = catalog("", r#"<nextCatalog catalog="file:///n/a.xml"/>"#, "file:///n/b.xml");
    let mut files = Files::default();
    assert_eq!(catalogs(&[a, b]).resolve(None, Some("s"), &mut files), None);
    assert!(files.asked.is_empty(), "both are in the list already: {:?}", files.asked);
    let root = catalog("", r#"<delegateSystem systemIdStartString="s" catalog="file:///n/inner.xml"/>"#, "file:///n/root.xml");
    let mut files = Files::default().with("file:///n/inner.xml", "", r#"<nextCatalog catalog="file:///n/root.xml"/>"#).with(
        "file:///n/root.xml",
        "",
        r#"<system systemId="s" uri="from-root"/>"#,
    );
    assert_eq!(catalogs(&[root]).resolve(None, Some("s"), &mut files), None, "the root was consulted: it is not read again from the loader");
    assert_eq!(files.asked, ["file:///n/inner.xml"]);
}

#[test]
fn a_chain_of_catalogs_is_cut_off_and_a_catalog_that_names_itself_is_read_once() {
    let mut files = Files::default();
    for i in 0..200 {
        files = files.with(&format!("file:///c/{i}.xml"), "", &format!(r#"<nextCatalog catalog="file:///c/{}.xml"/>"#, i + 1));
    }
    let all = catalogs(&[catalog("", r#"<nextCatalog catalog="file:///c/0.xml"/>"#, HERE)]);
    assert_eq!(all.resolve(None, Some("x"), &mut files), None);
    assert_eq!(files.asked.len(), xmlcraft_catalog::MAX_LOADS, "no more than the limit are loaded");
    let selfish = Files::default().with("file:///s.xml", "", r#"<nextCatalog catalog="file:///s.xml"/>"#);
    let all = catalogs(&[catalog("", r#"<nextCatalog catalog="file:///s.xml"/>"#, HERE)]);
    let mut selfish = selfish;
    assert_eq!(all.resolve(None, Some("x"), &mut selfish), None);
    assert_eq!(selfish.asked, ["file:///s.xml"]);
}

#[test]
fn publicid_urns_are_unwrapped_in_requests_and_entries() {
    let all = catalogs(&[catalog(
        "",
        r#"<public publicId="-//OASIS//DTD DocBook XML V4.5//EN" uri="docbook.dtd"/><public publicId="urn:publicid:-:A:B:EN" uri="wrapped.dtd"/>"#,
        HERE,
    )]);
    let want = Some("file:///etc/xml/docbook.dtd");
    assert_eq!(all.resolve(Some("urn:publicid:-:OASIS:DTD+DocBook+XML+V4.5:EN"), None, &mut NoLoader).as_deref(), want);
    // A system identifier that is a URN is the public identifier, with no system identifier.
    assert_eq!(all.resolve(None, Some("urn:publicid:-:OASIS:DTD+DocBook+XML+V4.5:EN"), &mut NoLoader).as_deref(), want);
    assert_eq!(
        all.resolve(Some("-//OASIS//DTD DocBook XML V4.5//EN"), Some("urn:publicid:-:OASIS:DTD+DocBook+XML+V4.5:EN"), &mut NoLoader).as_deref(),
        want
    );
    // Both given and different: the system identifier is dropped, the public one is used.
    assert_eq!(all.resolve(Some("-//OASIS//DTD DocBook XML V4.5//EN"), Some("urn:publicid:something:else"), &mut NoLoader).as_deref(), want);
    // An entry written as a URN is the public identifier it unwraps to.
    assert_eq!(all.resolve(Some("-//A//B//EN"), None, &mut NoLoader).as_deref(), Some("file:///etc/xml/wrapped.dtd"));
    // A URI that is a publicid URN is looked up as a public identifier.
    assert_eq!(all.resolve_uri("urn:publicid:-:OASIS:DTD+DocBook+XML+V4.5:EN", &mut NoLoader).as_deref(), want);
}

#[test]
fn uris_are_resolved_by_uri_rewriteuri_urisuffix_and_delegateuri() {
    let files = Files::default().with("file:///u/ns.xml", "", r#"<uri name="http://ns.example/deep/schema" uri="deep.xsd"/>"#);
    let body = r#"
        <uri name="http://ns.example/a" uri="a.xsd"/>
        <rewriteURI uriStartString="http://ns.example/" rewritePrefix="/schemas/"/>
        <rewriteURI uriStartString="http://ns.example/long/" rewritePrefix="/long/"/>
        <uriSuffix uriSuffix=".xsd" uri="any.xsd"/>
        <delegateURI uriStartString="http://ns.example/deep/" catalog="file:///u/ns.xml"/>"#;
    let all = catalogs(&[catalog("", body, "file:///u/catalog.xml")]);
    let mut f = files;
    assert_eq!(all.resolve_uri("http://ns.example/a", &mut f).as_deref(), Some("file:///u/a.xsd"));
    assert_eq!(all.resolve_uri("http://ns.example/b", &mut f).as_deref(), Some("file:///schemas/b"));
    assert_eq!(all.resolve_uri("http://ns.example/long/c", &mut f).as_deref(), Some("file:///long/c"));
    assert_eq!(all.resolve_uri("http://other/x.xsd", &mut f).as_deref(), Some("file:///u/any.xsd"));
    assert_eq!(all.resolve_uri("http://other/x.dtd", &mut f), None);
    // rewriteURI is before delegateURI, which only gets what nothing before answered.
    let only_delegate =
        catalogs(&[catalog("", r#"<delegateURI uriStartString="http://ns.example/deep/" catalog="file:///u/ns.xml"/>"#, "file:///u/catalog.xml")]);
    assert_eq!(only_delegate.resolve_uri("http://ns.example/deep/schema", &mut f).as_deref(), Some("file:///u/deep.xsd"));
    // A URI and a system identifier are different things: `system` entries do not answer a URI.
    let system = catalogs(&[catalog("", r#"<system systemId="http://x/y" uri="z"/>"#, HERE)]);
    assert_eq!(system.resolve_uri("http://x/y", &mut NoLoader), None);
}

#[test]
fn what_is_not_a_catalog_is_refused_and_what_is_odd_inside_one_is_noted() {
    assert!(matches!(Catalog::parse("<catalog xmlns='urn:oasis:names:tc:entity:xmlns:xml:catalog'>", ""), Err(CatalogError::NotWellFormed(_))));
    assert_eq!(Catalog::parse("<catalog/>", ""), Err(CatalogError::NotACatalog), "no namespace");
    assert_eq!(Catalog::parse("<schema xmlns='http://www.w3.org/2001/XMLSchema'/>", ""), Err(CatalogError::NotACatalog));
    assert!(matches!(Catalog::parse("<!-- nothing -->", ""), Err(CatalogError::NotWellFormed(_))), "a document has a root element");
    let c = catalog(
        "",
        r#"<system systemId="only-one-attribute"/>
           <frobnicate x="1"/>
           <ext:thing xmlns:ext="urn:other"><system systemId="inside-another-namespace" uri="no"/></ext:thing>
           <system systemId="ok" uri="yes"/>"#,
        HERE,
    );
    assert_eq!(c.entries().len(), 1, "{:?}", c.entries());
    assert_eq!(c.warnings().len(), 2, "{:?}", c.warnings());
    assert!(c.warnings().iter().any(|w| w.contains("system")) && c.warnings().iter().any(|w| w.contains("frobnicate")));
    // A prefix may stand for the catalog namespace.
    let prefixed =
        Catalog::parse(&format!(r#"<c:catalog xmlns:c="{CATALOG_NS}"><c:system systemId="a" uri="b"/></c:catalog>"#), HERE).expect("a catalog");
    assert_eq!(prefixed.entries(), [Entry::System { id: "a".into(), uri: "file:///etc/xml/b".into() }]);
}

#[test]
fn an_entity_of_the_catalogs_own_is_not_expanded_so_what_uses_it_is_left_out() {
    let text = format!(
        "<!DOCTYPE catalog [<!ENTITY dir \"dtds/\">]><catalog xmlns=\"{CATALOG_NS}\"><system systemId=\"a\" uri=\"&dir;a.dtd\"/><system systemId=\"b\" uri=\"b.dtd\"/><group xml:base=\"&dir;\"><system systemId=\"c\" uri=\"c.dtd\"/></group></catalog>"
    );
    let c = Catalog::parse(&text, HERE).expect("a catalog");
    assert_eq!(c.entries(), [Entry::System { id: "b".into(), uri: "file:///etc/xml/b.dtd".into() }]);
    assert_eq!(c.warnings().len(), 2, "{:?}", c.warnings());
    assert!(c.warnings().iter().all(|w| w.contains("&dir;")), "{:?}", c.warnings());
}

#[test]
fn groups_up_to_the_depth_limit_are_read_and_the_warnings_say_when_they_were_cut() {
    let nested = |depth: usize| format!("{}<system systemId=\"deep\" uri=\"x\"/>{}", "<group>".repeat(depth), "</group>".repeat(depth));
    assert_eq!(catalog("", &nested(63), HERE).entries().len(), 1);
    assert_eq!(catalog("", &nested(64), HERE).entries().len(), 1, "64 groups deep is within the limit");
    let over = catalog("", &nested(65), HERE);
    assert!(over.entries().is_empty() && over.warnings().len() == 1, "{:?}", over.warnings());
    // A catalog that gives many reasons for warnings lists the first ones and says that it stopped.
    let many = catalog("", &"<frobnicate/>".repeat(xmlcraft_catalog::MAX_WARNINGS * 3), HERE);
    assert_eq!(many.warnings().len(), xmlcraft_catalog::MAX_WARNINGS);
    assert!(many.warnings().last().is_some_and(|last| last.contains("not listed")), "{:?}", many.warnings().last());
}

#[test]
fn attribute_values_are_decoded_and_white_space_is_normalised() {
    let c = catalog("", "<public publicId=\"a &amp; b\n\tc &#x41;&#66;\" uri=\"x&amp;y\"/>", "");
    assert_eq!(c.entries(), [Entry::Public { id: "a & b c AB".into(), uri: "x&y".into(), prefer: None }]);
    // With no location the references stay relative.
    let all = catalogs(&[c]);
    assert_eq!(all.resolve(Some("a & b c AB"), None, &mut NoLoader).as_deref(), Some("x&y"));
}

#[test]
fn a_catalog_with_very_many_entries_is_cut_and_says_so() {
    let body: String = (0..xmlcraft_catalog::MAX_ENTRIES + 50).map(|i| format!("<uri name=\"n{i}\" uri=\"u\"/>")).collect();
    let c = catalog("", &body, HERE);
    assert_eq!(c.entries().len(), xmlcraft_catalog::MAX_ENTRIES);
    assert_eq!(c.warnings().len(), 1, "said once: {:?}", c.warnings());
}

#[test]
fn groups_nested_very_deep_do_not_overflow_the_stack() {
    let depth = 5000;
    let body = format!("{}<system systemId=\"deep\" uri=\"x\"/>{}", "<group>".repeat(depth), "</group>".repeat(depth));
    let text = format!("<catalog xmlns=\"{CATALOG_NS}\">{body}</catalog>");
    // It is refused (the parser's own depth limit) or read with the inside left out; either way, no crash.
    match Catalog::parse(&text, HERE) {
        Ok(c) => assert!(c.entries().is_empty()),
        Err(e) => assert!(matches!(e, CatalogError::NotWellFormed(_)), "{e}"),
    }
}

proptest::proptest! {
    /// Whatever the entries and the request, resolution ends, never panics, and gives the same answer twice.
    #[test]
    fn resolution_is_total_and_deterministic(
        entries in proptest::collection::vec((0usize..11, "[ab/:.%]{0,4}", "[ab/:.]{0,4}"), 0..12),
        public in proptest::option::of("[ab -/]{0,5}"),
        system in proptest::option::of("[ab/:.%]{0,6}"),
        uri_request in "[ab/:.%]{0,6}",
    ) {
        let names = ["public", "system", "rewriteSystem", "systemSuffix", "delegatePublic", "delegateSystem", "uri", "rewriteURI", "uriSuffix", "delegateURI", "nextCatalog"];
        let attrs = [("publicId", "uri"), ("systemId", "uri"), ("systemIdStartString", "rewritePrefix"), ("systemIdSuffix", "uri"), ("publicIdStartString", "catalog"), ("systemIdStartString", "catalog"), ("name", "uri"), ("uriStartString", "rewritePrefix"), ("uriSuffix", "uri"), ("uriStartString", "catalog"), ("catalog", "catalog")];
        let body: String = entries.iter().map(|(kind, a, b)| {
            let (first, second) = attrs[*kind];
            if names[*kind] == "nextCatalog" { format!("<nextCatalog catalog=\"{a}\"/>") } else { format!("<{} {first}=\"{a}\" {second}=\"{b}\"/>", names[*kind]) }
        }).collect();
        let c = catalog("", &body, "file:///p/catalog.xml");
        let all = catalogs(&[c]);
        // Delegated and next catalogs are the catalog itself under other names: cycles and re-reads.
        let mut loader = Files::default().with("file:///p/a", "", &body).with("file:///p/b", "", &body);
        let first = (all.resolve(public.as_deref(), system.as_deref(), &mut loader), all.resolve_uri(&uri_request, &mut loader));
        let second = (all.resolve(public.as_deref(), system.as_deref(), &mut loader), all.resolve_uri(&uri_request, &mut loader));
        proptest::prop_assert_eq!(first, second);
    }
}
