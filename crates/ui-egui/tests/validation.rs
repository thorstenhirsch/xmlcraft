//! F8 in the real shell: the validation runs on a worker thread, so these tests wait for it.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::Modifiers;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_ui_egui::XmlCraftApp;

/// A shell in which nothing is validated unless it is asked for (F8): a document is not left alone for an hour.
fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = shell_validating_after(Duration::from_secs(3600));
    h.state_mut().set_validate_delay(Duration::from_secs(3600));
    h
}

/// A shell in which a document that names a grammar is validated when it has been left alone for `delay`.
fn shell_validating_after(delay: Duration) -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.state_mut().set_validate_delay(delay);
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
    // What the system says is its own wording ("No such file or directory", "The system cannot find the file specified"), after the path.
    assert!(
        // The reason is in the Messages pane and, as the document names a schema, in Entry Helpers too.
        h.query_all_by_label_contains("the schema 'order.xsd' could not be loaded").count() >= 1
            && h.query_all_by_label_contains("order.xsd: ").count() >= 1,
        "the reason is shown, with the schema's name and the path that was tried"
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

// --- while typing ------------------------------------------------------------------------------------------------------------------

const BAD: &str = "<!DOCTYPE a [<!ELEMENT a (b)><!ELEMENT b EMPTY>]>\n<a>\n  <b>x</b>\n</a>\n";

/// Run frames for `time`, as a person who is not touching anything.
fn idle(h: &mut Harness<'static, XmlCraftApp>, time: Duration) {
    let started = Instant::now();
    while started.elapsed() < time {
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_document_that_names_a_grammar_is_validated_when_it_is_left_alone_without_a_word_in_the_status_bar() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell_validating_after(Duration::from_millis(150));
    open(&mut h, &file(dir.path(), "bad.xml", BAD));
    let before = h.state().status.clone();
    wait(&mut h, "the validation while typing", |app| app.validity().is_some());
    assert_eq!(h.state().validity(), Some((1, 0)), "F8 was never pressed");
    assert_eq!(h.state().status, before, "no word about it in the status bar");
    assert!(h.query_by_label("1 validity errors").is_some(), "the badge says so");
    assert!(!h.state().underlined().is_empty(), "the problem is underlined");
    assert_eq!(h.state().validations_requested(), 1);
}

#[test]
fn what_is_typed_is_validated_after_a_pause_and_not_after_each_key() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell_validating_after(Duration::from_millis(400));
    open(&mut h, &file(dir.path(), "bad.xml", BAD));
    wait(&mut h, "the first validation", |app| app.validity().is_some());
    assert_eq!(h.state().validations_requested(), 1);
    // The error is mended with a few edits, each before the pause is over: one validation follows them all.
    let at = text_of(&h).find("x</b>").expect("the text");
    for step in 0..5 {
        let doc = h.state_mut().session.active_document_mut().expect("shown");
        let edit = if step == 0 { xmlcraft_text::Edit::delete(at..at + 1) } else { xmlcraft_text::Edit::insert(0, " ") };
        assert!(doc.editor_mut().apply_edits(&[edit]));
        idle(&mut h, Duration::from_millis(60));
    }
    assert_eq!(h.state().validations_requested(), 1, "nothing was asked for while the typing went on");
    wait(&mut h, "the validation after the pause", |app| app.validity() == Some((0, 0)));
    assert_eq!(h.state().validations_requested(), 2, "one validation for all the edits");
    // And it is not validated again for the text that was validated.
    idle(&mut h, Duration::from_millis(600));
    assert_eq!(h.state().validations_requested(), 2);
}

fn text_of(h: &Harness<'static, XmlCraftApp>) -> String {
    h.state().session.active_document().expect("shown").editor().text()
}

#[test]
fn a_document_with_no_grammar_or_that_is_not_well_formed_is_left_alone() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell_validating_after(Duration::from_millis(50));
    open(&mut h, &file(dir.path(), "plain.xml", "<a><b/></a>"));
    idle(&mut h, Duration::from_millis(400));
    assert_eq!((h.state().validity(), h.state().validations_requested()), (None, 0), "it names no grammar");
    open(&mut h, &file(dir.path(), "broken.xml", "<!DOCTYPE a [<!ELEMENT a EMPTY>]><a>"));
    idle(&mut h, Duration::from_millis(400));
    assert_eq!((h.state().validity(), h.state().validations_requested()), (None, 0), "it is not well-formed: that is what Messages says");
}

#[test]
fn validating_while_typing_can_be_turned_off_and_f8_still_works() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell_validating_after(Duration::from_millis(50));
    assert_eq!(h.state().command_checked("xml.validate_typing"), Some(true), "it is on unless it is turned off");
    assert!(h.state_mut().execute("xml.validate_typing"));
    assert_eq!(h.state().command_checked("xml.validate_typing"), Some(false));
    open(&mut h, &file(dir.path(), "bad.xml", BAD));
    idle(&mut h, Duration::from_millis(500));
    assert_eq!((h.state().validity(), h.state().validations_requested()), (None, 0));
    validate(&mut h);
    assert_eq!(h.state().validity(), Some((1, 0)), "F8 does as it did");
    assert_eq!(h.state().validations_requested(), 1);
    // Turned on again, an edit is validated after the pause.
    assert!(h.state_mut().execute("xml.validate_typing"));
    let doc = h.state_mut().session.active_document_mut().expect("shown");
    let at = doc.editor().text().find("x</b>").expect("the text");
    assert!(doc.editor_mut().apply_edits(&[xmlcraft_text::Edit::delete(at..at + 1)]));
    wait(&mut h, "the validation after the edit", |app| app.validity() == Some((0, 0)));
    assert_eq!(h.state().validations_requested(), 2);
}

