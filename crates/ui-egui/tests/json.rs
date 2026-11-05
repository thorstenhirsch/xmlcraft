//! A JSON document in the real shell: read by its own rules, not by the XML ones.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_ui_egui::XmlCraftApp;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

fn file(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path
}

fn open(h: &mut Harness<'static, XmlCraftApp>, path: &Path) {
    assert!(h.state_mut().open_path(path));
    let started = Instant::now();
    while h.state().problems().is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the first analysis");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
}

const TEXT: &str = "{\n  \"name\": \"x\", // note\n  \"tags\": [1, 2],\n}\n";

#[test]
fn json_is_checked_as_json_and_a_relative_that_allows_the_text_is_not_complained_about() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.json", TEXT));
    assert_eq!(h.state().problems(), Some((2, 0)), "the comment and the trailing comma");
    open(&mut h, &file(dir.path(), "b.jsonc", TEXT));
    assert_eq!(h.state().problems(), Some((0, 0)), "JSON with comments allows both");
}

#[test]
fn a_json_document_is_not_offered_xml_schema_tools() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.xml", "<a/>"));
    assert!(h.query_by_label("Schema…").is_some());
    open(&mut h, &file(dir.path(), "a.json", "{}"));
    assert!(h.query_by_label("Schema…").is_none());
    assert!(h.query_by_label("XML…").is_none());
}

#[test]
fn the_outline_of_a_json_document_lists_its_members() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "o.json", "{\"alpha\": {\"beta\": 1}}"));
    assert!(h.query_by_label("alpha").is_some(), "the outline names the member");
    assert!(h.query_by_label("beta 1").is_some(), "with a look at its value");
}

#[test]
fn save_as_under_another_extension_reads_the_text_again_in_the_new_language() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.xml", "{\"a\": 1}"));
    let as_xml = h.state().problems();
    assert!(as_xml.is_some_and(|(errors, _)| errors > 0), "this is not XML: {as_xml:?}");
    let until = |h: &mut Harness<'static, XmlCraftApp>, what: &str, done: &dyn Fn(Option<(usize, usize)>) -> bool| {
        let started = Instant::now();
        while !done(h.state().problems()) {
            assert!(started.elapsed() < Duration::from_secs(10), "waiting for {what}");
            h.run_steps(1);
            std::thread::sleep(Duration::from_millis(5));
        }
        h.run_steps(2);
    };
    let rename = |h: &mut Harness<'static, XmlCraftApp>, name: &str| {
        h.state_mut().session.active_document_mut().expect("a document").mark_saved(dir.path().join(name), None);
    };
    rename(&mut h, "a.json");
    until(&mut h, "the JSON reading", &|p| p == Some((0, 0)));
    rename(&mut h, "a.xml");
    until(&mut h, "the XML reading", &|p| p == as_xml);
}
