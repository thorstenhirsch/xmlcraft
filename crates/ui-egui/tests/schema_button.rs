//! The command bar offers Schema… for an XML document (Generate Schema, Load Schema) and XML… for a schema or a DTD (Generate Sample).
//! The network is a mock: nothing here goes to the web.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use xmlcraft_resolve::{Conditions, FetchError, FetchLimits, FetchOutcome, Fetched, FixedClock, MemoryCache};
use xmlcraft_ui_egui::{Network, XmlCraftApp};

const SCHEMA: &str = "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"><xs:element name=\"n\" type=\"xs:int\"/></xs:schema>";
const XSI: &str = "xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"";

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

fn shell(web: &Arc<Web>) -> Harness<'static, XmlCraftApp> {
    let network = Network::new(Some(web.clone()), Some(Arc::new(MemoryCache::new())), Arc::new(FixedClock(1_000)));
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(move |cc| XmlCraftApp::with_network(cc, network.clone()));
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

fn tab_names(h: &Harness<'static, XmlCraftApp>) -> Vec<String> {
    h.state().tabs().iter().map(|t| t.name.clone()).collect()
}

/// Open the Schema… menu.
fn open_menu(h: &mut Harness<'static, XmlCraftApp>) {
    h.get_by_label("Schema…").click();
    h.run_steps(3);
}

#[test]
fn an_xml_document_has_schema_and_no_generate_or_xml_button() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell(&Arc::new(Web::default()));
    open(&mut h, &file(dir.path(), "a.xml", "<a/>"));
    assert!(h.query_by_label("Schema…").is_some());
    assert!(h.query_by_label("Generate…").is_none(), "the old button is gone");
    assert!(h.query_by_label("XML…").is_none(), "XML… is for schemas");
}

#[test]
fn a_schema_and_a_dtd_have_xml_that_generates_a_sample() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell(&Arc::new(Web::default()));
    for (name, text) in [("s.xsd", SCHEMA), ("d.dtd", "<!ELEMENT r EMPTY>")] {
        open(&mut h, &file(dir.path(), name, text));
        assert!(h.query_by_label("XML…").is_some(), "{name}");
        assert!(h.query_by_label("Schema…").is_none(), "{name}");
        h.get_by_label("XML…").click();
        h.run_steps(4);
        assert!(h.query_by_label("Generate Sample XML").is_some(), "{name}: the Generate Sample dialog is up");
        h.state_mut().execute("file.close"); // closes the tab (or asks) — the dialog goes first
        h.key_press(egui::Key::Escape);
        h.run_steps(3);
    }
}

#[test]
fn a_schema_text_without_an_extension_is_still_a_schema() {
    let mut h = shell(&Arc::new(Web::default()));
    h.state_mut().session.open(None, SCHEMA.as_bytes(), None).expect("opens");
    wait(&mut h, "the analysis", |app| app.problems().is_some());
    assert!(h.query_by_label("XML…").is_some());
}

#[test]
fn load_schema_is_greyed_out_without_a_link_and_there_is_no_action() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell(&Arc::new(Web::default()));
    open(&mut h, &file(dir.path(), "a.xml", "<a/>"));
    open_menu(&mut h);
    assert!(h.query_by_label("Generate Schema…").is_some());
    assert!(h.get_by_label("Load Schema").accesskit_node().is_disabled());
    assert!(!h.state_mut().execute("xml.load_schema"), "the command refuses too");
    assert_eq!(tab_names(&h), ["a.xml"]);
}

#[test]
fn load_schema_opens_a_linked_file_in_a_new_tab_and_a_second_time_reuses_it() {
    let dir = tempfile::tempdir().expect("dir");
    file(dir.path(), "n.xsd", SCHEMA);
    let mut h = shell(&Arc::new(Web::default()));
    open(&mut h, &file(dir.path(), "n.xml", &format!("<n {XSI} xsi:noNamespaceSchemaLocation=\"n.xsd\">1</n>")));
    open_menu(&mut h);
    assert!(!h.get_by_label("Load Schema").accesskit_node().is_disabled());
    h.get_by_label("Load Schema").click();
    wait(&mut h, "the schema tab", |app| app.tabs().len() == 2);
    assert_eq!(tab_names(&h), ["n.xml", "n.xsd"]);
    assert!(h.query_by_label("XML…").is_some(), "the schema is in front, and it offers XML…");
    // Back in the XML document, a second Load Schema goes to the tab that is open.
    h.state_mut().session.set_active(Some(0));
    h.run_steps(3);
    assert!(h.state_mut().execute("xml.load_schema"));
    wait(&mut h, "the same tab", |app| app.active() == Some(1));
    assert_eq!(h.state().tabs().len(), 2, "no second copy");
}

