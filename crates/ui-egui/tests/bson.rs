//! A BSON file in the real shell: shown as Extended JSON, saved as the bytes that text stands for.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_bson::{Bson, Document, read_all, write_all};
use xmlcraft_ui_egui::XmlCraftApp;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
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

fn text(h: &Harness<'static, XmlCraftApp>) -> String {
    h.state().session.active_document().expect("a document is shown").editor().text()
}

fn documents() -> Vec<Document> {
    vec![
        Document { fields: vec![("name".into(), Bson::String("alpha".into())), ("odd".into(), Bson::Double(f64::from_bits(0x7ff8_0000_0000_0001)))] },
        Document { fields: vec![("name".into(), Bson::String("beta".into())), ("n".into(), Bson::Int32(2))] },
    ]
}

fn file(dir: &Path) -> PathBuf {
    let path = dir.join("data.bson");
    std::fs::write(&path, write_all(&documents()).expect("writes")).expect("writes");
    path
}

#[test]
fn a_bson_file_is_shown_as_its_text_checked_and_saved_with_the_unedited_documents_untouched() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path());
    let original = std::fs::read(&path).expect("reads");
    let mut h = shell();
    open(&mut h, &path);
    assert_eq!(h.state().problems(), Some((0, 0)));
    assert!(text(&h).contains("{\"$numberDouble\":\"NaN\"}"), "{}", text(&h));
    assert!(h.query_by_label("XPath").is_none() && h.query_by_label("Convert").is_some(), "the data tools, not the XML ones");
    // Saved without a change: nothing changes on disk (the NaN keeps its payload although the text cannot say it).
    h.state_mut().execute("file.save");
    h.run_steps(3);
    assert_eq!(std::fs::read(&path).expect("reads"), original);
    // Edit the second document: the first is still the exact bytes.
    let edited = text(&h).replace("beta", "gamma");
    {
        let doc = h.state_mut().session.active_document_mut().expect("a document");
        doc.editor_mut().select_all();
        doc.editor_mut().type_text(&edited);
    }
    h.run_steps(3);
    assert!(h.state().has_unsaved_changes());
    h.state_mut().execute("file.save");
    h.run_steps(3);
    let saved = std::fs::read(&path).expect("reads");
    let first = write_all(&documents()[..1]).expect("writes");
    assert_eq!(&saved[..first.len()], first.as_slice(), "the first document is byte for byte what it was");
    assert_eq!(read_all(&saved).expect("reads")[1].fields[0].1, Bson::String("gamma".into()));
    assert!(!h.state().has_unsaved_changes());
}

#[test]
fn text_that_is_not_extended_json_is_underlined_and_not_saved() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path());
    let original = std::fs::read(&path).expect("reads");
    let mut h = shell();
    open(&mut h, &path);
    {
        let doc = h.state_mut().session.active_document_mut().expect("a document");
        doc.editor_mut().select_all();
        doc.editor_mut().type_text("{\"id\": {\"$oid\": \"zz\"}}");
    }
    let started = Instant::now();
    while h.state().problems() == Some((0, 0)) {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the new reading");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(h.state().problems(), Some((1, 0)), "an $oid that is not 24 hex digits");
    h.state_mut().execute("file.save");
    h.run_steps(4);
    assert!(h.query_by_label("Couldn't save the file").is_some(), "the reason is shown");
    assert_eq!(std::fs::read(&path).expect("reads"), original, "nothing was written");
}

#[test]
fn a_file_that_is_not_bson_is_not_opened_as_text() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("fake.bson");
    std::fs::write(&path, b"this is not bson").expect("writes");
    let mut h = shell();
    assert!(!h.state_mut().open_path(&path), "refused");
    h.run_steps(3);
    assert!(h.state().tabs().iter().all(|t| t.name != "fake.bson"), "no tab for it");
    assert!(h.query_by_label_contains("this is not a BSON file").is_some(), "and the reason is shown");
}

#[test]
fn the_json_schema_a_bson_document_names_completes_its_names() {
    let dir = tempfile::tempdir().expect("dir");
    let schema = r#"{"type": "object", "properties": {"name": {"type": "string"}, "kind": {"enum": ["alpha", "beta"]}}}"#;
    std::fs::write(dir.path().join("s.json"), schema).expect("writes");
    let path = dir.path().join("named.bson");
    let docs = vec![Document { fields: vec![("$schema".into(), Bson::String("s.json".into()))] }];
    std::fs::write(&path, write_all(&docs).expect("writes")).expect("writes");
    let mut h = shell();
    open(&mut h, &path);
    let started = Instant::now();
    while !h.state().has_data_schema() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the schema");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    let at = text(&h).find('{').map_or(1, |i| i + 1);
    h.state_mut().session.active_document_mut().expect("a document").editor_mut().set_selection(at, at);
    assert!(h.state_mut().execute("edit.complete"));
    h.run_steps(3);
    assert!(h.query_by_label("Complete name").is_some(), "the schema's names are offered in the Extended JSON text");
    assert!(h.query_by_label("Complete kind").is_some());
}
