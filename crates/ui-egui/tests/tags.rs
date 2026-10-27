//! Matching tags in the real shell: ⇧⌘\ goes to the other tag of the element the caret is at. The analysis runs on a worker thread, so
//! these tests wait for it.

use std::time::{Duration, Instant};

use egui_kittest::Harness;
use xmlcraft_ui_egui::XmlCraftApp;

fn shell_with(text: &str) -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h.state_mut().session.open(Some("/x/tags.xml".into()), text.as_bytes(), None).expect("opens");
    wait(&mut h, "the first analysis", |app| app.problems().is_some());
    h
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

fn caret(h: &Harness<'static, XmlCraftApp>) -> usize {
    h.state().session.active_document().expect("a document").editor().primary().head()
}

fn put_caret(h: &mut Harness<'static, XmlCraftApp>, at: usize) {
    h.state_mut().session.active_document_mut().expect("a document").editor_mut().set_selection(at, at);
}

const TEXT: &str = "<lib>\n  <book id=\"1\">text</book>\n  <br/>\n</lib>\n";

#[test]
fn the_command_goes_to_the_other_tag_and_back() {
    let mut h = shell_with(TEXT);
    let (open, close) = (TEXT.find("book id").expect("there"), TEXT.find("</book>").expect("there") + 2);
    // From the second letter of the name to the second letter of the other name, and back.
    put_caret(&mut h, open + 1);
    assert!(h.state_mut().execute("edit.go_to_matching_tag"));
    h.run_steps(2);
    assert_eq!(caret(&h), close + 1);
    assert!(h.state_mut().execute("edit.go_to_matching_tag"));
    h.run_steps(2);
    assert_eq!(caret(&h), open + 1);
    // From an attribute: the start of the other name.
    put_caret(&mut h, TEXT.find("\"1\"").expect("there"));
    h.state_mut().execute("edit.go_to_matching_tag");
    h.run_steps(2);
    assert_eq!(caret(&h), close);
}

#[test]
fn the_names_of_the_pair_the_caret_is_at_are_highlighted() {
    let mut h = shell_with(TEXT);
    let root_end = TEXT.rfind("lib").expect("there");
    assert_eq!(h.state().highlighted_tag_names(), Some([1..4, root_end..root_end + 3]), "the caret is just before `<lib>`: next to its tag");
    let (open, close) = (TEXT.find("book id").expect("there"), TEXT.find("</book>").expect("there") + 2);
    put_caret(&mut h, open + 6); // in the attribute
    h.run_steps(3);
    assert_eq!(h.state().highlighted_tag_names(), Some([open..open + 4, close..close + 4]));
    put_caret(&mut h, close + 1);
    h.run_steps(3);
    assert_eq!(h.state().highlighted_tag_names(), Some([close..close + 4, open..open + 4]), "from the end tag, the same two");
    put_caret(&mut h, TEXT.find("text").expect("there") + 1);
    h.run_steps(3);
    assert_eq!(h.state().highlighted_tag_names(), None, "in text there is none");
    // Not while a selection is made, nor with several carets.
    put_caret(&mut h, open);
    h.state_mut().session.active_document_mut().expect("a document").editor_mut().set_selection(open, open + 4);
    h.run_steps(3);
    assert_eq!(h.state().highlighted_tag_names(), None);
    // Typing makes the analysis older than the text: nothing is highlighted in the wrong place until the new one is here.
    put_caret(&mut h, open + 1);
    h.run_steps(3);
    assert!(h.state().highlighted_tag_names().is_some());
    h.state_mut().session.active_document_mut().expect("a document").editor_mut().type_text("z");
    h.run_steps(1);
    assert_eq!(h.state().highlighted_tag_names(), None, "stale");
    wait(&mut h, "the new analysis", |app| app.highlighted_tag_names().is_some());
    // The letter went into the end tag's name too (the names are linked): the pair is five letters long on both sides.
    assert_eq!(h.state().highlighted_tag_names(), Some([open..open + 5, close + 1..close + 6]));
}

#[test]
fn where_there_is_no_pair_it_says_so_and_stays() {
    let mut h = shell_with(TEXT);
    for (label, at) in [("an empty-element tag", TEXT.find("<br/>").expect("there") + 2), ("text", TEXT.find("text").expect("there") + 2)] {
        put_caret(&mut h, at);
        h.state_mut().execute("edit.go_to_matching_tag");
        h.run_steps(2);
        assert_eq!(caret(&h), at, "{label}: the caret stays");
        assert_eq!(h.state().status, "No matching tag here", "{label}");
    }
}

