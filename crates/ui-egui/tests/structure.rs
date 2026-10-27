//! Commenting and growing the selection in the real shell: the commands and their shortcuts.

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use xmlcraft_ui_egui::XmlCraftApp;

fn shell_with(text: &str) -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h.state_mut().session.open(Some("/x/s.xml".into()), text.as_bytes(), None).expect("opens");
    h.run_steps(4);
    h
}

fn text(h: &Harness<'static, XmlCraftApp>) -> String {
    h.state().session.active_document().expect("a document").editor().text()
}

fn selection(h: &Harness<'static, XmlCraftApp>) -> (usize, usize) {
    let c = h.state().session.active_document().expect("a document").editor().primary();
    (c.start(), c.end())
}

fn put_caret(h: &mut Harness<'static, XmlCraftApp>, at: usize) {
    h.state_mut().session.active_document_mut().expect("a document").editor_mut().set_selection(at, at);
}

#[test]
fn control_slash_comments_the_line_out_and_in_and_says_when_it_cannot() {
    let mut h = shell_with("<a>\n  <b/>\n  <!-- c -->\n</a>");
    put_caret(&mut h, 6);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Slash);
    h.run_steps(2);
    assert_eq!(text(&h), "<a>\n  <!-- <b/> -->\n  <!-- c -->\n</a>");
    h.key_press_modifiers(Modifiers::COMMAND, Key::Slash);
    h.run_steps(2);
    assert_eq!(text(&h), "<a>\n  <b/>\n  <!-- c -->\n</a>");
    // A line that is a comment already is brought back; one with a comment and more is refused, and the text stays.
    let end_of_comment = text(&h).find("-->").expect("there") + 3;
    put_caret(&mut h, end_of_comment);
    h.state_mut().session.active_document_mut().expect("a document").editor_mut().type_text(" <d/>");
    assert!(text(&h).contains("<!-- c --> <d/>"), "the comment is whole, with more after it: {}", text(&h));
    let before = text(&h);
    h.key_press_modifiers(Modifiers::COMMAND, Key::Slash);
    h.run_steps(2);
    assert_eq!(text(&h), before);
    assert!(h.state().status.starts_with("Can't comment that out"), "{}", h.state().status);
}

#[test]
fn the_selection_grows_and_shrinks_with_the_shortcuts() {
    let mut h = shell_with("<a><b c=\"value\">x</b></a>");
    put_caret(&mut h, 11); // in `value`
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::ArrowRight);
    h.run_steps(2);
    assert_eq!(selection(&h), (9, 14), "the word");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::ArrowRight);
    h.run_steps(2);
    assert_eq!(selection(&h), (8, 15), "the value with its quotes");
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::ArrowLeft);
    h.run_steps(2);
    assert_eq!(selection(&h), (9, 14));
    // At the whole text there is nothing bigger.
    h.state_mut().session.active_document_mut().expect("a document").editor_mut().select_all();
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::ArrowRight);
    h.run_steps(2);
    assert_eq!(h.state().status, "Nothing bigger to select here");
}
