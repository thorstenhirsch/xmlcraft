//! Find and replace in the real shell: the bar, the counter, stepping through matches, replacing, and the extra carets of ⌘D.

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_ui_egui::XmlCraftApp;

fn shell_with(text: &str) -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h.state_mut().session.open(Some("/x/books.xml".into()), text.as_bytes(), None).expect("opens");
    h.run_steps(4);
    h
}

fn text(h: &Harness<'static, XmlCraftApp>) -> String {
    h.state().session.active_document().expect("a document").editor().text()
}

fn selections(h: &Harness<'static, XmlCraftApp>) -> Vec<(usize, usize)> {
    h.state().session.active_document().expect("a document").editor().carets().iter().map(|c| (c.start(), c.end())).collect()
}

const BOOKS: &str = "<books>\n  <book id=\"1\"/>\n  <Book id=\"2\"/>\n  <magazine/>\n  <book id=\"3\"/>\n</books>\n";

#[test]
fn the_find_bar_finds_counts_and_steps_through_matches() {
    let mut h = shell_with(BOOKS);
    assert!(h.query_by_label("Find").is_none(), "no bar until it is asked for");
    h.state_mut().execute("edit.find");
    h.run_steps(3);
    h.get_by_label("Find").type_text("book");
    h.run_steps(3);
    // Not case-sensitive by default: `books` (twice: the root's tags), `book`, `Book` and `book`.
    assert!(h.query_by_label("1 of 5").is_some(), "a counter that says where it is");
    let first = selections(&h);
    assert_eq!(first, [(1, 5)], "the first match at the caret is selected as it is typed");
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(
        selections(&h),
        [(BOOKS.find("<book id=\"1\"").expect("there") + 1, BOOKS.find("<book id=\"1\"").expect("there") + 5)],
        "Enter goes to the next match"
    );
    assert!(h.query_by_label("2 of 5").is_some());
    h.key_press_modifiers(Modifiers::SHIFT, Key::Enter);
    h.run_steps(3);
    assert_eq!(selections(&h), first, "Shift+Enter comes back");
    assert!(h.query_by_label("1 of 5").is_some());
    // Escape closes the bar and gives the text the keyboard.
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(h.query_by_label("Find").is_none());
}

#[test]
fn the_options_change_what_matches() {
    let mut h = shell_with(BOOKS);
    h.state_mut().execute("edit.find");
    h.run_steps(3);
    h.get_by_label("Find").type_text("book");
    h.run_steps(3);
    assert!(h.query_by_label("2 of 5").is_some() || h.query_by_label("1 of 5").is_some() || h.query_by_label("5 matches").is_some());
    h.get_by_label("Match case").click();
    h.run_steps(3);
    assert!(h.query_all_by_label_contains("of 4").next().is_some() || h.query_by_label("4 matches").is_some(), "`Book` no longer matches");
    h.get_by_label("Whole word").click();
    h.run_steps(3);
    assert!(
        h.query_all_by_label_contains("of 2").next().is_some() || h.query_by_label("2 matches").is_some(),
        "`books` and `magazine` are not the word"
    );
    // A pattern that is not valid says so instead of finding nothing.
    h.get_by_label("Regular expression").click();
    h.get_by_label("Find").focus();
    h.run_steps(2);
    h.get_by_label("Find").type_text("(");
    h.run_steps(3);
    assert!(
        h.query_all_by_label_contains("not valid").next().is_some()
            || h.query_all_by_label_contains("unclosed").next().is_some()
            || h.query_all_by_label_contains("unopened").next().is_some(),
        "the problem is shown"
    );
}

#[test]
fn a_selection_becomes_the_search_and_f3_goes_on_without_the_bar() {
    let mut h = shell_with(BOOKS);
    let at = BOOKS.find("magazine").expect("there");
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(at, at + 8);
    h.state_mut().execute("edit.find");
    h.run_steps(3);
    assert_eq!(h.get_by_label("Find").value().as_deref(), Some("magazine"), "the selected text is what to look for");
    // F3 and Shift+F3 are commands of their own.
    h.state_mut().execute("edit.find_next");
    h.run_steps(2);
    assert_eq!(selections(&h), [(at, at + 8)], "the only match: it wraps back to it");
    assert!(h.state_mut().execute("edit.find_previous"));
}

#[test]
fn replace_one_and_replace_all() {
    let mut h = shell_with(BOOKS);
    h.state_mut().execute("edit.replace");
    h.run_steps(3);
    h.get_by_label("Find").type_text("<book ");
    h.get_by_label("Replace with").focus();
    h.run_steps(2);
    h.get_by_label("Replace with").type_text("<volume ");
    h.run_steps(3);
    h.get_by_label("Replace").click();
    h.run_steps(3);
    assert_eq!(text(&h).matches("<volume ").count(), 1, "one replaced, the next selected: {}", text(&h));
    h.get_by_label("Replace All").click();
    h.run_steps(3);
    assert_eq!(text(&h).matches("<volume ").count(), 3, "all three `<book ` (the case does not matter): {}", text(&h));
    assert!(h.state().status.starts_with("Replaced"), "{}", h.state().status);
    h.state_mut().execute("edit.undo");
    h.run_steps(2);
    assert_eq!(text(&h).matches("<volume ").count(), 1, "Replace All is one undo step");
}

#[test]
fn control_d_adds_the_next_occurrence_and_typing_changes_all_of_them() {
    let mut h = shell_with("one two one three one");
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(1, 1);
    h.state_mut().execute("edit.select_next_occurrence");
    h.state_mut().execute("edit.select_next_occurrence");
    h.state_mut().execute("edit.select_next_occurrence");
    h.run_steps(2);
    assert_eq!(selections(&h), [(0, 3), (8, 11), (18, 21)]);
    h.get_by_label("XML editor").focus();
    h.run_steps(2);
    h.get_by_label("XML editor").type_text("1");
    h.run_steps(2);
    assert_eq!(text(&h), "1 two 1 three 1");
    // And all at once.
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(0, 0);
    h.state_mut().execute("edit.select_all_occurrences");
    h.run_steps(2);
    assert_eq!(selections(&h).len(), 3, "every `1`");
}
