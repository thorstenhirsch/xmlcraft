//! Find and replace in the real shell: the bar, the counter, stepping through matches, replacing, and the extra carets of ⌘D.

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
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

/// The bar's counter ("2 of 5", "5 matches"…), `None` while there is none.
fn counter(h: &Harness<'static, XmlCraftApp>) -> Option<String> {
    let mut labels = h.query_all_by_label_contains(" of ").chain(h.query_all_by_label_contains(" match")).filter_map(|n| {
        let a = n.accesskit_node();
        a.label().or_else(|| a.value())
    });
    // The counter is the label that starts with a number (the buttons are "Next match", "Previous match").
    labels.find(|l| l.starts_with(|c: char| c.is_ascii_digit()))
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
    // The commands step the same way: Shift+F3 from the first match wraps round to the last one.
    h.key_press_modifiers(Modifiers::SHIFT, Key::F3);
    h.run_steps(3);
    let last = BOOKS.rfind("book").expect("there");
    assert_eq!(selections(&h), [(last, last + 4)], "backwards from the first match: the last one, in the closing tag");
    assert_eq!(counter(&h).as_deref(), Some("5 of 5"));
    h.key_press_modifiers(Modifiers::SHIFT, Key::F3);
    h.run_steps(3);
    assert_eq!(counter(&h).as_deref(), Some("4 of 5"));
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
    assert_eq!(counter(&h).as_deref(), Some("1 of 5"));
    h.get_by_label("Match case").click();
    h.run_steps(3);
    assert!(counter(&h).is_some_and(|c| c.ends_with(" of 4")), "`Book` no longer matches: {:?}", counter(&h));
    h.get_by_label("Whole word").click();
    h.run_steps(3);
    assert!(counter(&h).is_some_and(|c| c.ends_with(" of 2")), "`books` and `magazine` are not the word: {:?}", counter(&h));
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
    // F3 and Shift+F3 are commands of their own: they carry on without the bar.
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(h.query_by_label("Find").is_none(), "the bar is closed");
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(0, 0);
    h.key_press(Key::F3);
    h.run_steps(2);
    assert_eq!(selections(&h), [(at, at + 8)], "F3 finds the next match of the retained query");
    assert!(h.query_by_label("Find").is_none(), "F3 does not bring the bar back when there is a query");
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(BOOKS.len(), BOOKS.len());
    h.key_press_modifiers(Modifiers::SHIFT, Key::F3);
    h.run_steps(2);
    assert_eq!(selections(&h), [(at, at + 8)], "Shift+F3 goes backwards from the end: the same, only, match");
    assert!(h.query_by_label("Find").is_none());
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
    let next = text(&h).find("<Book ").expect("the next match is still there");
    assert_eq!(selections(&h), [(next, next + 6)], "and the next `<Book ` is selected: {}", text(&h));
    h.get_by_label("Replace All").click();
    h.run_steps(3);
    assert_eq!(text(&h).matches("<volume ").count(), 3, "all three `<book ` (the case does not matter): {}", text(&h));
    assert!(h.state().status.starts_with("Replaced"), "{}", h.state().status);
    h.state_mut().execute("edit.undo");
    h.run_steps(2);
    assert_eq!(text(&h).matches("<volume ").count(), 1, "Replace All is one undo step");
}

#[test]
fn replace_all_says_which_limit_stopped_it() {
    let mut h = shell_with(&"x".repeat(10_001));
    h.state_mut().execute("edit.replace");
    h.run_steps(3);
    h.get_by_label("Find").type_text("x");
    h.get_by_label("Replace with").focus();
    h.run_steps(2);
    h.get_by_label("Replace with").type_text("y");
    h.run_steps(3);
    h.get_by_label("Replace All").click();
    h.run_steps(3);
    assert_eq!(h.state().status, "More than 10000 matches: nothing was replaced");
    assert_eq!(text(&h), "x".repeat(10_001), "nothing changed");
    // Few matches, but together more than may be written.
    let mut h = shell_with(&"x".repeat(5000));
    h.state_mut().execute("edit.replace");
    h.run_steps(3);
    h.get_by_label("Find").type_text("x");
    h.get_by_label("Replace with").focus();
    h.run_steps(2);
    h.get_by_label("Replace with").type_text(&"y".repeat(4000));
    h.run_steps(3);
    h.get_by_label("Replace All").click();
    h.run_steps(3);
    assert_eq!(h.state().status, "The replacements would be more than 16 MiB: nothing was replaced");
    assert_eq!(text(&h), "x".repeat(5000), "nothing changed");
}

#[test]
fn a_text_too_large_to_count_is_still_searched_and_stepped_through() {
    // Over 4 MiB: the bar does not read the whole text after every edit, so it does not count; it still finds and goes to matches.
    let filler = "<row>filler text to make the document big</row>\n".repeat(100_000);
    let text = format!("<rows>\n{filler}<hit>needle</hit>\n</rows>\n");
    assert!(text.len() > 4 * 1024 * 1024);
    let mut h = shell_with(&text);
    h.state_mut().execute("edit.find");
    h.run_steps(3);
    h.get_by_label("Find").type_text("needle");
    h.run_steps(3);
    assert!(h.query_by_label("Too large to count").is_some(), "no counter for a text this size");
    let at = text.find("needle").expect("there");
    assert_eq!(selections(&h), [(at, at + 6)], "the match is found, and selected");
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(0, 0);
    h.key_press(Key::F3);
    h.run_steps(2);
    assert_eq!(selections(&h), [(at, at + 6)], "F3 goes to it");
}

#[test]
fn control_d_adds_the_next_occurrence_and_typing_changes_all_of_them() {
    let mut h = shell_with("one two one three one");
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(1, 1);
    for _ in 0..3 {
        h.key_press_modifiers(Modifiers::COMMAND, Key::D);
        h.run_steps(2);
    }
    assert_eq!(selections(&h), [(0, 3), (8, 11), (18, 21)]);
    h.get_by_label("XML editor").focus();
    h.run_steps(2);
    h.get_by_label("XML editor").type_text("1");
    h.run_steps(2);
    assert_eq!(text(&h), "1 two 1 three 1");
    // And all at once.
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(0, 0);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::L);
    h.run_steps(2);
    assert_eq!(selections(&h).len(), 3, "every `1`");
}
