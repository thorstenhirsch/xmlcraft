//! Expanding XIncludes in the real shell: one command, a minimal edit for each include, one undoable step.

use std::time::{Duration, Instant};

use egui_kittest::Harness;
use xmlcraft_ui_egui::XmlCraftApp;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

fn text(h: &Harness<'static, XmlCraftApp>) -> String {
    h.state().session.active_document().expect("a document").editor().text()
}

fn wait(h: &mut Harness<'static, XmlCraftApp>, what: &str, done: impl Fn(&XmlCraftApp) -> bool) {
    let started = Instant::now();
    while !done(h.state()) {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for {what}: {}", h.state().status);
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
}

#[test]
fn each_include_is_replaced_by_its_own_edit_in_the_documents_line_endings_and_undone_in_one_step() {
    let dir = tempfile::tempdir().expect("dir");
    std::fs::write(dir.path().join("b.xml"), "<b>\n  in\n</b>").expect("writes");
    std::fs::write(dir.path().join("c.xml"), "<c/>").expect("writes");
    let path = dir.path().join("a.xml");
    let original = "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\">\r\n  <xi:include href=\"b.xml\"/>\r\n  <keep/>\r\n  <xi:include href=\"c.xml\"/>\r\n</a>\r\n";
    std::fs::write(&path, original).expect("writes");
    let mut h = shell();
    h.state_mut().session.open(Some(path), original.as_bytes(), None).expect("opens");
    h.run_steps(3);
    // The caret sits in `<keep/>`, between the two includes: a minimal edit carries it along; a rewrite of the whole text would not.
    let keep = original.find("<keep/>").expect("keep") + 2;
    h.state_mut().session.active_document_mut().expect("a document").editor_mut().set_selection(keep, keep);
    assert!(h.state_mut().execute("xml.xinclude"));
    wait(&mut h, "the expansion", |app| app.status.starts_with("Expanded"));
    let expanded = "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\">\r\n  <b>\r\n  in\r\n</b>\r\n  <keep/>\r\n  <c/>\r\n</a>\r\n";
    assert_eq!(text(&h), expanded, "the included lines take the document's CRLF");
    let caret = h.state().session.active_document().expect("a document").editor().primary().head();
    assert_eq!(caret, expanded.find("<keep/>").expect("keep") + 2, "the caret stayed in <keep/>");
    assert!(h.state_mut().execute("edit.undo"));
    assert_eq!(text(&h), original, "one undo restores every byte");
}

#[test]
fn a_document_with_no_include_is_said_so_and_left_alone() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("a.xml");
    std::fs::write(&path, "<a/>").expect("writes");
    let mut h = shell();
    h.state_mut().session.open(Some(path), b"<a/>", None).expect("opens");
    h.run_steps(3);
    assert!(h.state_mut().execute("xml.xinclude"));
    assert_eq!(h.state().status, "There are no XIncludes to expand");
    assert_eq!(text(&h), "<a/>");
}
