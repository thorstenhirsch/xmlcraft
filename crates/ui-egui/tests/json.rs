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

fn text(h: &Harness<'static, XmlCraftApp>) -> String {
    h.state().session.active_document().expect("a document is shown").editor().text()
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

/// Save As under `name` in the folder of the test: the picker answers with that path.
fn save_as(h: &mut Harness<'static, XmlCraftApp>, dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    h.state_mut().pick_override = Some(vec![path.clone()]);
    h.state_mut().execute("file.save_as");
    h.run_steps(4);
    path
}

fn until(h: &mut Harness<'static, XmlCraftApp>, what: &str, done: &dyn Fn(&XmlCraftApp) -> bool) {
    let started = Instant::now();
    while !done(h.state()) {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for {what}");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
}

#[test]
fn save_as_under_another_extension_reads_the_text_again_in_the_new_language_and_forgets_the_old_checks() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    // XML with a DTD of its own, checked: it has a grammar and a verdict.
    open(&mut h, &file(dir.path(), "a.xml", "<!DOCTYPE r [<!ELEMENT r EMPTY>]><r>x</r>"));
    h.state_mut().execute("xml.validate");
    until(&mut h, "the validation", &|app| app.validity().is_some());
    assert!(h.state().has_grammar());
    let as_xml = h.state().problems();
    // Saved as JSON, the text is read as JSON (not well-formed there: no value), the file is written, and nothing of the DTD is left.
    let path = save_as(&mut h, dir.path(), "a.json");
    until(&mut h, "the JSON reading", &|app| app.problems().is_some_and(|(errors, _)| errors > 0));
    assert!(path.exists(), "the file was written");
    assert_eq!(h.state().tabs().first().map(|t| t.name.clone()).as_deref(), Some("a.json"));
    assert_eq!(h.state().validity(), None, "the old validation is gone");
    assert!(!h.state().has_grammar(), "and so is the DTD");
    assert!(h.state().underlined().iter().all(|(r, _)| r.end <= 41), "underlines are those of the JSON reading");
    // And back.
    save_as(&mut h, dir.path(), "b.xml");
    until(&mut h, "the XML reading", &|app| app.problems() == as_xml);
}

#[test]
fn saving_json_as_xml_in_an_encoding_the_bytes_do_not_tell_declares_it() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.json", "{\"a\": \"é\"}"));
    h.state_mut().session.active_document_mut().expect("a document").set_encoding(xmlcraft_text::encoding::Encoding::WINDOWS_1252, false);
    let path = save_as(&mut h, dir.path(), "a.xml");
    let written = std::fs::read(&path).expect("reads");
    assert!(written.starts_with(b"<?xml version=\"1.0\" encoding=\"windows-1252\"?>"), "{:?}", String::from_utf8_lossy(&written));
    assert!(written.ends_with(&[0xE9, b'"', b'}']), "and the text is in that encoding");
}

#[test]
fn a_save_as_to_xml_that_stops_at_a_character_the_encoding_cannot_hold_leaves_the_text_as_it_was() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.json", "{\"a\": 1} // ł"));
    // (JSONC would read the comment; the point here is the text.)
    h.state_mut().session.active_document_mut().expect("a document").set_encoding(xmlcraft_text::encoding::Encoding::WINDOWS_1252, false);
    let before = text(&h);
    let path = save_as(&mut h, dir.path(), "a.xml");
    assert!(h.query_by_label("a.json has a character its encoding cannot hold").is_some(), "the question is asked");
    assert_eq!(text(&h), before, "the XML declaration the attempt wrote is taken back");
    assert!(!path.exists(), "and nothing was written");
}

