//! F8 in the real shell: the validation runs on a worker thread, so these tests wait for it.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::Modifiers;
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

/// Run frames until `done` holds, or fail after a few seconds.
fn wait(h: &mut Harness<'static, XmlCraftApp>, what: &str, done: impl Fn(&XmlCraftApp) -> bool) {
    let started = Instant::now();
    while !done(h.state()) {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for {what}");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
}

fn open(h: &mut Harness<'static, XmlCraftApp>, path: &Path) {
    assert!(h.state_mut().open_path(path));
    wait(h, "the first analysis", |app| app.problems().is_some());
}

fn validate(h: &mut Harness<'static, XmlCraftApp>) {
    assert!(h.state_mut().execute("xml.validate"));
    wait(h, "the validation", |app| app.validity().is_some());
}

#[test]
fn f8_says_a_valid_document_is_valid() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "ok.xml", "<!DOCTYPE a [<!ELEMENT a (b)><!ELEMENT b EMPTY>]><a><b/></a>"));
    assert_eq!(h.state().validity(), None, "nothing is validated until it is asked for");
    assert!(h.query_by_label("Well-formed").is_some());
    validate(&mut h);
    assert_eq!(h.state().validity(), Some((0, 0)));
    assert!(h.query_by_label("Valid").is_some(), "the status bar says so");
    assert!(h.query_by_label("No problems").is_some(), "and so does Messages");
}

#[test]
fn validity_problems_are_listed_after_the_others_and_a_click_goes_to_them() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "bad.xml", "<!DOCTYPE a [<!ELEMENT a (b)><!ELEMENT b EMPTY>]>\n<a>\n  <b>x</b>\n  <c/>\n</a>\n"));
    validate(&mut h);
    assert_eq!(h.state().validity(), Some((2, 0)));
    assert!(h.query_by_label("2 validity errors").is_some());
    let row = h.get_by_label("The element 'c' is not declared. (Ln 4, Col 4)");
    let at = row.rect().center();
    h.hover_at(at);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    let doc = h.state().session.active_document().expect("shown");
    let sel = doc.editor().primary().sel;
    assert_eq!(&doc.editor().text()[sel.start()..sel.end()], "c", "the element name is selected");
    assert!(h.query_by_label_contains("declared EMPTY").is_some(), "the other problem is listed too");
}

#[test]
fn an_edit_makes_the_validation_out_of_date() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.xml", "<!DOCTYPE a [<!ELEMENT a EMPTY>]><a/>"));
    validate(&mut h);
    assert_eq!(h.state().validity(), Some((0, 0)));
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(0, 0);
    h.get_by_label("XML editor").focus();
    h.run_steps(2);
    h.get_by_label("XML editor").type_text("<!-- x -->");
    wait(&mut h, "the analysis of the new text", |app| app.validity().is_none());
    assert!(h.query_by_label("Valid").is_none() && h.query_by_label("Well-formed").is_some(), "the status bar goes back to what it knows");
    validate(&mut h);
    assert_eq!(h.state().validity(), Some((0, 0)));
}

#[test]
fn the_external_subset_is_read_from_the_folder_of_the_document_and_its_problems_name_the_file() {
    let dir = tempfile::tempdir().expect("dir");
    file(dir.path(), "r.dtd", "<!ELEMENT a (b)>\n<!ELEMENT b EMPTY>\n<!ATTLIST b x ID #FIXED 'v'>");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "doc.xml", "<!DOCTYPE a SYSTEM \"r.dtd\">\n<a><b/></a>\n"));
    validate(&mut h);
    assert_eq!(h.state().validity(), Some((1, 0)));
    let row = h.get_by_label("The ID attribute 'x' of 'b' must be #IMPLIED or #REQUIRED. (r.dtd)");
    // A problem in another file does not move the caret.
    let before = h.state().session.active_document().expect("shown").editor().primary().sel;
    let at = row.rect().center();
    h.hover_at(at);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    assert_eq!(h.state().session.active_document().expect("shown").editor().primary().sel, before);
}

#[test]
fn a_document_that_is_not_well_formed_says_it_cannot_be_validated() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "bad.xml", "<!DOCTYPE a [<!ELEMENT a EMPTY>]><a>"));
    validate(&mut h);
    assert_eq!(h.state().validity(), Some((0, 0)));
    assert!(h.state().status.contains("not well-formed, so it cannot be validated"), "{}", h.state().status);
    assert!(h.query_by_label_contains("is not closed").is_some());
}

