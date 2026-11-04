//! The element the caret is in: rename (both tags), wrap, unwrap, select and copy its path, in the real shell.

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

/// Open `text` and wait until it has been read, so that the commands know its elements.
fn open(h: &mut Harness<'static, XmlCraftApp>, dir: &Path, text: &str) {
    assert!(h.state_mut().open_path(&file(dir, "doc.xml", text)));
    let started = Instant::now();
    while h.state().problems().is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the first analysis");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
}

fn text(h: &Harness<'static, XmlCraftApp>) -> String {
    h.state().session.active_document().expect("shown").editor().text()
}

fn selection(h: &Harness<'static, XmlCraftApp>) -> (usize, usize) {
    let s = h.state().session.active_document().expect("shown").editor().primary();
    (s.start(), s.end())
}

fn put_caret(h: &mut Harness<'static, XmlCraftApp>, offset: usize) {
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().place_caret(offset, false);
    h.run_steps(2);
}

const DOC: &str = "<a><b>x</b><c/></a>";

#[test]
fn rename_element_renames_both_tags_in_one_undoable_step() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, dir.path(), DOC);
    put_caret(&mut h, 6); // inside <b>
    assert!(h.state_mut().execute("xml.rename_element"));
    h.run_steps(3);
    h.get_by_label("The new name of the element").type_text("item");
    h.run_steps(2);
    h.get_by_label("Rename").click();
    h.run_steps(3);
    assert_eq!(text(&h), "<a><item>x</item><c/></a>");
    assert!(h.state_mut().execute("edit.undo"));
    assert_eq!(text(&h), DOC, "one step undoes both tags");
}

#[test]
fn rename_element_refuses_a_name_that_is_not_one_and_leaves_the_text() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, dir.path(), DOC);
    put_caret(&mut h, 6);
    assert!(h.state_mut().execute("xml.rename_element"));
    h.run_steps(3);
    h.get_by_label("The new name of the element").type_text("1 bad");
    h.run_steps(2);
    h.get_by_label("Rename").click();
    h.run_steps(3);
    assert_eq!(text(&h), DOC);
    assert!(h.query_by_label_contains("not an element name").is_some(), "the reason is shown in the dialog");
}

#[test]
fn wrap_in_tag_wraps_the_selection_and_links_the_names() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, dir.path(), DOC);
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(6, 7); // the x
    assert!(h.state_mut().execute("xml.wrap_in_tag"));
    h.run_steps(2);
    assert_eq!(text(&h), "<a><b><element>x</element></b><c/></a>");
    assert_eq!(selection(&h), (7, 14), "the new name is selected");
    // After a frame has been drawn the link is still there: typing the name renames both tags.
    h.run_steps(3);
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().type_input("item");
    assert_eq!(text(&h), "<a><b><item>x</item></b><c/></a>");
    assert!(h.state_mut().execute("edit.undo"));
    assert!(h.state_mut().execute("edit.undo"));
    assert_eq!(text(&h), DOC);
}

#[test]
fn unwrap_element_keeps_what_is_inside() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, dir.path(), DOC);
    put_caret(&mut h, 6);
    assert!(h.state_mut().execute("xml.unwrap_element"));
    h.run_steps(2);
    assert_eq!(text(&h), "<a>x<c/></a>");
}

#[test]
fn select_element_selects_it_and_again_the_one_around_it() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, dir.path(), DOC);
    put_caret(&mut h, 6);
    assert!(h.state_mut().execute("xml.select_element"));
    h.run_steps(2);
    assert_eq!(selection(&h), (3, 11), "<b>x</b>");
    assert!(h.state_mut().execute("xml.select_element"));
    h.run_steps(2);
    assert_eq!(selection(&h), (0, DOC.len()), "then <a>…</a>");
}

#[test]
fn copy_xpath_says_which_path_was_copied() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, dir.path(), DOC);
    put_caret(&mut h, 6);
    assert!(h.state_mut().execute("xml.copy_xpath"));
    h.run_steps(2);
    assert!(h.state().status.contains("/a/b"), "{}", h.state().status);
}

#[test]
fn the_commands_say_so_when_the_caret_is_in_no_element() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, dir.path(), "<?xml version=\"1.0\"?>\n<a/>\n");
    put_caret(&mut h, 2); // inside the declaration
    assert!(h.state_mut().execute("xml.unwrap_element"));
    h.run_steps(2);
    assert_eq!(text(&h), "<?xml version=\"1.0\"?>\n<a/>\n");
    assert!(h.state().status.contains("not in an element"), "{}", h.state().status);
}

#[test]
fn rename_element_does_not_replace_a_question_that_is_being_asked() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, dir.path(), DOC);
    put_caret(&mut h, 6);
    assert!(h.state_mut().execute("edit.go_to_line"));
    h.run_steps(3);
    assert!(h.state_mut().execute("xml.rename_element"));
    h.run_steps(3);
    assert!(h.query_by_label("The new name of the element").is_none(), "the question that was asked is still the one on the screen");
}
