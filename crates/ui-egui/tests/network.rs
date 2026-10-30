//! Schemas that are not files, and XML ▸ Work Offline, in the real shell. The network is a mock: nothing here goes to the web.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_resolve::{Conditions, FetchError, FetchLimits, FetchOutcome, Fetched, FixedClock, MemoryCache};
use xmlcraft_ui_egui::{Network, XmlCraftApp};

const SCHEMA: &str = "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"><xs:element name=\"n\" type=\"xs:int\"/></xs:schema>";
const DOCUMENT: &str =
    "<n xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"http://schemas.test/n.xsd\">seven</n>";

/// A web with one schema on it, and a count of the questions it was asked.
#[derive(Default)]
struct Web {
    asked: Mutex<Vec<String>>,
}

impl xmlcraft_resolve::Fetcher for Web {
    fn fetch(&self, url: &str, _: &Conditions, _: &FetchLimits) -> Result<FetchOutcome, FetchError> {
        self.asked.lock().expect("lock").push(url.to_owned());
        if url.ends_with("schemas.test/n.xsd") {
            Ok(FetchOutcome::Fresh(Fetched { body: SCHEMA.as_bytes().to_vec(), final_url: url.to_owned(), etag: None, last_modified: None }))
        } else {
            Err(FetchError::Status(404))
        }
    }
}

impl Web {
    fn questions(&self) -> usize {
        self.asked.lock().expect("lock").len()
    }
}

fn shell(web: &Arc<Web>) -> Harness<'static, XmlCraftApp> {
    let network = Network::new(Some(web.clone()), Some(Arc::new(MemoryCache::new())), Arc::new(FixedClock(1_000)));
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(move |cc| XmlCraftApp::with_network(cc, network.clone()));
    // Nothing is validated unless it is asked for (F8).
    h.state_mut().set_validate_delay(Duration::from_secs(3600));
    h.run_steps(4);
    h
}

fn wait(h: &mut Harness<'static, XmlCraftApp>, what: &str, done: impl Fn(&XmlCraftApp) -> bool) {
    let started = Instant::now();
    while !done(h.state()) {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for {what}");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
}

fn file(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path
}

fn open(h: &mut Harness<'static, XmlCraftApp>, path: &Path) {
    assert!(h.state_mut().open_path(path));
    wait(h, "the first analysis", |app| app.problems().is_some());
}

#[test]
fn a_schema_that_is_not_a_file_is_downloaded_and_the_document_validated_against_it() {
    let dir = tempfile::tempdir().expect("dir");
    let web = Arc::new(Web::default());
    let mut h = shell(&web);
    assert_eq!(web.questions(), 0, "nothing is downloaded until a document needs it");
    open(&mut h, &file(dir.path(), "n.xml", DOCUMENT));
    // Entry Helpers need the schema the document names, so it is asked for as soon as the document is shown.
    wait(&mut h, "the download for Entry Helpers", |_| web.questions() >= 1);
    let asked = web.questions();
    assert!(h.state_mut().execute("xml.validate"));
    wait(&mut h, "the validation", |app| app.validity().is_some());
    assert_eq!(h.state().validity(), Some((1, 0)), "`seven` is not an int, as the schema on the web says");
    assert!(h.query_all_by_label_contains("seven").next().is_some(), "the problem is listed");
    assert!(h.query_all_by_label_contains("offline").next().is_none());
    assert_eq!(web.questions(), asked, "the copy that was downloaded is kept, not asked for again");
}

#[test]
fn working_offline_downloads_nothing_and_going_online_again_does_with_no_further_ado() {
    let dir = tempfile::tempdir().expect("dir");
    let web = Arc::new(Web::default());
    let mut h = shell(&web);
    // A document that names a schema is validated when it has been left alone for a moment.
    h.state_mut().set_validate_delay(Duration::from_millis(150));
    assert_eq!(h.state().command_checked("xml.work_offline"), Some(false), "online unless the person says otherwise");
    assert!(h.state_mut().execute("xml.work_offline"));
    assert_eq!(h.state().command_checked("xml.work_offline"), Some(true));
    open(&mut h, &file(dir.path(), "n.xml", DOCUMENT));
    wait(&mut h, "the validation offline", |app| app.validations_requested() == 1 && app.validity().is_some());
    assert_eq!(web.questions(), 0, "offline asks nobody");
    assert!(h.query_all_by_label_contains("offline").next().is_some(), "the problem says why the schema was not read");
    assert!(h.query_all_by_label_contains("seven").next().is_none(), "the document was not checked against a schema that was not read");
    let helpers_asked = h.state().schema_requests();
    // Online again: the schema is asked for again, for Entry Helpers and for the validation, and nobody has to do a thing.
    assert!(h.state_mut().execute("xml.work_offline"));
    assert_eq!(h.state().command_checked("xml.work_offline"), Some(false));
    wait(&mut h, "the download", |_| web.questions() >= 1);
    wait(&mut h, "the validation online", |app| app.validations_requested() == 2 && app.validity() == Some((1, 0)));
    assert!(h.state().schema_requests() > helpers_asked, "Entry Helpers asked again");
    assert!(h.query_all_by_label_contains("seven").next().is_some(), "the problem is listed");
    assert!(h.query_all_by_label_contains("offline").next().is_none());
}

#[test]
fn opening_a_document_asks_nobody_about_the_namespace_it_is_in() {
    let dir = tempfile::tempdir().expect("dir");
    let web = Arc::new(Web::default());
    let mut h = shell(&web);
    // No schema named; the root element is in a namespace that is an address (as namespaces often are).
    open(&mut h, &file(dir.path(), "ns.xml", "<n xmlns=\"http://schemas.test/n.xsd\">1</n>"));
    wait(&mut h, "Entry Helpers to ask for the schema known for the namespace", |app| app.schema_requests() >= 1);
    h.run_steps(10);
    assert_eq!(web.questions(), 0, "a namespace is a name, not an address to read: {:?}", web.asked.lock().expect("lock"));
}