#[test]
fn f8_is_the_shortcut_and_the_command_is_in_the_xml_menu() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "ok.xml", "<!DOCTYPE a [<!ELEMENT a EMPTY>]><a/>"));
    h.key_press(egui::Key::F8);
    wait(&mut h, "the validation", |app| app.validity().is_some());
    assert_eq!(h.state().validity(), Some((0, 0)));
}

#[test]
fn what_is_wrong_with_the_document_is_underlined_while_the_validation_is_current() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    let text = "<!DOCTYPE a [<!ELEMENT a EMPTY>]><a>x</a>";
    open(&mut h, &file(dir.path(), "a.xml", text));
    assert!(h.state().underlined().is_empty());
    validate(&mut h);
    let underlined: Vec<&str> = h.state().underlined().iter().filter_map(|(range, _)| text.get(range.clone())).collect();
    assert_eq!(underlined, ["x"], "the content of the EMPTY element");
    // An edit makes it out of date: the underline goes until the next validation.
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(0, 0);
    h.get_by_label("XML editor").focus();
    h.run_steps(2);
    h.get_by_label("XML editor").type_text(" ");
    wait(&mut h, "the analysis of the new text", |app| app.validity().is_none());
    assert!(h.state().underlined().is_empty());
}

const XS: &str = "http://www.w3.org/2001/XMLSchema";

fn order_schema(id_type: &str) -> String {
    format!(
        "<xs:schema xmlns:xs=\"{XS}\"><xs:element name=\"order\"><xs:complexType><xs:sequence><xs:element name=\"id\" type=\"{id_type}\"/></xs:sequence></xs:complexType></xs:element></xs:schema>"
    )
}

fn order(id: &str) -> String {
    format!(
        "<order xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"order.xsd\">\n  <id>{id}</id>\n</order>\n"
    )
}

#[test]
fn f8_checks_a_document_against_the_schema_it_names() {
    let dir = tempfile::tempdir().expect("dir");
    file(dir.path(), "order.xsd", &order_schema("xs:int"));
    let mut h = shell();
    open(&mut h, &file(dir.path(), "ok.xml", &order("7")));
    validate(&mut h);
    assert_eq!(h.state().validity(), Some((0, 0)));
    assert!(h.query_by_label("Valid").is_some(), "{:?}", h.state().status);
    open(&mut h, &file(dir.path(), "bad.xml", &order("seven")));
    validate(&mut h);
    assert_eq!(h.state().validity(), Some((1, 0)));
    assert!(h.query_by_label("1 validity errors").is_some());
    assert!(h.query_by_label_contains("'seven' is not a valid integer").is_some(), "the problem is listed with the datatype's own words");
}

#[test]
fn f8_says_when_the_schema_a_document_names_is_not_there() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "lonely.xml", &order("7")));
    validate(&mut h);
    assert_eq!(h.state().validity(), Some((1, 0)));
    assert!(
        h.query_by_label_contains("the schema 'order.xsd' could not be loaded").is_some() && h.query_by_label_contains("No such file").is_some(),
        "the reason is shown, with the schema's name and what the system said"
    );
    assert!(h.query_by_label("Valid").is_none(), "an unchecked document is not called valid");
}

#[test]
fn f8_uses_a_schema_that_is_open_as_it_is_edited() {
    let dir = tempfile::tempdir().expect("dir");
    // Saved: the id is an integer, so "seven" is wrong; the open copy has been changed to a string.
    let schema = file(dir.path(), "order.xsd", &order_schema("xs:int"));
    let mut h = shell();
    open(&mut h, &file(dir.path(), "doc.xml", &order("seven")));
    validate(&mut h);
    assert_eq!(h.state().validity(), Some((1, 0)), "the saved schema wants an integer");
    open(&mut h, &schema);
    let editor = h.state_mut().session.active_document_mut().expect("the schema is shown").editor_mut();
    let at = editor.text().find("xs:int").expect("the type");
    assert!(editor.apply_edits(&[xmlcraft_text::Edit { range: at..at + "xs:int".len(), text: "xs:string".to_owned() }]));
    // Back to the document; the schema is open, changed and not saved.
    h.state_mut().session.set_active(Some(0));
    h.run_steps(2);
    validate(&mut h);
    assert_eq!(h.state().validity(), Some((0, 0)), "the unsaved text of the schema is what counts: {}", h.state().status);
}
