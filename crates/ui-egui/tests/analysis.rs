//! Problems, the outline, F7 and Go to Line in the real shell: the analysis runs on a worker thread, so these tests wait for it.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::{Key, Modifiers};
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

#[test]
fn a_well_formed_document_says_so() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "ok.xml", "<a><b/></a>"));
    assert_eq!(h.state().problems(), Some((0, 0)));
    assert!(h.query_by_label("Well-formed").is_some(), "the status bar says so");
    assert!(h.query_by_label("No problems").is_some(), "and so does Messages");
}

#[test]
fn problems_are_listed_and_a_click_goes_to_them() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "bad.xml", "<a>\n  <b></c>\n</a>\n"));
    assert_eq!(h.state().problems(), Some((1, 0)));
    assert!(h.query_by_label("1 errors").is_some());
    let row = h.get_by_label("The end tag </c> does not match the start tag <b>. (Ln 2, Col 6)");
    // A pointer delivers the press and the release in frames of their own (egui drops the focus at a press that is not on the focused
    // widget; the click that follows is what hands the keyboard to the editor).
    let at = row.rect().center();
    h.hover_at(at);
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
    h.run_steps(3);
    let doc = h.state().session.active_document().expect("shown");
    let sel = doc.editor().primary().sel;
    assert_eq!(&doc.editor().text()[sel.start()..sel.end()], "</c", "the problem is selected");
    assert!(h.get_by_label("XML editor").is_focused(), "and the editor has the keyboard");
}

#[test]
fn the_analysis_follows_the_typing() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.xml", "<a></a>"));
    assert_eq!(h.state().problems(), Some((0, 0)));
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(3, 3);
    h.get_by_label("XML editor").focus();
    h.run_steps(2);
    h.get_by_label("XML editor").type_text("<b>");
    wait(&mut h, "the unclosed element to be noticed", |app| app.problems() == Some((1, 0)));
    h.get_by_label("XML editor").type_text("</b>");
    wait(&mut h, "the problem to go away", |app| app.problems() == Some((0, 0)));
}

#[test]
fn the_outline_lists_elements_filters_and_jumps() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "lib.xml", "<library>\n  <book id=\"b1\"/>\n  <magazine name=\"m\"/>\n  <book id=\"b2\"/>\n</library>\n"));
    for label in ["library", "book b1", "magazine m", "book b2"] {
        assert!(h.query_by_label(label).is_some(), "the outline lists {label}");
    }
    h.get_by_label("book b2").click();
    h.run_steps(3);
    let doc = h.state().session.active_document().expect("shown");
    let caret = doc.editor().primary().head();
    assert_eq!(&doc.editor().text()[caret..caret + 4], "book");
    assert_eq!(doc.editor().buffer().line_of(caret), 3);
    // The filter keeps what matches.
    h.get_by_label("Filter elements").focus();
    h.run_steps(2);
    h.get_by_label("Filter elements").type_text("maga");
    h.run_steps(3);
    assert!(h.query_by_label("magazine m").is_some() && h.query_by_label("book b1").is_none());
}

#[test]
fn f7_checks_now_and_opens_messages() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "bad.xml", "<a><b></a>"));
    assert!(h.state_mut().execute("view.messages"), "close Messages");
    h.run_steps(2);
    h.key_press(Key::F7);
    h.run_steps(3);
    assert!(h.state().bottom_open, "F7 shows the messages");
    assert!(h.query_by_label("bad.xml: 1 errors").is_some());
}

#[test]
fn go_to_line_moves_the_caret() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "lines.xml", "<a>\n  <b>\n    <c/>\n  </b>\n</a>\n"));
    h.key_press_modifiers(Modifiers::COMMAND, Key::G);
    h.run_steps(3);
    assert!(h.query_by_label("Go to Line").is_some());
    h.get_by_label("Line or line:column").type_text("3:5");
    h.run_steps(2);
    h.get_by_label("Go").click();
    h.run_steps(3);
    let doc = h.state().session.active_document().expect("shown");
    let pos = doc.editor().buffer().position(doc.editor().primary().head());
    assert_eq!((pos.line + 1, pos.col + 1), (3, 5));
    // Not a number: the dialog closes and the status says so.
    h.state_mut().execute("edit.go_to_line");
    h.run_steps(3);
    h.get_by_label("Line or line:column").type_text("abc");
    h.run_steps(2);
    h.get_by_label("Go").click();
    h.run_steps(3);
    assert!(h.query_by_label("'abc' is not a line number").is_some());
}
