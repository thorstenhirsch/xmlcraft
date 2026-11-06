//! The schema chip in the header: it names the JSON Schema a JSON or YAML document uses, and the UBL schema that ships with the app for
//! an invoice, instead of saying that the document follows no schema.

use std::path::Path;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_ui_egui::XmlCraftApp;

const INVOICE: &str = r#"<Invoice xmlns="urn:oasis:names:specification:ubl:schema:xsd:Invoice-2" xmlns:cbc="urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2"><cbc:ID>1</cbc:ID></Invoice>"#;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

fn open(h: &mut Harness<'static, XmlCraftApp>, dir: &Path, name: &str, text: &str, ready: impl Fn(&XmlCraftApp) -> bool) {
    std::fs::write(dir.join(name), text).expect("writes");
    assert!(h.state_mut().open_path(&dir.join(name)));
    let started = Instant::now();
    while !ready(h.state()) {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for {name}");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(3);
}

#[test]
fn a_ubl_invoice_names_the_schema_that_ships_with_the_app() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, dir.path(), "i.xml", INVOICE, |app| app.has_grammar());
    assert!(h.query_by_label_contains("UBL 2.1 Invoice").is_some(), "the chip names the bundled schema");
    assert!(h.query_by_label("No schema").is_none());
    open(&mut h, dir.path(), "plain.xml", "<a/>", |app| app.problems().is_some());
    assert!(h.query_by_label("No schema").is_some(), "a document that follows none still says so");
}

#[test]
fn a_json_document_names_its_schema_and_one_that_names_none_says_so() {
    let dir = tempfile::tempdir().expect("dir");
    std::fs::write(dir.path().join("s.json"), r#"{"type": "object"}"#).expect("writes");
    let mut h = shell();
    open(&mut h, dir.path(), "d.json", r#"{"$schema": "s.json", "a": 1}"#, |app| app.has_data_schema());
    assert!(h.query_by_label_contains("Schema · s.json").is_some(), "the chip names the schema file");
    assert!(h.query_by_label("No schema").is_none());
    open(&mut h, dir.path(), "e.json", r#"{"a": 1}"#, |app| app.problems().is_some());
    assert!(h.query_by_label("No schema").is_some());
}
