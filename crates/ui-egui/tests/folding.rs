//! Folding in the real shell: the commands (which wait for the analysis of the text as it is), the shortcuts, the fold marks in the
//! gutter, and what folded text does to the caret and to typing. The analysis runs on a worker thread, so these tests wait for it.

use std::time::{Duration, Instant};

use egui::{Event, Key, Modifiers, PointerButton};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_ui_egui::XmlCraftApp;

const DOC: &str = "<lib>\n  <book>\n    <t>x</t>\n    <u>y</u>\n  </book>\n  <book>\n    <t>z</t>\n  </book>\n</lib>\n";

fn shell_with(text: &str) -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h.state_mut().session.open(Some("/x/books.xml".into()), text.as_bytes(), None).expect("opens");
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

fn editor<'a>(h: &'a Harness<'static, XmlCraftApp>) -> &'a xmlcraft_editor::Editor {
    h.state().session.active_document().expect("a document").editor()
}

fn put_caret_on_line(h: &mut Harness<'static, XmlCraftApp>, line: usize) {
    let at = editor(h).buffer().line_start(line).unwrap_or(0) + 5;
    h.state_mut().session.active_document_mut().expect("a document").editor_mut().set_selection(at, at);
}

fn hidden(h: &Harness<'static, XmlCraftApp>) -> Vec<usize> {
    (0..editor(h).buffer().line_count()).filter(|l| editor(h).is_line_hidden(*l)).collect()
}

#[test]
fn the_fold_commands_fold_and_unfold_what_the_caret_is_in() {
    let mut h = shell_with(DOC);
    put_caret_on_line(&mut h, 2); // inside the first book
    assert!(h.state_mut().execute("edit.fold"));
    h.run_steps(2);
    assert_eq!(hidden(&h), [2, 3], "the first book's content: its end tag stays");
    assert!(h.state_mut().execute("edit.fold"));
    h.run_steps(2);
    assert_eq!(hidden(&h), [1, 2, 3, 4, 5, 6, 7], "the caret was on the folded line: the one around it folds");
    assert!(h.state_mut().execute("edit.unfold"));
    h.run_steps(2);
    assert_eq!(hidden(&h), [2, 3]);
    assert!(h.state_mut().execute("edit.unfold_all"));
    assert!(hidden(&h).is_empty());
    assert!(h.state_mut().execute("edit.fold_all"));
    assert_eq!(hidden(&h), [1, 2, 3, 4, 5, 6, 7]);
    assert!(h.state_mut().execute("edit.fold_level_2"));
    assert_eq!(hidden(&h), [2, 3, 6], "only the books");
    assert!(h.state_mut().execute("edit.unfold_all"));
    // Nothing to fold at the end of the text.
    h.state_mut().session.active_document_mut().expect("a document").editor_mut().set_selection(DOC.len(), DOC.len());
    h.state_mut().execute("edit.fold");
    assert_eq!(h.state().status, "Nothing to fold here");
}

#[test]
fn the_shortcuts_fold_and_unfold() {
    let mut h = shell_with(DOC);
    put_caret_on_line(&mut h, 2);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::OpenBracket);
    h.run_steps(3);
    assert_eq!(hidden(&h), [2, 3]);
    put_caret_on_line(&mut h, 1);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::ALT, Key::CloseBracket);
    h.run_steps(3);
    assert!(hidden(&h).is_empty());
}

#[test]
fn a_fold_command_given_while_the_analysis_is_old_waits_for_the_new_one() {
    let mut h = shell_with(DOC);
    // A line is added at the top: every line below moves, and the ranges of the analysis are about the old text.
    h.state_mut().session.active_document_mut().expect("a document").editor_mut().apply_edits(&[xmlcraft_text::Edit::insert(0, "<!-- x -->\n")]);
    put_caret_on_line(&mut h, 3); // inside the first book, now one line down
    h.state_mut().execute("edit.fold");
    wait(&mut h, "the fold", |app| app.session.active_document().is_some_and(|d| d.editor().visible_rows() < d.editor().buffer().line_count()));
    assert_eq!(hidden(&h), [3, 4], "the first book's content, where it is now");
    // A command that is overtaken by typing is dropped, not applied to lines that moved.
    h.state_mut().execute("edit.unfold_all");
    let editor_mut =
        |h: &mut Harness<'static, XmlCraftApp>| h.state_mut().session.active_document_mut().expect("a document").editor_mut().type_text("z");
    editor_mut(&mut h); // the analysis is about the text before this
    h.state_mut().execute("edit.fold"); // so it waits for the new one
    editor_mut(&mut h); // and the text changes again before that arrives
    // The analysis of the latest text has arrived when the caret, which is in a tag name, has its pair highlighted: by then the command
    // would have been done, were it not dropped.
    wait(&mut h, "the analysis of the latest text", |app| app.highlighted_tag_names().is_some());
    for _ in 0..5 {
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(hidden(&h).is_empty(), "the text changed before the analysis arrived");
}

#[test]
fn the_caret_moves_over_folded_lines_and_typing_below_them_works() {
    let mut h = shell_with(DOC);
    put_caret_on_line(&mut h, 2);
    h.state_mut().execute("edit.fold"); // lines 2 and 3 hidden; the caret is on line 1 now
    h.run_steps(2);
    h.get_by_label("XML editor").focus();
    h.run_steps(2);
    let line = |h: &Harness<'static, XmlCraftApp>| editor(h).buffer().line_of(editor(h).primary().head());
    assert_eq!(line(&h), 1);
    h.key_press(Key::ArrowDown);
    h.run_steps(2);
    assert_eq!(line(&h), 4, "down from the fold goes to the line after it");
    h.key_press(Key::ArrowUp);
    h.run_steps(2);
    assert_eq!(line(&h), 1);
    // Typing at the end of the line that stands for the fold keeps the fold.
    h.key_press(Key::End);
    h.state_mut().session.active_document_mut().expect("a document").editor_mut().type_text("!");
    h.run_steps(2);
    assert_eq!(hidden(&h), [2, 3]);
    // A find jump into it opens it.
    let at = editor(&h).text().find("<u>").expect("there");
    h.state_mut().session.active_document_mut().expect("a document").editor_mut().set_selection(at, at + 3);
    h.run_steps(2);
    assert!(hidden(&h).is_empty());
}

#[test]
fn the_gutter_mark_folds_and_unfolds_from_the_mouse() {
    let mut h = shell_with(DOC);
    let g = h.state().editor_geometry().expect("drawn");
    // The strip at the right of the gutter, on the row of line 1 (the first book).
    let mark = g.point(1, 0) + egui::vec2(-8.0 - 8.0, g.row_height / 2.0);
    let click = |h: &mut Harness<'static, XmlCraftApp>, at: egui::Pos2| {
        h.hover_at(at);
        h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.event(Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE });
        h.run_steps(3);
    };
    click(&mut h, mark);
    wait(&mut h, "the fold", |app| app.session.active_document().is_some_and(|d| d.editor().visible_rows() < d.editor().buffer().line_count()));
    assert_eq!(hidden(&h), [2, 3]);
    click(&mut h, mark);
    assert!(hidden(&h).is_empty(), "a mark on a fold opens it");
}
