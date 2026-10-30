//! The resolution chain with a world made of maps: files, open documents, a server that answers what it is told.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use xmlcraft_resolve::{
    Budget, Buffers, CacheEntry, Clock, Conditions, Environment, FetchError, FetchLimits, FetchOutcome, Fetched, Fetcher, FileAccess,
    MAX_DOWNLOAD_BYTES, MemoryCache, NoBuffers, NoFiles, Origin, Outcome, Policy, Request, Resolver, ResourceCache, Stage,
};

#[derive(Default)]
struct Files(Mutex<HashMap<String, Vec<u8>>>);

impl Files {
    fn with(files: &[(&str, &str)]) -> Arc<Self> {
        Arc::new(Self(Mutex::new(files.iter().map(|(p, t)| ((*p).to_owned(), t.as_bytes().to_vec())).collect())))
    }
}

impl FileAccess for Files {
    fn read(&self, path: &str) -> Result<Vec<u8>, String> {
        self.0.lock().expect("lock").get(path).cloned().ok_or_else(|| format!("{path}: no such file"))
    }

    /// These files are on a system that folds case.
    fn identity(&self, path: &str) -> String {
        path.to_lowercase()
    }
}

struct Open(HashMap<String, Arc<str>>);

impl Buffers for Open {
    fn text(&self, location: &str) -> Option<Arc<str>> {
        self.0.get(location).cloned()
    }
}

/// A server: what each URL answers, and what was asked.
#[derive(Default)]
struct Net {
    answers: Mutex<HashMap<String, Result<FetchOutcome, FetchError>>>,
    calls: Mutex<Vec<(String, Conditions)>>,
}

impl Net {
    fn answer(&self, url: &str, answer: Result<FetchOutcome, FetchError>) {
        self.answers.lock().expect("lock").insert(url.to_owned(), answer);
    }

    fn serve(&self, url: &str, body: &str) {
        self.answer(url, Ok(FetchOutcome::Fresh(fetched(url, body))));
    }

    fn urls(&self) -> Vec<String> {
        self.calls.lock().expect("lock").iter().map(|(u, _)| u.clone()).collect()
    }
}

fn fetched(url: &str, body: &str) -> Fetched {
    Fetched { body: body.as_bytes().to_vec(), final_url: url.to_owned(), etag: Some("\"v1\"".into()), last_modified: None }
}

impl Fetcher for Net {
    fn fetch(&self, url: &str, conditions: &Conditions, _: &FetchLimits) -> Result<FetchOutcome, FetchError> {
        self.calls.lock().expect("lock").push((url.to_owned(), conditions.clone()));
        self.answers.lock().expect("lock").get(url).cloned().unwrap_or(Err(FetchError::Status(404)))
    }
}

#[derive(Default)]
struct Time(AtomicI64);

impl Clock for Time {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

const DAY: i64 = 24 * 60 * 60;

struct World {
    files: Arc<Files>,
    net: Arc<Net>,
    cache: Arc<MemoryCache>,
    time: Arc<Time>,
    open: HashMap<String, Arc<str>>,
    policy: Policy,
    catalogs: Vec<String>,
    fetcher: bool,
}

impl World {
    fn new() -> Self {
        Self {
            files: Files::with(&[]),
            net: Arc::new(Net::default()),
            cache: Arc::new(MemoryCache::new()),
            time: Arc::new(Time(AtomicI64::new(1_000_000))),
            open: HashMap::new(),
            policy: Policy::default(),
            catalogs: Vec::new(),
            fetcher: true,
        }
    }

    fn resolver(&self) -> Resolver {
        let buffers: Arc<dyn Buffers> = if self.open.is_empty() { Arc::new(NoBuffers) } else { Arc::new(Open(self.open.clone())) };
        let env = Environment {
            buffers,
            files: self.files.clone(),
            fetcher: self.fetcher.then(|| self.net.clone() as Arc<dyn Fetcher>),
            cache: Some(self.cache.clone()),
            clock: self.time.clone(),
        };
        Resolver::new(env, self.policy.clone(), &self.catalogs)
    }

