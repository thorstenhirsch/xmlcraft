//! Assign Schema in the real shell: a DTD or a schema written into the document as an edit that can be undone.

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

fn wait(h: &mut Harness<'static, XmlCraftApp>, what: &str, done: impl Fn(&mut Harness<'static, XmlCraftApp>) -> bool) {
    let started = Instant::now();
    while !done(h) {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for {what}");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
}

fn open(h: &mut Harness<'static, XmlCraftApp>, path: &Path) {
    assert!(h.state_mut().open_path(path));
    wait(h, "the first analysis", |h| h.state().problems().is_some());
}

fn text(h: &Harness<'static, XmlCraftApp>) -> String {
    h.state().session.active_document().map(|d| d.editor().text()).unwrap_or_default()
}

const FIELD: &str = "The location of the schema or the DTD";

fn assign_with(h: &mut Harness<'static, XmlCraftApp>, location: &str) {
    assert!(h.state_mut().execute("xml.assign_schema"));
    h.run_steps(3);
    assert!(h.query_by_label("Assign Schema").is_some(), "the form is up");
    h.get_by_label(FIELD).focus();
    h.run_steps(1);
    h.get_by_label(FIELD).type_text(location);
    h.run_steps(2);
    h.get_by_label("Assign").click();
    h.run_steps(3);
}

#[test]
fn a_dtd_is_assigned_as_a_doctype_and_the_edit_can_be_undone() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "order.xml", "<?xml version=\"1.0\"?>\n<order>\n  <item/>\n</order>\n"));
    assign_with(&mut h, "order.dtd");
    assert_eq!(text(&h), "<?xml version=\"1.0\"?>\n<!DOCTYPE order SYSTEM \"order.dtd\">\n<order>\n  <item/>\n</order>\n");
    assert!(h.query_by_label("Assign Schema").is_none(), "the form is gone");
    assert!(h.state().status.contains("order.dtd"), "{}", h.state().status);
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().undo();
    assert_eq!(text(&h), "<?xml version=\"1.0\"?>\n<order>\n  <item/>\n</order>\n", "one step takes it back");
}

#[test]
fn a_schema_chosen_with_browse_is_written_from_the_folder_of_the_document_and_then_validates_it() {
    let dir = tempfile::tempdir().expect("dir");
    let schema = file(
        dir.path(),
        "order.xsd",
        "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\" targetNamespace=\"urn:o\" elementFormDefault=\"qualified\"><xs:element name=\"order\" type=\"xs:int\"/></xs:schema>",
    );
    let mut h = shell();
    open(&mut h, &file(dir.path(), "good.xml", "<order xmlns=\"urn:o\">7</order>"));
    assert!(h.state_mut().execute("xml.assign_schema"));
    h.run_steps(3);
    h.state_mut().pick_override = Some(vec![schema]);
    h.get_by_label("Browse…").click();
    h.run_steps(4);
    h.get_by_label("Assign").click();
    h.run_steps(3);
    let assigned = text(&h);
    assert!(
        assigned.contains("xsi:schemaLocation=\"urn:o order.xsd\"") && assigned.contains("xmlns:xsi="),
        "the namespace was read from the schema: {assigned}"
    );
    // F8 now has a schema to check against, and the document is valid.
    assert!(h.state_mut().execute("xml.validate"));
    wait(&mut h, "the validation", |h| h.state().validity().is_some());
    assert_eq!(h.state().validity(), Some((0, 0)), "{}", h.state().status);
    // The same document with a wrong value is found wanting.
    let editor = h.state_mut().session.active_document_mut().expect("shown").editor_mut();
    let at = editor.text().find('7').expect("the value");
    assert!(editor.apply_edits(&[xmlcraft_text::Edit { range: at..at + 1, text: "seven".to_owned() }]));
    assert!(h.state_mut().execute("xml.validate"));
    wait(&mut h, "the validation", |h| h.state().validity().is_some());
    assert_eq!(h.state().validity(), Some((1, 0)));
}

#[test]
fn what_cannot_be_assigned_is_said_in_the_form_and_nothing_changes() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.xml", "<!DOCTYPE a>\n<a/>"));
    assert!(h.state_mut().execute("xml.assign_schema"));
    h.run_steps(3);
    h.get_by_label("Assign").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("Type where the schema").is_some(), "an empty location is said");
    h.get_by_label(FIELD).focus();
    h.run_steps(1);
    h.get_by_label(FIELD).type_text("a.xsd");
    h.run_steps(2);
    h.get_by_label("Assign").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("has a DOCTYPE").is_some(), "a schema cannot be assigned where a DOCTYPE rules");
    assert!(h.query_by_label("Assign Schema").is_some(), "the form stays, for another try");
    assert_eq!(text(&h), "<!DOCTYPE a>\n<a/>");
    // Saying that it is a DTD makes it a DTD.
    h.get_by_label("a DTD").click();
    h.run_steps(2);
    h.get_by_label("Assign").click();
    h.run_steps(3);
    assert_eq!(text(&h), "<!DOCTYPE a SYSTEM \"a.xsd\">\n<a/>");
}

#[test]
fn cancel_changes_nothing() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.xml", "<a/>"));
    assert!(h.state_mut().execute("xml.assign_schema"));
    h.run_steps(3);
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(h.query_by_label("Assign Schema").is_none());
    assert_eq!(text(&h), "<a/>");
}

