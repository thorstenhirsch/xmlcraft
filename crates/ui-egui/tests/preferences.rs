//! The Preferences window: the settings the app keeps, changed in one place. The network is a mock.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use xmlcraft_resolve::{Conditions, FetchError, FetchLimits, FetchOutcome, Fetched, FixedClock, MemoryCache};
use xmlcraft_ui_egui::theme::ThemePreference;
use xmlcraft_ui_egui::{Network, XmlCraftApp};

const SCHEMA: &str = "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"><xs:element name=\"n\" type=\"xs:int\"/></xs:schema>";
const DOCUMENT: &str = "<n xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"http://schemas.test/n.xsd\">7</n>";

#[derive(Default)]
struct Web {
    asked: Mutex<usize>,
}

impl xmlcraft_resolve::Fetcher for Web {
    fn fetch(&self, url: &str, _: &Conditions, _: &FetchLimits) -> Result<FetchOutcome, FetchError> {
        *self.asked.lock().expect("lock") += 1;
        if url.ends_with("schemas.test/n.xsd") {
            Ok(FetchOutcome::Fresh(Fetched { body: SCHEMA.as_bytes().to_vec(), final_url: url.to_owned(), etag: None, last_modified: None }))
        } else {
            Err(FetchError::Status(404))
        }
    }
}

fn shell(web: &Arc<Web>) -> Harness<'static, XmlCraftApp> {
    let network = Network::new(Some(web.clone()), Some(Arc::new(MemoryCache::new())), Arc::new(FixedClock(1_000)));
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 900.0)).build_eframe(move |cc| XmlCraftApp::with_network(cc, network.clone()));
    h.run_steps(4);
    h
}

fn open_preferences(h: &mut Harness<'static, XmlCraftApp>) {
    assert!(h.state_mut().execute("app.preferences"));
    h.run_steps(3);
}

fn file(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path
}

#[test]
fn the_window_opens_from_the_edit_menu_command_and_done_closes_it() {
    let mut h = shell(&Arc::new(Web::default()));
    open_preferences(&mut h);
    assert!(h.query_by_label("Preferences").is_some());
    for section in ["APPEARANCE", "EDITING", "SCHEMAS"] {
        assert!(h.query_by_label(section).is_some(), "{section}");
    }
    h.get_by_label("Done").click();
    h.run_steps(3);
    assert!(h.query_by_label("APPEARANCE").is_none(), "closed");
}

#[test]
fn the_theme_the_size_and_the_switches_change_at_once() {
    let mut h = shell(&Arc::new(Web::default()));
    open_preferences(&mut h);
    h.get_by_label("Dark").click();
    h.run_steps(2);
    assert_eq!(h.state().theme_preference, ThemePreference::Dark);
    h.get_by_label("Zoom In").click();
    h.run_steps(2);
    assert!((h.state().editor_zoom() - 1.1).abs() < 1e-4);
    h.get_by_label("Actual Size").click();
    h.run_steps(2);
    assert_eq!(h.state().editor_zoom(), 1.0);
    assert_eq!(h.state().command_checked("xml.validate_typing"), Some(true));
    h.get_by_label("Validate while typing").click();
    h.run_steps(2);
    assert_eq!(h.state().command_checked("xml.validate_typing"), Some(false));
    assert_eq!(h.state().command_checked("xml.work_offline"), Some(false));
    h.get_by_label("Work offline").click();
    h.run_steps(2);
    assert_eq!(h.state().command_checked("xml.work_offline"), Some(true), "the same switch as XML ▸ Work Offline");
}

#[test]
fn the_downloads_are_counted_and_can_be_deleted() {
    let dir = tempfile::tempdir().expect("dir");
    let web = Arc::new(Web::default());
    let mut h = shell(&web);
    open_preferences(&mut h);
    assert!(h.query_by_label_contains("0 files").is_some());
    assert!(h.get_by_label("Clear Cache").accesskit_node().is_disabled(), "nothing to delete");
    h.state_mut().session.open(Some(file(dir.path(), "n.xml", DOCUMENT)), DOCUMENT.as_bytes(), None).expect("opens");
    // The cache is written by the worker after the fetch: wait for the window to say so, not for the fetch to start.
    let started = Instant::now();
    while h.query_by_label_contains("1 files").is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the cache to hold the schema");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(10));
    }
    h.get_by_label("Clear Cache").click();
    h.run_steps(3);
    // (The document asks for its schema again, so the cache may hold it again by now: the message is what says it was cleared.)
    assert!(h.state().status.contains("Deleted 1 downloaded files"), "{}", h.state().status);
}

#[test]
fn a_catalog_is_added_listed_kept_once_and_removed() {
    let mut h = shell(&Arc::new(Web::default()));
    h.state_mut().add_user_catalog("/x/catalog.xml");
    h.state_mut().add_user_catalog("/x/catalog.xml");
    h.state_mut().add_user_catalog("  ");
    assert_eq!(h.state().user_catalogs(), ["/x/catalog.xml"], "twice is once, nothing is nothing");
    open_preferences(&mut h);
    assert!(h.query_by_label("/x/catalog.xml").is_some(), "the path is listed");
    h.get_by_label("Remove /x/catalog.xml").click();
    h.run_steps(3);
    assert!(h.state().user_catalogs().is_empty());
    h.state_mut().remove_user_catalog(5);
    assert!(h.query_by_label("Add Catalog…").is_some());
}

#[test]
fn a_catalog_path_is_trimmed_once_so_that_it_is_not_listed_twice() {
    let mut h = shell(&Arc::new(Web::default()));
    h.state_mut().add_user_catalog("  /x/c.xml ");
    h.state_mut().add_user_catalog("/x/c.xml");
    assert_eq!(h.state().user_catalogs(), ["/x/c.xml"]);
}
