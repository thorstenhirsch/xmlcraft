//! Convert in the real shell: the form, its options, and the document it opens.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_ui_egui::XmlCraftApp;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 900.0)).build_eframe(|cc| XmlCraftApp::new(cc));
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

fn names(h: &Harness<'static, XmlCraftApp>) -> Vec<String> {
    h.state().session.documents().iter().map(|d| d.name().to_owned()).collect()
}

fn text_of(h: &Harness<'static, XmlCraftApp>, name: &str) -> String {
    h.state().session.documents().iter().find(|d| d.name() == name).map(|d| d.editor().text()).unwrap_or_default()
}

#[test]
fn an_xml_document_is_converted_to_json_and_opens_as_a_new_document() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "order.xml", "<order id=\"7\"><item>Tea</item></order>"));
    assert!(h.state_mut().execute("file.convert"));
    h.run_steps(3);
    assert!(h.query_by_label("Convert document").is_some(), "the form is up");
    h.get_by_label("JSON").click();
    h.run_steps(2);
    h.get_by_label("Convert").click();
    h.run_steps(3);
    let json = text_of(&h, "order.json");
    assert!(json.contains("\"@id\": \"7\"") && json.contains("\"item\": \"Tea\""), "{json}");
    let shown = h.state().session.active_document().expect("shown");
    assert_eq!(shown.name(), "order.json", "the new document is the one shown");
    assert!(shown.is_dirty() && shown.path().is_none());
    assert!(names(&h).contains(&"order.xml".to_owned()), "the source is untouched");
    assert!(h.state().status.contains("order.json"), "{}", h.state().status);
}

#[test]
fn json_goes_to_yaml_by_default_and_a_refused_conversion_says_why() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "d.json", "{\"a\": [1, 2]}"));
    assert!(h.state_mut().execute("file.convert"));
    h.run_steps(3);
    h.get_by_label("Convert").click();
    h.run_steps(3);
    assert_eq!(text_of(&h, "d.yaml"), "a:\n  - 1\n  - 2\n");
    // A list of numbers is not documents: BSON refuses it, and the form stays up with the reason.
    open(&mut h, &file(dir.path(), "e.json", "[1, 2]"));
    assert!(h.state_mut().execute("file.convert"));
    h.run_steps(3);
    h.get_by_label("BSON").click();
    h.run_steps(2);
    h.get_by_label("Convert").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("cannot be converted").is_some(), "the reason is shown in the form");
    assert!(!names(&h).contains(&"e.bson".to_owned()));
}

#[test]
fn nothing_to_convert_when_nothing_is_open() {
    let mut h = shell();
    assert!(!h.state_mut().execute("file.convert"));
}

#[test]
fn the_options_of_the_form_reach_the_conversion() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "d.json", "{\"a\": 1}"));
    assert!(h.state_mut().execute("file.convert"));
    h.run_steps(3);
    h.get_by_label("XML").click();
    h.run_steps(2);
    h.get_by_label("XPath 3.1 (json-to-xml)").click();
    h.run_steps(2);
    h.get_by_label("Convert").click();
    h.run_steps(3);
    let xml = text_of(&h, "d.xml");
    assert!(xml.contains("http://www.w3.org/2005/xpath-functions") && xml.contains("<number key=\"a\">1</number>"), "{xml}");
}

#[test]
fn json_lines_are_converted_as_a_list_and_the_same_format_is_not_offered() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "d.jsonl", "{\"a\":1}\n{\"a\":2}\n"));
    assert!(h.state_mut().execute("file.convert"));
    h.run_steps(3);
    h.get_by_label("Convert").click();
    h.run_steps(3);
    assert_eq!(text_of(&h, "d.yaml"), "- a: 1\n- a: 2\n");
    // Converting a document to the format it is in does nothing: the button is disabled.
    open(&mut h, &file(dir.path(), "e.xml", "<a/>"));
    assert!(h.state_mut().execute("file.convert"));
    h.run_steps(3);
    h.get_by_label("XML").click();
    h.run_steps(2);
    h.get_by_label("Convert").click();
    h.run_steps(3);
    assert!(h.query_by_label("Convert document").is_some(), "the form is still up");
    assert!(!names(&h).contains(&"e.xml.xml".to_owned()) && names(&h).iter().filter(|n| n.as_str() == "e.xml").count() == 1);
}

#[test]
fn the_indentation_of_the_form_reaches_the_conversion() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "d.yaml", "a:\n  b: 1\n"));
    assert!(h.state_mut().execute("file.convert"));
    h.run_steps(3);
    h.get_by_label("4 spaces").click();
    h.run_steps(2);
    h.get_by_label("Convert").click();
    h.run_steps(3);
    assert_eq!(text_of(&h, "d.json"), "{\n    \"a\": {\n        \"b\": 1\n    }\n}\n");
}