#[test]
fn a_schema_the_document_names_is_validated_against_while_typing_too() {
    let dir = tempfile::tempdir().expect("dir");
    file(dir.path(), "n.xsd", "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"><xs:element name=\"n\" type=\"xs:int\"/></xs:schema>");
    let mut h = shell_validating_after(Duration::from_millis(150));
    let doc = "<n xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"n.xsd\">seven</n>";
    open(&mut h, &file(dir.path(), "n.xml", doc));
    wait(&mut h, "the validation against the schema", |app| app.validity().is_some());
    assert_eq!(h.state().validity(), Some((1, 0)));
    assert_eq!(h.state().validations_requested(), 1);
}

#[test]
fn f8_is_not_followed_by_a_second_validation_of_the_text_it_validated() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell_validating_after(Duration::from_millis(300));
    open(&mut h, &file(dir.path(), "bad.xml", BAD));
    validate(&mut h);
    assert_eq!(h.state().validations_requested(), 1);
    idle(&mut h, Duration::from_millis(800));
    assert_eq!(h.state().validations_requested(), 1, "the quiet period is no reason to do again what F8 did");
}

#[test]
fn a_validation_is_asked_for_once_though_frames_go_by_before_the_answer() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell_validating_after(Duration::from_millis(100));
    open(&mut h, &file(dir.path(), "bad.xml", BAD));
    let started = Instant::now();
    while h.state().validations_requested() == 0 {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the request");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(2));
    }
    // Frames, one after the other, before a worker is likely to have answered: the same text is not asked for again.
    h.run_steps(12);
    assert_eq!(h.state().validations_requested(), 1);
    wait(&mut h, "the answer", |app| app.validity().is_some());
    assert_eq!(h.state().validations_requested(), 1);
}

#[test]
fn a_schema_that_is_open_and_edited_is_what_the_validation_while_typing_uses() {
    let dir = tempfile::tempdir().expect("dir");
    let xsd =
        file(dir.path(), "n.xsd", "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"><xs:element name=\"n\" type=\"xs:string\"/></xs:schema>");
    let mut h = shell_validating_after(Duration::from_millis(150));
    // The schema is open, and its type changed in the editor, not saved: seven is not an int.
    assert!(h.state_mut().open_path(&xsd));
    h.run_steps(4);
    let doc = h.state_mut().session.active_document_mut().expect("shown");
    let text = doc.editor().text();
    let at = text.find("xs:string").expect("the type");
    assert!(doc.editor_mut().apply_edits(&[xmlcraft_text::Edit::replace(at..at + "xs:string".len(), "xs:int")]));
    let instance = "<n xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"n.xsd\">seven</n>";
    open(&mut h, &file(dir.path(), "n.xml", instance));
    wait(&mut h, "the validation", |app| app.validity().is_some());
    assert_eq!(h.state().validity(), Some((1, 0)), "valid against the file on disk, invalid against the schema as it is being edited");
}

const XHTML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd\">\n\
    <html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"en\" lang=\"en\"><head><title>T</title></head><body><p>Text.</p></body></html>\n";

#[test]
fn f8_checks_a_page_against_the_xhtml_dtd_that_ships_with_the_app() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "page.xhtml", XHTML));
    validate(&mut h);
    assert_eq!(h.state().validity(), Some((0, 0)));
    assert!(h.query_by_label("No problems").is_some(), "and not even a note that the DTD was not read");
    // A broken page is found by the same DTD.
    open(&mut h, &file(dir.path(), "bad.xhtml", &XHTML.replace("<p>Text.</p>", "Text.")));
    validate(&mut h);
    assert!(h.state().validity().is_some_and(|(errors, _)| errors >= 1), "{:?}", h.state().validity());
}

#[test]
fn f8_uses_a_dtd_that_is_open_as_it_is_edited() {
    let dir = tempfile::tempdir().expect("dir");
    let dtd = file(dir.path(), "r.dtd", "<!ELEMENT r EMPTY>");
    let mut h = shell();
    // The DTD is open, and changed in the editor, not saved: `r` now needs a child.
    assert!(h.state_mut().open_path(&dtd));
    h.run_steps(4);
    let doc = h.state_mut().session.active_document_mut().expect("shown");
    let len = doc.editor().text().len();
    assert!(doc.editor_mut().apply_edits(&[xmlcraft_text::Edit::replace(0..len, "<!ELEMENT r (a)><!ELEMENT a EMPTY>")]));
    open(&mut h, &file(dir.path(), "doc.xml", "<!DOCTYPE r SYSTEM \"r.dtd\"><r/>"));
    validate(&mut h);
    assert_eq!(h.state().validity(), Some((1, 0)), "valid against the file on disk, invalid against the DTD as it is being edited");
}
