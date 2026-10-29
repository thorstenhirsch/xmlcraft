//! Entry Helpers in the real shell: the grammar of the DTD is read on a worker thread, so these tests wait for it.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::Modifiers;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_ui_egui::XmlCraftApp;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).with_step_dt(0.02).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

fn file(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path
}

fn wait(h: &mut Harness<'static, XmlCraftApp>, what: &str, done: impl Fn(&XmlCraftApp) -> bool) {
    let started = Instant::now();
    while !done(h.state()) {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for {what}");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(3);
}

/// Open a document and put the caret at `¦` of `marked`.
fn open_at(h: &mut Harness<'static, XmlCraftApp>, dir: &Path, marked: &str) {
    let caret = marked.find('¦').expect("a marker");
    let text = marked.replacen('¦', "", 1);
    // Each document is a file of its own: opening a path again would show the document that is open.
    let name = format!("doc{}.xml", h.state().session.documents().len());
    let path = file(dir, &name, &text);
    assert!(h.state_mut().open_path(&path));
    wait(h, "the grammar", |app| app.has_grammar());
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(caret, caret);
    h.run_steps(6);
}

fn text(h: &Harness<'static, XmlCraftApp>) -> String {
    h.state().session.active_document().expect("shown").editor().text()
}

/// A double click on the row `label`. Time passes first, so that an earlier click is not counted with these.
fn double_click(h: &mut Harness<'static, XmlCraftApp>, label: &str) {
    h.run_steps(30);
    let at = h.get_by_label(label).rect().center();
    h.hover_at(at);
    for pressed in [true, false, true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(3);
}

const DTD: &str = "<!DOCTYPE r [<!ELEMENT r (a, b?, s?)><!ELEMENT a EMPTY><!ATTLIST a id ID #REQUIRED kind (p|q) #IMPLIED><!ELEMENT b (#PCDATA)><!ELEMENT s (b)>\
                   <!ENTITY e 'text'>]>";

#[test]
fn the_elements_the_content_model_allows_here_are_listed_with_the_required_ones_marked() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r>¦</r>"));
    assert!(h.query_by_label("a (required)").is_some(), "a must come first");
    assert!(h.query_by_label("b").is_none(), "b may only come after a");
    // After a, b may come and nothing is required.
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1'/>¦</r>"));
    assert!(h.query_by_label("b").is_some() && h.query_by_label("a (required)").is_none());
}

#[test]
fn a_double_click_puts_an_element_with_what_it_requires_and_undo_takes_it_back() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    let marked = format!("{DTD}<r>¦</r>");
    open_at(&mut h, dir.path(), &marked);
    double_click(&mut h, "a (required)");
    assert_eq!(text(&h), format!("{DTD}<r><a id=\"p\"/></r>").replace("id=\"p\"", "id=\"\""));
    // The caret is in the value that has to be filled in.
    let doc = h.state().session.active_document().expect("shown");
    let caret = doc.editor().primary().head();
    assert_eq!(&doc.editor().text()[caret - 7..caret], "<a id=\"", "{}", doc.editor().text());
    // One undo takes it away.
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().undo();
    assert_eq!(text(&h), marked.replacen('¦', "", 1));
}

#[test]
fn the_option_decides_whether_the_children_a_declaration_requires_are_put_too() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1'/>¦</r>"));
    double_click(&mut h, "s");
    assert_eq!(text(&h), format!("{DTD}<r><a id='1'/><s><b></b></s></r>"), "the option is on at first");
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().undo();
    h.get_by_label("Insert required content").click();
    h.run_steps(3);
    double_click(&mut h, "s");
    assert_eq!(text(&h), format!("{DTD}<r><a id='1'/><s></s></r>"), "off: just the element");
}

#[test]
fn attributes_of_the_element_are_offered_and_added_to_its_start_tag() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1' ¦/></r>"));
    // An attribute that is given is greyed and does not go in twice.
    double_click(&mut h, "id (required)");
    assert!(!text(&h).contains("id='1' id"), "{}", text(&h));
    double_click(&mut h, "kind");
    assert_eq!(text(&h), format!("{DTD}<r><a id='1' kind=\"p\" /></r>"), "the first value of an enumeration is the start");
}

#[test]
fn an_entity_is_put_as_a_reference() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1'/><b>¦</b></r>"));
    double_click(&mut h, "&e;");
    assert_eq!(text(&h), format!("{DTD}<r><a id='1'/><b>&e;</b></r>"));
}

#[test]
fn a_document_without_a_doctype_says_so() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    let path = file(dir.path(), "plain.xml", "<r><a/></r>");
    assert!(h.state_mut().open_path(&path));
    h.run_steps(8);
    assert!(h.query_by_label("No DTD").is_some());
}

#[test]
fn a_doctype_too_big_to_read_says_so() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    let doctype = format!("<!DOCTYPE r [<!--{}--><!ELEMENT r ANY>]>", " ".repeat(xmlcraft_engine::schema::MAX_DOCTYPE_BYTES));
    let path = file(dir.path(), "big.xml", &format!("{doctype}<r/>"));
    assert!(h.state_mut().open_path(&path));
    let started = Instant::now();
    while h.query_by_label("No DTD to show").is_none() {
        assert!(started.elapsed() < Duration::from_secs(20), "waiting for the answer");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(h.query_by_label("Reading the DTD…").is_none());
}

fn edit(h: &mut Harness<'static, XmlCraftApp>, range: std::ops::Range<usize>, with: &str) {
    let doc = h.state_mut().session.active_document_mut().expect("shown");
    assert!(doc.editor_mut().apply_edits(&[xmlcraft_text::Edit::replace(range, with)]));
    h.run_steps(8);
}

#[test]
fn the_choices_follow_the_text_when_the_caret_stays_where_it_is() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1'/>¦</r>"));
    assert!(h.query_by_label("b").is_some() && h.query_by_label("a (required)").is_none());
    // The same number of bytes before the caret, and `a` is not there any more.
    let at = DTD.len() + "<r>".len();
    edit(&mut h, at..at + "<a id='1'/>".len(), "<!--1234-->");
    assert_eq!(h.state().session.active_document().expect("shown").editor().primary().head(), at + 11);
    assert!(h.query_by_label("a (required)").is_some() && h.query_by_label("b").is_none());
}

#[test]
fn what_is_selected_is_replaced_by_what_is_put() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><!--c-->¦</r>"));
    let caret = h.state().session.active_document().expect("shown").editor().primary().head();
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(caret - "<!--c-->".len(), caret);
    h.run_steps(6);
    double_click(&mut h, "a (required)");
    assert_eq!(text(&h), format!("{DTD}<r><a id=\"\"/></r>"));
}

#[test]
fn taking_the_doctype_away_takes_the_choices_away() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r>¦</r>"));
    assert!(h.query_by_label("a (required)").is_some());
    edit(&mut h, 0..DTD.len(), "");
    // The grammar goes when the worker has read the text again; on a busy machine that is not within the first frames.
    wait(&mut h, "the grammar to go", |app| !app.has_grammar());
    assert!(h.query_by_label("No DTD").is_some() && h.query_by_label("a (required)").is_none());
    assert!(!h.state().has_grammar());
}