#[test]
fn a_jump_asked_for_before_the_analysis_is_ready_waits_for_it() {
    let mut h = shell_with(TEXT);
    // Typing makes the analysis stale; the command is given at once, without a frame in between.
    put_caret(&mut h, TEXT.find("book id").expect("there") + 1);
    h.state_mut().session.active_document_mut().expect("a document").editor_mut().type_text("x");
    let typed = h.state().session.active_document().expect("a document").editor().text();
    let (open, close) = (typed.find("bxook").expect("there") + 2, typed.find("</book>").expect("there"));
    assert_eq!(caret(&h), open, "the caret is after the typed letter, in the start tag's name");
    h.state_mut().execute("edit.go_to_matching_tag");
    wait(&mut h, "the jump", |app| app.session.active_document().is_some_and(|d| d.editor().primary().head() != open));
    // The end tag still says `book`: they are paired whatever the names say, and the caret is at the same column of the other name.
    assert_eq!(caret(&h), close + 2 + 2);
    // A jump that was waiting is dropped when the text changes again before the analysis arrives: the analysis is of the text before
    // the first of these two edits, so the command waits, and the second edit overtakes it.
    h.state_mut().session.active_document_mut().expect("a document").editor_mut().type_text("y");
    put_caret(&mut h, open);
    h.state_mut().execute("edit.go_to_matching_tag");
    h.state_mut().session.active_document_mut().expect("a document").editor_mut().type_text("z");
    let after_typing = caret(&h);
    for _ in 0..30 {
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(caret(&h), after_typing, "the caret is where typing left it");
    assert!(h.state().status.is_empty(), "and nothing was said: {}", h.state().status);
}

#[test]
fn a_jump_that_is_waiting_is_dropped_when_another_tab_is_shown() {
    let mut h = shell_with(TEXT);
    let first = h.state().session.active().expect("a document");
    // The analysis is of the text before this edit: the command waits for a new one.
    put_caret(&mut h, TEXT.find("book id").expect("there") + 1);
    h.state_mut().session.active_document_mut().expect("a document").editor_mut().type_text("x");
    let before = caret(&h);
    h.state_mut().execute("edit.go_to_matching_tag");
    // Before it arrives, another document is shown, with its caret somewhere.
    h.state_mut().session.open(Some("/x/other.xml".into()), b"<x><y>text</y></x>", None).expect("opens");
    put_caret(&mut h, 4);
    for _ in 0..40 {
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(caret(&h), 4, "the jump was not made in the document that is shown");
    h.state_mut().session.set_active(Some(first));
    h.run_steps(2);
    assert_eq!(caret(&h), before, "nor in the one that asked: it was dropped");
}

/// Run `f` on the editor of the document being shown.
fn with_editor<R>(h: &mut Harness<'static, XmlCraftApp>, f: impl FnOnce(&mut xmlcraft_editor::Editor) -> R) -> R {
    f(h.state_mut().session.active_document_mut().expect("a document").editor_mut())
}

#[test]
fn typing_in_a_tag_name_renames_the_other_tag_of_the_element() {
    let mut h = shell_with(TEXT);
    let open = TEXT.find("book id").expect("there");
    assert!(with_editor(&mut h, |e| e.linked_names().is_none()), "the caret is at the start of the text: no name");
    // The caret in the start tag's name: a frame links the pair from the analysis.
    put_caret(&mut h, open + 4);
    h.run_steps(3);
    assert!(with_editor(&mut h, |e| e.linked_names().is_some()));
    with_editor(&mut h, |e| e.type_input("s"));
    assert_eq!(with_editor(&mut h, |e| e.text()), TEXT.replace("book id", "books id").replace("</book>", "</books>"));
    // Typing again at once, before the analysis of the new text is here: the link follows the text.
    with_editor(&mut h, |e| e.type_input("t"));
    assert_eq!(with_editor(&mut h, |e| e.text()), TEXT.replace("book id", "bookst id").replace("</book>", "</bookst>"));
    with_editor(&mut h, |e| (e.backspace(), e.backspace()));
    assert_eq!(with_editor(&mut h, |e| e.text()), TEXT);
    // One undo step for all the typing and one for all the deleting, each holding both tags.
    with_editor(&mut h, |e| e.undo());
    assert_eq!(with_editor(&mut h, |e| e.text()), TEXT.replace("book id", "bookst id").replace("</book>", "</bookst>"));
    // The caret leaves the names: after a frame nothing is linked.
    put_caret(&mut h, TEXT.find("text").expect("there") + 1);
    h.run_steps(3);
    assert!(with_editor(&mut h, |e| e.linked_names().is_none()));
}