#[test]
fn pretty_print_and_minify_work_on_json_and_a_text_with_an_error_is_left_alone() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "f.json", "{\"a\":[1,2],\"b\":{}}"));
    assert!(h.state_mut().execute("xml.pretty_print"));
    h.run_steps(2);
    assert_eq!(text(&h), "{\n  \"a\": [\n    1,\n    2\n  ],\n  \"b\": {}\n}");
    assert!(h.state_mut().execute("xml.minify"));
    h.run_steps(2);
    assert_eq!(text(&h), "{\"a\":[1,2],\"b\":{}}");
    assert!(h.state_mut().execute("edit.undo"));
    h.run_steps(2);
    assert!(text(&h).starts_with("{\n  \"a\""), "one undo step");
    open(&mut h, &file(dir.path(), "bad.json", "{\"a\": [1,}"));
    h.state_mut().execute("xml.pretty_print");
    h.run_steps(3);
    assert!(h.query_by_label("The document is not well-formed").is_some());
    assert_eq!(text(&h), "{\"a\": [1,}", "left as it was");
    open(&mut h, &file(dir.path(), "l.jsonl", "{\"a\": 1}\n{\"b\":  2}\n"));
    h.state_mut().execute("xml.pretty_print");
    h.run_steps(3);
    assert!(h.query_by_label("Nothing was changed").is_some(), "JSON Lines are not indented");
}

#[test]
fn f8_checks_a_json_document_against_the_schema_it_names_and_lists_what_is_wrong() {
    let dir = tempfile::tempdir().expect("dir");
    file(dir.path(), "schema.json", r#"{"type": "object", "properties": {"n": {"type": "integer", "minimum": 1}}, "required": ["name"]}"#);
    let mut h = shell();
    let text = "{\n  \"$schema\": \"schema.json\",\n  \"n\": 0\n}\n";
    open(&mut h, &file(dir.path(), "doc.json", text));
    assert!(h.state_mut().execute("xml.validate"));
    let started = Instant::now();
    while h.state().validity().is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the validation");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
    assert_eq!(h.state().validity(), Some((2, 0)), "the minimum and the required name");
    assert!(h.query_by_label("2 validity errors").is_some() || h.state().status.contains("2 validity errors"), "{}", h.state().status);
    assert!(h.query_by_label_contains("minimum").is_some(), "the minimum is listed");
    assert!(h.query_by_label_contains("required").is_some(), "and the required name");
    // Fixed, it is valid.
    {
        let doc = h.state_mut().session.active_document_mut().expect("shown");
        doc.editor_mut().select_all();
        doc.editor_mut().type_text("{\n  \"$schema\": \"schema.json\",\n  \"name\": \"x\",\n  \"n\": 2\n}\n");
    }
    h.run_steps(2);
    assert!(h.state_mut().execute("xml.validate"));
    let started = Instant::now();
    while h.state().validity() != Some((0, 0)) {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the second validation");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(h.state().status.contains("is valid"), "{}", h.state().status);
}

#[test]
fn f8_checks_an_openapi_description_and_a_json_schema_against_the_bundled_schemas() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    let wait = |h: &mut Harness<'static, XmlCraftApp>, count: (usize, usize)| {
        let started = Instant::now();
        while h.state().validity() != Some(count) {
            assert!(started.elapsed() < Duration::from_secs(10), "waiting for the validation: {:?}", h.state().validity());
            h.run_steps(1);
            std::thread::sleep(Duration::from_millis(5));
        }
        h.run_steps(2);
    };
    // No `$schema` and no file next to it: the version the description says is enough.
    let api = "{\n  \"openapi\": \"3.0.3\",\n  \"info\": {\"title\": 5, \"version\": \"1\"},\n  \"paths\": {}\n}\n";
    open(&mut h, &file(dir.path(), "api.json", api));
    assert!(h.state_mut().execute("xml.validate"));
    wait(&mut h, (1, 0));
    assert!(h.query_by_label_contains("title").is_some(), "the problem is listed");
    // A JSON Schema that names its draft is checked against that draft's meta-schema.
    let schema = "{\n  \"$schema\": \"https://json-schema.org/draft/2020-12/schema\",\n  \"type\": \"strin\"\n}\n";
    open(&mut h, &file(dir.path(), "schema.json", schema));
    assert!(h.state_mut().execute("xml.validate"));
    let started = Instant::now();
    while h.state().validity().is_none_or(|(errors, _)| errors == 0) {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the second validation");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(h.state().validity().is_some_and(|(errors, _)| errors >= 1), "{:?}", h.state().validity());
}