#[test]
fn what_is_there_already_is_said_and_leaves_nothing_to_undo() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.xml", "<a/>"));
    assign_with(&mut h, "a.dtd");
    assert_eq!(text(&h), "<!DOCTYPE a SYSTEM \"a.dtd\">\n<a/>");
    assign_with(&mut h, "a.dtd");
    assert!(h.query_by_label("Assign Schema").is_none(), "the form is gone");
    assert!(h.state().status.contains("already"), "{}", h.state().status);
    assert_eq!(text(&h), "<!DOCTYPE a SYSTEM \"a.dtd\">\n<a/>");
    // The second time was no step: one undo takes the first back.
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().undo();
    assert_eq!(text(&h), "<a/>");
}

#[test]
fn the_kind_overrules_what_the_name_looks_like() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.xml", "<a/>"));
    assert!(h.state_mut().execute("xml.assign_schema"));
    h.run_steps(3);
    h.get_by_label(FIELD).focus();
    h.run_steps(1);
    h.get_by_label(FIELD).type_text("rules.dtd");
    h.run_steps(2);
    h.get_by_label("an XML Schema").click();
    h.run_steps(2);
    h.get_by_label("Assign").click();
    h.run_steps(3);
    assert!(text(&h).contains("xsi:noNamespaceSchemaLocation=\"rules.dtd\""), "{}", text(&h));
    // And the other way round.
    assert!(h.state_mut().session.active_document_mut().expect("shown").editor_mut().undo());
    assert!(h.state_mut().execute("xml.assign_schema"));
    h.run_steps(3);
    h.get_by_label(FIELD).focus();
    h.run_steps(1);
    h.get_by_label(FIELD).type_text("rules.xsd");
    h.run_steps(2);
    h.get_by_label("a DTD").click();
    h.run_steps(2);
    h.get_by_label("Assign").click();
    h.run_steps(3);
    assert_eq!(text(&h), "<!DOCTYPE a SYSTEM \"rules.xsd\">\n<a/>");
}

#[test]
fn enter_in_the_field_assigns() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.xml", "<a/>"));
    assert!(h.state_mut().execute("xml.assign_schema"));
    h.run_steps(3);
    h.get_by_label(FIELD).focus();
    h.run_steps(1);
    h.get_by_label(FIELD).type_text("a.dtd");
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(text(&h), "<!DOCTYPE a SYSTEM \"a.dtd\">\n<a/>");
}

#[test]
fn the_error_goes_when_the_location_is_changed() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.xml", "<a/>"));
    assert!(h.state_mut().execute("xml.assign_schema"));
    h.run_steps(3);
    h.get_by_label("Assign").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("Type where the schema").is_some());
    h.get_by_label(FIELD).focus();
    h.run_steps(1);
    h.get_by_label(FIELD).type_text("a");
    h.run_steps(2);
    assert!(h.query_by_label_contains("Type where the schema").is_none(), "it was about the empty location");
}

#[test]
fn a_document_that_is_gone_is_said_and_nothing_is_written() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.xml", "<a/>"));
    assert!(h.state_mut().execute("xml.assign_schema"));
    h.run_steps(3);
    h.get_by_label(FIELD).focus();
    h.run_steps(1);
    h.get_by_label(FIELD).type_text("a.dtd");
    h.run_steps(2);
    assert!(h.state_mut().execute("file.close"));
    h.run_steps(3);
    assert!(h.query_by_label("Assign Schema").is_some(), "the form stays, as the dialog is not the document's");
    h.get_by_label("Assign").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("is gone").is_some(), "it says why nothing was written");
    assert!(h.state().session.active_document().is_none());
}

#[test]
fn the_keyboard_is_in_the_field_when_the_form_opens() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.xml", "<a/>"));
    assert!(h.state_mut().execute("xml.assign_schema"));
    h.run_steps(3);
    assert!(h.get_by_label(FIELD).is_focused(), "typing goes into the location without a click");
}

#[test]
fn escape_closes_the_form_and_changes_nothing() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.xml", "<a/>"));
    assert!(h.state_mut().execute("xml.assign_schema"));
    h.run_steps(3);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.query_by_label("Assign Schema").is_none());
    assert_eq!(text(&h), "<a/>");
}

#[test]
fn white_space_around_the_location_is_not_part_of_it() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.xml", "<a/>"));
    assign_with(&mut h, "  a.dtd ");
    assert_eq!(text(&h), "<!DOCTYPE a SYSTEM \"a.dtd\">\n<a/>");
}

#[test]
fn choosing_a_file_takes_the_error_away() {
    let dir = tempfile::tempdir().expect("dir");
    let schema = file(dir.path(), "a.dtd", "<!ELEMENT a EMPTY>");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.xml", "<a/>"));
    assert!(h.state_mut().execute("xml.assign_schema"));
    h.run_steps(3);
    h.get_by_label("Assign").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("Type where the schema").is_some());
    h.state_mut().pick_override = Some(vec![schema]);
    h.get_by_label("Browse…").click();
    h.run_steps(4);
    assert!(h.query_by_label_contains("Type where the schema").is_none(), "the empty location is not empty any more");
    h.get_by_label("Assign").click();
    h.run_steps(3);
    assert_eq!(text(&h), "<!DOCTYPE a SYSTEM \"a.dtd\">\n<a/>");
}