    fn advance(&self, seconds: i64) {
        self.time.0.fetch_add(seconds, Ordering::SeqCst);
    }
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("utf-8")
}

const XHTML_PUBLIC: &str = "-//W3C//DTD XHTML 1.0 Strict//EN";
const XHTML_SYSTEM: &str = "http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd";
const XHTML_BUNDLED: &str = "bundled:/w3c/xhtml1/xhtml1-strict.dtd";

#[test]
fn a_relative_system_identifier_is_a_file_next_to_the_document() {
    let mut w = World::new();
    w.files = Files::with(&[("/docs/a.dtd", "<!ELEMENT a ANY>")]);
    let found = w.resolver().resolve(&Request::external_id(None, Some("a.dtd"), "/docs/a.xml"), &mut Budget::new()).expect("found");
    assert_eq!((text(&found.bytes), found.origin, found.location.as_str()), ("<!ELEMENT a ANY>", Origin::File, "/docs/a.dtd"));
    assert!(found.trail.of(Stage::BundledCatalog).all(|s| s.outcome == Outcome::Missed), "{}", found.trail);
}

#[test]
fn a_file_uri_and_an_absolute_path_are_files_too() {
    let mut w = World::new();
    w.files = Files::with(&[("/x/b c.dtd", "<!ELEMENT b ANY>"), ("/y/d.dtd", "<!ELEMENT d ANY>")]);
    let r = w.resolver();
    let by_uri = r.resolve(&Request::external_id(None, Some("file:///x/b%20c.dtd"), ""), &mut Budget::new()).expect("found");
    assert_eq!((text(&by_uri.bytes), by_uri.origin), ("<!ELEMENT b ANY>", Origin::File));
    let absolute = r.resolve(&Request::external_id(None, Some("/y/d.dtd"), "/docs/a.xml"), &mut Budget::new()).expect("found");
    assert_eq!(absolute.location, "/y/d.dtd");
}

#[test]
fn an_open_document_comes_before_the_file() {
    let mut w = World::new();
    w.files = Files::with(&[("/docs/a.dtd", "on disk")]);
    w.open.insert("/docs/a.dtd".into(), Arc::from("being edited"));
    let found = w.resolver().resolve(&Request::external_id(None, Some("a.dtd"), "/docs/a.xml"), &mut Budget::new()).expect("found");
    assert_eq!((text(&found.bytes), found.origin), ("being edited", Origin::Buffer));
}

#[test]
fn an_open_document_comes_before_the_catalogs_too() {
    let mut w = World::new();
    w.open.insert(XHTML_SYSTEM.replace("http://www.w3.org/TR/xhtml1/DTD/", "/docs/"), Arc::from("my own strict.dtd"));
    let found = w
        .resolver()
        .resolve(&Request::external_id(Some(XHTML_PUBLIC), Some("xhtml1-strict.dtd"), "/docs/a.xhtml"), &mut Budget::new())
        .expect("found");
    assert_eq!((text(&found.bytes), found.origin), ("my own strict.dtd", Origin::Buffer));
}

#[test]
fn an_identifier_the_bundle_knows_is_read_from_the_bundle_and_nothing_is_downloaded() {
    let w = World::new();
    let found =
        w.resolver().resolve(&Request::external_id(Some(XHTML_PUBLIC), Some(XHTML_SYSTEM), "/docs/a.xhtml"), &mut Budget::new()).expect("found");
    assert_eq!((found.origin, found.location.as_str()), (Origin::Bundle, XHTML_BUNDLED));
    assert_eq!(Some(found.bytes.clone()), xmlcraft_schemas::get("w3c/xhtml1/xhtml1-strict.dtd"));
    assert!(w.net.urls().is_empty());
    let step = found.trail.of(Stage::BundledCatalog).next().expect("the catalog was asked");
    assert!(step.outcome == Outcome::Found && step.what.contains(XHTML_BUNDLED), "{}", found.trail);
}

#[test]
fn a_system_identifier_alone_is_mapped_by_a_rewrite_and_https_works_like_http() {
    let w = World::new();
    let r = w.resolver();
    for system in ["http://www.w3.org/TR/xhtml1/DTD/xhtml-lat1.ent", "https://www.w3.org/TR/xhtml1/DTD/xhtml-lat1.ent"] {
        let found = r.resolve(&Request::external_id(None, Some(system), ""), &mut Budget::new()).expect("found");
        assert_eq!((found.origin, found.location.as_str()), (Origin::Bundle, "bundled:/w3c/xhtml1/xhtml-lat1.ent"), "{system}");
    }
    assert!(w.net.urls().is_empty());
}

#[test]
fn a_namespace_or_a_schema_location_is_a_uri_entry() {
    let w = World::new();
    let r = w.resolver();
    for (uri, bundled) in [
        ("http://www.w3.org/2001/XMLSchema", "bundled:/w3c/xmlschema/XMLSchema.xsd"),
        ("http://www.w3.org/XML/1998/namespace", "bundled:/w3c/xml/xml.xsd"),
        ("http://www.w3.org/2001/xml.xsd", "bundled:/w3c/xml/xml.xsd"),
    ] {
        let found = r.resolve(&Request::uri(uri, "/docs/s.xsd"), &mut Budget::new()).expect("found");
        assert_eq!((found.origin, found.location.as_str()), (Origin::Bundle, bundled), "{uri}");
    }
    assert!(w.net.urls().is_empty());
}

#[test]
fn the_dtds_and_schemas_that_ship_with_the_app_are_found_by_their_identifiers_namespaces_and_addresses() {
    let w = World::new();
    let r = w.resolver();
    let dtd = |public: Option<&str>, system: Option<&str>| {
        let request = Request::external_id(public, system, "/docs/a.xml");
        r.resolve(&request, &mut Budget::new()).map(|found| (found.origin, found.location))
    };
    let uri = |name: &str| r.resolve(&Request::uri(name, "/docs/a.xsd"), &mut Budget::new()).map(|found| (found.origin, found.location));
    let bundled = |path: &str| Ok((Origin::Bundle, format!("bundled:/{path}")));
    // XHTML 1.1 and SVG 1.1: by the public identifier of the DTD and of its modules, and by the address of any of them.
    assert_eq!(
        dtd(Some("-//W3C//DTD XHTML 1.1//EN"), Some("http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd")).map_err(|f| f.message),
        bundled("w3c/xhtml11/xhtml11.dtd")
    );
    assert_eq!(
        dtd(Some("-//W3C//ELEMENTS XHTML Text 1.0//EN"), Some("http://www.w3.org/MarkUp/DTD/xhtml-text-1.mod")).map_err(|f| f.message),
        bundled("w3c/xhtml11/xhtml-text-1.mod")
    );
    assert_eq!(dtd(None, Some("https://www.w3.org/MarkUp/DTD/xhtml-table-1.mod")).map_err(|f| f.message), bundled("w3c/xhtml11/xhtml-table-1.mod"));
    assert_eq!(
        dtd(Some("-//W3C//DTD SVG 1.1//EN"), Some("http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd")).map_err(|f| f.message),
        bundled("w3c/svg11/svg11.dtd")
    );
    assert_eq!(dtd(None, Some("http://www.w3.org/Graphics/SVG/1.1/DTD/svg-filter.mod")).map_err(|f| f.message), bundled("w3c/svg11/svg-filter.mod"));
    // The schemas: by namespace, and by the address a document gives.
    for (name, path) in [
        ("http://www.w3.org/1998/Math/MathML", "w3c/mathml3/mathml3.xsd"),
        ("http://www.w3.org/Math/XMLSchema/mathml3/mathml3-common.xsd", "w3c/mathml3/mathml3-common.xsd"),
        ("http://www.w3.org/1999/xlink", "w3c/xlink/xlink.xsd"),
        ("https://www.w3.org/1999/xlink.xsd", "w3c/xlink/xlink.xsd"),
        ("http://www.w3.org/2000/09/xmldsig#", "w3c/xmldsig/xmldsig-core-schema.xsd"),
        ("http://www.w3.org/2003/05/soap-envelope", "w3c/soap12/soap-envelope.xsd"),
        ("http://www.w3.org/ns/wsdl", "w3c/wsdl20/wsdl20.xsd"),
        ("http://www.w3.org/2007/06/wsdl/wsdl20.xsd", "w3c/wsdl20/wsdl20.xsd"),
    ] {
        assert_eq!(uri(name).map_err(|f| f.message), bundled(path), "{name}");
    }
    assert!(w.net.urls().is_empty(), "nothing was downloaded");
}

#[test]
fn a_bundled_file_finds_its_neighbours_by_their_relative_names() {
    let w = World::new();
    let r = w.resolver();
    let base = "bundled:/w3c/xmlschema/XMLSchema.xsd";
    // The DOCTYPE of the schema for schemas: its public identifier is in the catalog, and the relative name is a file of the bundle.
    for request in [
        Request::external_id(Some("-//W3C//DTD XMLSCHEMA 200102//EN"), Some("XMLSchema.dtd"), base),
        Request::external_id(None, Some("XMLSchema.dtd"), base),
        Request::external_id(Some("datatypes"), Some("datatypes.dtd"), "bundled:/w3c/xmlschema/XMLSchema.dtd"),
        Request::external_id(None, Some("datatypes.dtd"), "bundled:/w3c/xmlschema/XMLSchema.dtd"),
    ] {
        let found = r.resolve(&request, &mut Budget::new()).expect("found");
        assert_eq!(found.origin, Origin::Bundle, "{request:?}");
    }
}

#[test]
fn the_users_catalogs_are_asked_before_the_bundled_ones() {
    let mut w = World::new();
    w.files = Files::with(&[
        (
            "/cat/catalog.xml",
            &format!(
                "<catalog xmlns=\"urn:oasis:names:tc:entity:xmlns:xml:catalog\"><public publicId=\"{XHTML_PUBLIC}\" uri=\"file:///my/strict.dtd\"/></catalog>"
            ),
        ),
        ("/my/strict.dtd", "mine"),
    ]);
    w.catalogs = vec!["/cat/catalog.xml".into()];
    let r = w.resolver();
    assert!(r.warnings().is_empty(), "{:?}", r.warnings());
    let found = r.resolve(&Request::external_id(Some(XHTML_PUBLIC), Some(XHTML_SYSTEM), "/docs/a.xhtml"), &mut Budget::new()).expect("found");
    assert_eq!((text(&found.bytes), found.origin), ("mine", Origin::File));
    assert!(found.trail.of(Stage::BundledCatalog).next().is_none(), "the bundled catalog was not needed: {}", found.trail);
}

#[test]
fn a_catalog_entry_that_leads_nowhere_does_not_end_the_search() {
    let mut w = World::new();
    w.files = Files::with(&[(
        "/cat/catalog.xml",
        &format!(
            "<catalog xmlns=\"urn:oasis:names:tc:entity:xmlns:xml:catalog\"><public publicId=\"{XHTML_PUBLIC}\" uri=\"file:///gone.dtd\"/></catalog>"
        ),
    )]);
    w.catalogs = vec!["/cat/catalog.xml".into()];
    let found = w.resolver().resolve(&Request::external_id(Some(XHTML_PUBLIC), Some(XHTML_SYSTEM), ""), &mut Budget::new()).expect("found");
    assert_eq!(found.origin, Origin::Bundle);
    let failed = found.trail.of(Stage::File).any(|s| matches!(s.outcome, Outcome::Failed(_)));
    assert!(failed, "the failure is in the trail: {}", found.trail);
}

#[test]
fn a_user_catalog_that_cannot_be_used_is_noted_and_skipped() {
    let mut w = World::new();
    w.files = Files::with(&[("/cat/bad.xml", "<not-a-catalog/>"), ("/cat/text.xml", "<catalog")]);
    w.catalogs = vec!["/cat/bad.xml".into(), "/cat/text.xml".into(), "/cat/missing.xml".into(), "http://example.org/c.xml".into()];
    let r = w.resolver();
    assert_eq!(r.warnings().len(), 4, "{:?}", r.warnings());
    assert!(r.warnings().iter().all(|w| w.contains("not used")), "{:?}", r.warnings());
    // The bundled catalog still works.
    assert!(r.resolve(&Request::external_id(Some(XHTML_PUBLIC), None, ""), &mut Budget::new()).is_ok());
}

#[test]
fn the_bundled_catalogs_can_be_turned_off() {
    let mut w = World::new();
    w.policy.use_bundled = false;
    w.net.serve("https://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd", "from the web");
    let found = w.resolver().resolve(&Request::external_id(Some(XHTML_PUBLIC), Some(XHTML_SYSTEM), ""), &mut Budget::new()).expect("found");
    assert_eq!((text(&found.bytes), found.origin), ("from the web", Origin::Network));
}

#[test]
fn a_download_is_kept_and_served_from_the_cache_while_it_is_current() {
    let w = World::new();
    w.net.serve("https://example.org/s/a.xsd", "<schema/>");
    let r = w.resolver();
    let request = Request::uri("http://example.org/s/a.xsd", "");
    let first = r.resolve(&request, &mut Budget::new()).expect("found");
    assert_eq!((first.origin, first.location.as_str()), (Origin::Network, "https://example.org/s/a.xsd"));
    assert_eq!(w.net.urls(), ["https://example.org/s/a.xsd"], "https is tried first");
    w.advance(29 * DAY);
    let second = r.resolve(&request, &mut Budget::new()).expect("found");
    assert_eq!((second.origin, text(&second.bytes)), (Origin::Cache { stale: false }, "<schema/>"));
    assert_eq!(w.net.urls().len(), 1, "no second download");
    assert!(w.cache.get("http://example.org/s/a.xsd").is_some(), "kept under the URL that was asked for");
}

#[test]
fn http_is_tried_when_https_does_not_answer() {
    let w = World::new();
    w.net.answer("https://example.org/a.xsd", Err(FetchError::Network("connection refused".into())));
    w.net.serve("http://example.org/a.xsd", "plain");
    let found = w.resolver().resolve(&Request::uri("http://example.org/a.xsd", ""), &mut Budget::new()).expect("found");
    assert_eq!((text(&found.bytes), found.location.as_str()), ("plain", "http://example.org/a.xsd"));
    assert_eq!(w.net.urls(), ["https://example.org/a.xsd", "http://example.org/a.xsd"]);
    assert!(found.trail.of(Stage::Network).any(|s| matches!(&s.outcome, Outcome::Failed(why) if why.contains("refused"))), "{}", found.trail);
}

#[test]
fn what_a_redirect_led_to_is_where_the_text_is() {
    let w = World::new();
    w.net.answer(
        "https://example.org/old.xsd",
        Ok(FetchOutcome::Fresh(Fetched {
            body: b"moved".to_vec(),
            final_url: "https://example.org/new/s.xsd".into(),
            etag: None,
            last_modified: None,
        })),
    );
    let found = w.resolver().resolve(&Request::uri("http://example.org/old.xsd", ""), &mut Budget::new()).expect("found");
    assert_eq!(found.location, "https://example.org/new/s.xsd");
    // From the cache it is the same place.
    let again = w.resolver().resolve(&Request::uri("http://example.org/old.xsd", ""), &mut Budget::new()).expect("found");
    assert_eq!((again.location.as_str(), again.origin), ("https://example.org/new/s.xsd", Origin::Cache { stale: false }));
}

#[test]
fn a_copy_past_its_time_is_asked_about_and_kept_when_it_is_still_current() {
    let w = World::new();
    w.net.serve("https://example.org/a.xsd", "v1");
    let r = w.resolver();
    let request = Request::uri("https://example.org/a.xsd", "");
    r.resolve(&request, &mut Budget::new()).expect("found");
    w.advance(31 * DAY);
    w.net.answer("https://example.org/a.xsd", Ok(FetchOutcome::NotModified));
    let found = r.resolve(&request, &mut Budget::new()).expect("found");
    assert_eq!((found.origin, text(&found.bytes)), (Origin::Cache { stale: false }, "v1"));
    let (_, conditions) = w.net.calls.lock().expect("lock").last().cloned().expect("asked");
    assert_eq!(conditions.etag.as_deref(), Some("\"v1\""), "the server is asked with what the copy is");
    // The copy is current again for another 30 days.
    w.advance(10 * DAY);
    let calls = w.net.urls().len();
    assert_eq!(r.resolve(&request, &mut Budget::new()).expect("found").origin, Origin::Cache { stale: false });
    assert_eq!(w.net.urls().len(), calls);
}

#[test]
fn a_changed_resource_replaces_the_copy() {
    let w = World::new();
    w.net.serve("https://example.org/a.xsd", "v1");
    let r = w.resolver();
    let request = Request::uri("https://example.org/a.xsd", "");
    r.resolve(&request, &mut Budget::new()).expect("found");
    w.advance(31 * DAY);
    w.net.serve("https://example.org/a.xsd", "v2");
    let found = r.resolve(&request, &mut Budget::new()).expect("found");
    assert_eq!((found.origin, text(&found.bytes)), (Origin::Network, "v2"));
    assert_eq!(w.cache.get("https://example.org/a.xsd").map(|e| e.body), Some(b"v2".to_vec()));
}

#[test]
fn when_the_server_cannot_be_asked_the_old_copy_is_served_as_stale() {
    let w = World::new();
    w.net.serve("https://example.org/a.xsd", "v1");
    let r = w.resolver();
    let request = Request::uri("https://example.org/a.xsd", "");
    r.resolve(&request, &mut Budget::new()).expect("found");
    w.advance(31 * DAY);
    w.net.answer("https://example.org/a.xsd", Err(FetchError::Network("timed out".into())));
    let found = r.resolve(&request, &mut Budget::new()).expect("found");
    assert_eq!((found.origin, text(&found.bytes)), (Origin::Cache { stale: true }, "v1"));
    assert!(found.trail.of(Stage::Network).any(|s| matches!(&s.outcome, Outcome::Failed(why) if why.contains("timed out"))), "{}", found.trail);
}

#[test]
fn offline_nothing_is_downloaded_and_an_old_copy_is_still_served() {
    let mut w = World::new();
    w.cache.put(CacheEntry {
        url: "https://example.org/a.xsd".into(),
        final_url: "https://example.org/a.xsd".into(),
        body: b"old".to_vec(),
        etag: None,
        last_modified: None,
        fetched_at: -40 * DAY,
    });
    w.policy.offline = true;
    let r = w.resolver();
    let found = r.resolve(&Request::uri("https://example.org/a.xsd", ""), &mut Budget::new()).expect("found");
    assert_eq!((found.origin, text(&found.bytes)), (Origin::Cache { stale: true }, "old"));
    assert!(found.trail.of(Stage::Network).any(|s| s.outcome == Outcome::Skipped("offline".into())), "{}", found.trail);
    let missing = r.resolve(&Request::uri("https://example.org/b.xsd", ""), &mut Budget::new()).expect_err("not there");
    assert!(missing.message.contains("offline"), "{missing}");
    assert!(w.net.urls().is_empty());
}

#[test]
fn with_downloading_off_the_cache_is_all_there_is() {
    let mut w = World::new();
    w.policy.auto_download = false;
    let err = w.resolver().resolve(&Request::uri("https://example.org/a.xsd", ""), &mut Budget::new()).expect_err("not downloaded");
    assert!(err.message.contains("downloading is off"), "{err}");
    assert!(err.trail.of(Stage::Network).any(|s| matches!(s.outcome, Outcome::Skipped(_))));
    assert!(w.net.urls().is_empty());
    // A build that cannot download says so as well.
    let mut w = World::new();
    w.fetcher = false;
    let err = w.resolver().resolve(&Request::uri("https://example.org/a.xsd", ""), &mut Budget::new()).expect_err("not downloaded");
    assert!(err.message.contains("cannot download"), "{err}");
}

#[test]
fn a_download_over_the_limit_is_refused() {
    let w = World::new();
    w.net.answer(
        "https://example.org/big.xsd",
        Ok(FetchOutcome::Fresh(Fetched {
            body: vec![b' '; MAX_DOWNLOAD_BYTES + 1],
            final_url: "https://example.org/big.xsd".into(),
            etag: None,
            last_modified: None,
        })),
    );
    let err = w.resolver().resolve(&Request::uri("https://example.org/big.xsd", ""), &mut Budget::new()).expect_err("too large");
    assert!(err.message.contains("larger than"), "{err}");
    assert!(w.cache.is_empty(), "nothing was kept");
}

#[test]
fn a_server_that_says_not_modified_without_a_copy_is_an_error() {
    let w = World::new();
    w.net.answer("https://example.org/a.xsd", Ok(FetchOutcome::NotModified));
    let err = w.resolver().resolve(&Request::uri("https://example.org/a.xsd", ""), &mut Budget::new()).expect_err("no copy");
    assert!(err.message.contains("there is none"), "{err}");
}

#[test]
fn a_copy_from_the_future_is_not_current() {
    let w = World::new();
    w.cache.put(CacheEntry {
        url: "https://example.org/a.xsd".into(),
        final_url: "https://example.org/a.xsd".into(),
        body: b"old".to_vec(),
        etag: None,
        last_modified: None,
        fetched_at: i64::MAX,
    });
    w.net.serve("https://example.org/a.xsd", "new");
    let found = w.resolver().resolve(&Request::uri("https://example.org/a.xsd", ""), &mut Budget::new()).expect("found");
    assert_eq!((found.origin, text(&found.bytes)), (Origin::Network, "new"));
}

#[test]
fn schemes_other_than_files_http_and_https_are_not_read() {
    let w = World::new();
    for system in ["ftp://example.org/a.dtd", "jar:file:///x.jar!/a.dtd", "gopher://x/a"] {
        let err = w.resolver().resolve(&Request::external_id(None, Some(system), ""), &mut Budget::new()).expect_err("not read");
        assert!(err.message.contains("is not read"), "{system}: {err}");
    }
}

#[test]
fn a_reference_with_nothing_to_go_by_fails_and_says_where_it_looked() {
    let w = World::new();
    let err = w.resolver().resolve(&Request::external_id(Some("-//NOBODY//DTD Nothing//EN"), None, ""), &mut Budget::new()).expect_err("unknown");
    assert!(err.message.contains("-//NOBODY//DTD Nothing//EN"), "{err}");
    assert!(err.trail.of(Stage::BundledCatalog).any(|s| s.outcome == Outcome::Missed), "{}", err.trail);
    let empty = w.resolver().resolve(&Request::external_id(None, Some(""), ""), &mut Budget::new()).expect_err("nothing");
    assert!(empty.message.contains("nothing names"), "{empty}");
}

#[test]
fn one_load_may_read_only_so_much() {
    let mut w = World::new();
    w.files = Files::with(&[("/a.dtd", "12345"), ("/b.dtd", "12345"), ("/c.dtd", "12345")]);
    let r = w.resolver();
    let mut few = Budget::with_limits(2, 1000);
    assert!(r.resolve(&Request::external_id(None, Some("/a.dtd"), ""), &mut few).is_ok());
    assert!(r.resolve(&Request::external_id(None, Some("/b.dtd"), ""), &mut few).is_ok());
    let err = r.resolve(&Request::external_id(None, Some("/c.dtd"), ""), &mut few).expect_err("over");
    assert!(err.message.contains("which is all it may read"), "{err}");
    assert_eq!((few.resources(), few.bytes()), (2, 10));
    let mut small = Budget::with_limits(10, 8);
    assert!(r.resolve(&Request::external_id(None, Some("/a.dtd"), ""), &mut small).is_ok());
    let err = r.resolve(&Request::external_id(None, Some("/b.dtd"), ""), &mut small).expect_err("over");
    assert!(err.message.contains("bytes it may read in all"), "{err}");
    assert_eq!((small.resources(), small.bytes()), (1, 5), "what did not fit was not spent");
}

#[test]
fn the_resolver_can_be_shared_between_threads() {
    fn shared<T: Send + Sync>() {}
    shared::<Resolver>();
}

#[test]
fn a_copy_is_current_up_to_the_last_second_of_its_time() {
    let w = World::new();
    w.net.serve("https://example.org/a.xsd", "v1");
    let r = w.resolver();
    let request = Request::uri("https://example.org/a.xsd", "");
    r.resolve(&request, &mut Budget::new()).expect("found");
    w.advance(30 * DAY - 1);
    assert_eq!(r.resolve(&request, &mut Budget::new()).expect("found").origin, Origin::Cache { stale: false });
    assert_eq!(w.net.urls().len(), 1, "one second short of 30 days: not asked");
    w.advance(1);
    w.net.answer("https://example.org/a.xsd", Ok(FetchOutcome::NotModified));
    assert_eq!(r.resolve(&request, &mut Budget::new()).expect("found").origin, Origin::Cache { stale: false });
    assert_eq!(w.net.urls().len(), 2, "30 days: asked");
}

#[test]
fn a_document_has_one_identity_whatever_it_is_called() {
    let r = World::new().resolver();
    // A file is what the files say it is: here, the case does not matter.
    assert_eq!(r.identity("/S/Types.xsd"), r.identity("/s/types.xsd"));
    // A `file:` URI is the file it names.
    assert_eq!(r.identity("file:///S/Types.xsd"), r.identity("/s/types.xsd"));
    assert_ne!(r.identity("/s/a.xsd"), r.identity("/s/b.xsd"));
    // Anything else is its own spelling: the case of a URL counts.
    assert_ne!(r.identity("https://example.org/A.xsd"), r.identity("https://example.org/a.xsd"));
    assert_eq!(r.identity("https://example.org/a.xsd"), "https://example.org/a.xsd");
    // Files that cannot tell better answer with the spelling made plain.
    assert_eq!(NoFiles.identity("a\\b/C"), xmlcraft_resolve::normalized("a\\b/C"));
}
