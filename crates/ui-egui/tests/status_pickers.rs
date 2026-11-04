//! The status bar's encoding, line ending and indentation menus, in the real shell.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_ui_egui::XmlCraftApp;

fn shell_with(text: &[u8]) -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h.state_mut().session.open(Some("/x/s.xml".into()), text, None).expect("opens");
    h.run_steps(4);
    h
}

fn text(h: &Harness<'static, XmlCraftApp>) -> String {
    h.state().session.active_document().expect("a document").editor().text()
}

#[test]
fn the_line_ending_menu_converts_the_file_in_one_undoable_step() {
    let mut h = shell_with(b"<a>\n  <b/>\n</a>");
    h.get_by_label("LF").click();
    h.run_steps(3);
    h.get_by_label("Convert to CRLF").click();
    h.run_steps(3);
    assert_eq!(text(&h), "<a>\r\n  <b/>\r\n</a>");
    assert!(h.query_by_label("CRLF").is_some(), "the status bar says so");
    assert!(h.state().status.contains("Converted 2 line breaks to CRLF"), "{}", h.state().status);
    assert!(h.state_mut().execute("edit.undo"));
    assert_eq!(text(&h), "<a>\n  <b/>\n</a>");
}

#[test]
fn the_indentation_menu_changes_how_new_lines_are_indented_and_not_the_text() {
    let mut h = shell_with(b"<a>\n  <b/>\n</a>");
    let before = text(&h);
    h.get_by_label("Spaces: 2").click();
    h.run_steps(3);
    h.get_by_label("Tabs").click();
    h.run_steps(3);
    assert_eq!(text(&h), before, "the text is as it was");
    let settings = h.state().session.active_document().expect("a document").editor().settings().clone();
    assert!(settings.use_tabs);
    assert!(h.query_by_label("Tabs").is_some());
}

#[test]
fn the_encoding_menu_changes_how_the_file_is_written() {
    let mut h = shell_with(b"<a/>");
    h.get_by_label("UTF-8").click();
    h.run_steps(3);
    h.get_by_label("UTF-8 BOM").click();
    h.run_steps(3);
    let doc = h.state().session.active_document().expect("a document");
    assert!(doc.encoding().bom, "a byte-order mark is written");
    assert!(h.query_by_label("UTF-8 BOM").is_some());
    assert_eq!(text(&h), "<a/>", "the text is not touched by a mark");
}