#[test]
fn load_schema_downloads_a_remote_schema_into_an_unsaved_tab() {
    let dir = tempfile::tempdir().expect("dir");
    let web = Arc::new(Web::default());
    let mut h = shell(&web);
    open(&mut h, &file(dir.path(), "n.xml", &format!("<n {XSI} xsi:noNamespaceSchemaLocation=\"http://schemas.test/n.xsd\">1</n>")));
    assert!(h.state_mut().execute("xml.load_schema"));
    wait(&mut h, "the schema tab", |app| app.tabs().len() == 2);
    assert_eq!(tab_names(&h), ["n.xml", "n.xsd"]);
    let text = h.state().session.active_document().expect("shown").editor().text();
    assert_eq!(text, SCHEMA);
    assert_eq!(web.asked.lock().expect("lock").len(), 1);
}

#[test]
fn load_schema_says_why_when_it_cannot() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell(&Arc::new(Web::default()));
    open(&mut h, &file(dir.path(), "n.xml", &format!("<n {XSI} xsi:noNamespaceSchemaLocation=\"missing.xsd\">1</n>")));
    assert!(h.state_mut().execute("xml.load_schema"));
    let started = Instant::now();
    while h.query_by_label("Couldn't load the schema").is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the message");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(tab_names(&h), ["n.xml"], "nothing was opened");
}

#[test]
fn a_dtd_named_by_the_doctype_is_loaded_too() {
    let dir = tempfile::tempdir().expect("dir");
    file(dir.path(), "r.dtd", "<!ELEMENT r EMPTY>");
    let mut h = shell(&Arc::new(Web::default()));
    open(&mut h, &file(dir.path(), "r.xml", "<!DOCTYPE r SYSTEM \"r.dtd\"><r/>"));
    assert!(h.state_mut().execute("xml.load_schema"));
    wait(&mut h, "the DTD tab", |app| app.tabs().len() == 2);
    assert_eq!(tab_names(&h), ["r.xml", "r.dtd"]);
}

#[test]
fn a_schema_document_is_not_offered_load_schema_even_if_it_names_something() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell(&Arc::new(Web::default()));
    let text = format!("<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\" {XSI} xsi:noNamespaceSchemaLocation=\"other.xsd\"/>");
    open(&mut h, &file(dir.path(), "s.xsd", &text));
    assert!(!h.state_mut().execute("xml.load_schema"), "Load Schema is for XML documents");
}

#[test]
fn schema_resolution_shows_the_places_it_looked_in_and_opens_nothing() {
    let dir = tempfile::tempdir().expect("dir");
    file(dir.path(), "n.xsd", SCHEMA);
    let mut h = shell(&Arc::new(Web::default()));
    open(&mut h, &file(dir.path(), "n.xml", &format!("<n {XSI} xsi:noNamespaceSchemaLocation=\"n.xsd\">1</n>")));
    assert!(h.state_mut().execute("xml.schema_resolution"));
    let started = Instant::now();
    while h.query_by_label_contains("Found in a file").is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the trail");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(tab_names(&h), ["n.xml"], "the trail is shown, not opened");
}

#[test]
fn schema_resolution_says_which_place_failed() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell(&Arc::new(Web::default()));
    open(&mut h, &file(dir.path(), "n.xml", &format!("<n {XSI} xsi:noNamespaceSchemaLocation=\"missing.xsd\">1</n>")));
    assert!(h.state_mut().execute("xml.schema_resolution"));
    let started = Instant::now();
    while h.query_by_label_contains("failed (").is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the trail");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(tab_names(&h), ["n.xml"]);
}
