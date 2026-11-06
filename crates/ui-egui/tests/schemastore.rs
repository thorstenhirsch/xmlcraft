//! SchemaStore in the Assign Schema window, in the real shell: the schemas for the file are fetched through the app's network (a mock
//! web here: nothing goes to the real one), listed, and the one that is chosen is assigned as a `$schema` member.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_resolve::{Conditions, FetchError, FetchLimits, FetchOutcome, Fetched, FixedClock, MemoryCache};
use xmlcraft_ui_egui::{Network, XmlCraftApp};

const CATALOG: &str = r#"{"schemas": [
  {"name": "package.json", "description": "NPM manifest", "fileMatch": ["package.json"], "url": "https://json.schemastore.org/package.json"},
  {"name": "Other", "fileMatch": ["other.json"], "url": "https://json.schemastore.org/other.json"}]}"#;

#[derive(Default)]
struct Web {
    asked: Mutex<Vec<String>>,
}

impl xmlcraft_resolve::Fetcher for Web {
    fn fetch(&self, url: &str, _: &Conditions, _: &FetchLimits) -> Result<FetchOutcome, FetchError> {
        self.asked.lock().expect("lock").push(url.to_owned());
        if url.ends_with("schemastore.org/api/json/catalog.json") {
            Ok(FetchOutcome::Fresh(Fetched { body: CATALOG.as_bytes().to_vec(), final_url: url.to_owned(), etag: None, last_modified: None }))
        } else {
            Err(FetchError::Status(404))
        }
    }
}

fn shell(web: &Arc<Web>) -> Harness<'static, XmlCraftApp> {
    let network = Network::new(Some(web.clone()), Some(Arc::new(MemoryCache::new())), Arc::new(FixedClock(1_000)));
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(move |cc| XmlCraftApp::with_network(cc, network.clone()));
    h.run_steps(4);
    h
}

fn file(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path
}

#[test]
fn the_schemas_for_a_file_are_listed_from_schemastore_and_the_chosen_one_is_assigned() {
    let dir = tempfile::tempdir().expect("dir");
    let web = Arc::new(Web::default());
    let mut h = shell(&web);
    assert!(h.state_mut().open_path(&file(dir.path(), "package.json", "{\n  \"name\": \"x\"\n}\n")));
    h.run_steps(3);
    assert!(h.state_mut().execute("xml.assign_schema"));
    h.run_steps(3);
    assert!(web.asked.lock().expect("lock").is_empty(), "opening the window asks nobody");
    h.get_by_label("Find a schema for this file").click_accesskit();
    let started = Instant::now();
    while h.query_by_label_contains("NPM manifest").is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for SchemaStore");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(h.query_by_label_contains("Other").is_none(), "only the schemas for this file name are listed");
    assert_eq!(web.asked.lock().expect("lock").len(), 1);
    h.get_by_label_contains("NPM manifest").click_accesskit();
    h.run_steps(2);
    h.get_by_label("Assign").click_accesskit();
    h.run_steps(4);
    let text = h.state().session.active_document().expect("shown").editor().text();
    assert!(text.contains("\"$schema\": \"https://json.schemastore.org/package.json\""), "{text}");
}

#[test]
fn a_file_with_no_schema_in_the_catalog_says_so() {
    let dir = tempfile::tempdir().expect("dir");
    let web = Arc::new(Web::default());
    let mut h = shell(&web);
    assert!(h.state_mut().open_path(&file(dir.path(), "mine.json", "{}")));
    h.run_steps(3);
    assert!(h.state_mut().execute("xml.assign_schema"));
    h.run_steps(3);
    h.get_by_label("Find a schema for this file").click_accesskit();
    let started = Instant::now();
    while h.query_by_label_contains("no schema for this file name").is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the answer");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(10));
    }
}
