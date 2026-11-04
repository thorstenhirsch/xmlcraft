//! The Problems panel, the way from one problem to the next, and the header that says where a document stands, in the real shell.

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

fn caret(h: &Harness<'static, XmlCraftApp>) -> usize {
    h.state().session.active_document().expect("shown").editor().primary().head()
}

/// A text with three mistakes, on lines 2, 4 and 6.
const BROKEN: &str = "<r>\n<a></b>\n<c/>\n<d></e>\n<f/>\n<g></h>\n</r>\n";

fn starts_of_lines(text: &str, lines: &[usize]) -> Vec<usize> {
    let mut offsets = vec![0];
    for (i, c) in text.char_indices() {
        if c == '\n' {
            offsets.push(i + 1);
        }
    }
    lines.iter().filter_map(|l| offsets.get(*l - 1).copied()).collect()
}

#[test]
fn the_keys_step_from_one_problem_to_the_next_and_round() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "broken.xml", BROKEN));
    let errors = h.state().problems().expect("analysed").0;
    assert!(errors >= 3, "{errors} errors");
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().place_caret(0, false);
    let starts = starts_of_lines(BROKEN, &[2, 3, 4, 5, 6, 7]);
    let wrong_lines = [(starts[0], starts[1]), (starts[2], starts[3]), (starts[4], starts[5])];
    // Each step lands on a line that has a mistake, going forward through the text.
    let mut seen = Vec::new();
    for _ in 0..errors {
        assert!(h.state_mut().execute("xml.next_problem"));
        h.run_steps(2);
        seen.push(caret(&h));
    }
    assert!(seen.windows(2).all(|w| w[0] < w[1]), "forward: {seen:?}");
    assert!(seen.iter().all(|c| wrong_lines.iter().any(|(from, to)| from <= c && c < to)), "each stop is on line 2, 4 or 6: {seen:?}");
    // Past the last it is the first again.
    assert!(h.state_mut().execute("xml.next_problem"));
    h.run_steps(2);
    assert_eq!(caret(&h), seen[0], "round to the first");
    // And back: the previous of the first is the last.
    assert!(h.state_mut().execute("xml.previous_problem"));
    h.run_steps(2);
    assert_eq!(caret(&h), *seen.last().expect("some"), "round to the last");
    assert!(h.query_all_by_label_contains("Problem ").next().is_some(), "the status bar says which problem it is");
}

#[test]
fn a_document_with_no_problems_says_so_when_asked_for_the_next() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "ok.xml", "<a/>"));
    assert!(h.state_mut().execute("xml.next_problem"));
    h.run_steps(3);
    assert!(h.query_by_label("There are no problems.").is_some());
    assert_eq!(caret(&h), 0, "the caret stays");
}

#[test]
fn the_panel_lists_the_problems_and_a_filter_narrows_them() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "broken.xml", BROKEN));
    let errors = h.state().problems().expect("analysed").0;
    assert!(h.query_by_label(&format!("All {errors}")).is_some(), "the chip counts every problem");
    assert!(h.query_by_label(&format!("Errors {errors}")).is_some());
    assert!(h.query_by_label("Warnings 0").is_some());
    h.get_by_label("Warnings 0").click();
    h.run_steps(3);
    assert!(h.query_by_label("Nothing in this list").is_some(), "no warnings: the list says so and how to see the rest");
    h.get_by_label(&format!("Errors {errors}")).click();
    h.run_steps(3);
    assert!(h.query_by_label("Nothing in this list").is_none());
}

#[test]
fn the_rail_counts_the_problems_on_its_button_and_the_header_says_where_the_document_stands() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "ok.xml", "<a><b/></a>"));
    assert!(h.query_by_label("Well-formed").is_some(), "the header's pill");
    assert!(h.query_by_label("No schema").is_some(), "a document that follows nothing says so, and offers to assign");
    assert!(h.query_by_label("Select element a").is_some(), "the path of the caret starts at the root");
    assert!(h.query_by_label("Toggle Problems").is_some(), "no problems, no count");
    open(&mut h, &file(dir.path(), "bad.xml", BROKEN));
    assert!(h.query_all_by_label_contains(" errors").next().is_some(), "the pill counts the errors");
    let errors = h.state().problems().expect("analysed").0;
    assert!(h.query_by_label(&format!("Toggle Problems, {errors} problems")).is_some(), "the rail's button counts them too");
    assert!(h.query_by_label("Select element a").is_none(), "another document, another path");
}

#[test]
fn the_count_on_the_rail_includes_what_the_validation_found() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "v.xml", "<!DOCTYPE r [<!ELEMENT r EMPTY>]><r><x/></r>"));
    assert!(h.query_by_label("Toggle Problems").is_some(), "well-formed: nothing counted until it is validated");
    assert!(h.state_mut().execute("xml.validate"));
    wait(&mut h, "the validation", |app| app.validity().is_some());
    let (errors, _) = h.state().validity().expect("validated");
    assert!(errors >= 1);
    assert!(h.query_by_label(&format!("Toggle Problems, {errors} problems")).is_some(), "the rail counts the validity errors");
    assert!(h.query_all_by_label_contains(&format!("{errors} validity errors")).next().is_some(), "and so does the pill of the header");
}

#[test]
fn what_is_wrong_at_a_place_is_what_the_hover_tip_says() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "broken.xml", BROKEN));
    // `</b>` on line 2 does not close `<a>`; `<c/>` on line 3 is fine.
    let end_tag = BROKEN.find("</b>").expect("in the text");
    let messages = h.state().problem_messages_at(end_tag + 2);
    assert!(!messages.is_empty(), "a mistake has a message");
    assert!(messages.len() <= 4);
    let fine = BROKEN.find("<c/>").expect("in the text");
    assert!(h.state().problem_messages_at(fine + 1).is_empty(), "nothing is wrong there");
    // Text that was edited since the analysis has other places: no tip rather than a wrong one.
    // (At the end, so that the old places are still where they were: only the version says the analysis is old.)
    let editor = h.state_mut().session.active_document_mut().expect("shown").editor_mut();
    editor.place_caret(BROKEN.len(), false);
    editor.type_input("x");
    assert!(h.state().problem_messages_at(end_tag + 2).is_empty(), "the analysis is about older text");
}

#[test]
fn the_pointer_resting_on_a_mistake_shows_its_message() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "broken.xml", BROKEN));
    let expected = h.state().problem_messages_at(BROKEN.find("</b>").expect("in the text") + 2);
    assert!(!expected.is_empty());
    let geometry = h.state().editor_geometry().expect("the editor was drawn");
    // Line 2 is `<a></b>`: the pointer goes to the `/` of the end tag.
    let at = egui::pos2(geometry.text_x + geometry.column_width * 4.5, geometry.origin.y + geometry.row_height * 1.5);
    h.hover_at(at);
    let started = Instant::now();
    // The Problems panel lists the same message with its place after it; the tip is the message alone.
    while h.query_all_by_label(&expected[0]).count() == 0 {
        assert!(started.elapsed() < Duration::from_secs(5), "waiting for the tip: {:?}", expected[0]);
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_problem_that_can_be_fixed_has_a_fix_button_that_puts_it_right_in_one_undoable_step() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "fixme.xml", "<a>\n<b></c>\n</a>\n"));
    h.run_steps(3);
    h.get_by_label("Fix: Rename the end tag to </b>").click();
    h.run_steps(4);
    assert_eq!(h.state().session.active_document().expect("shown").editor().text(), "<a>\n<b></b>\n</a>\n");
    assert!(h.state().status.contains("Fixed: Rename the end tag to </b>"), "{}", h.state().status);
    assert!(h.state_mut().execute("edit.undo"));
    assert_eq!(h.state().session.active_document().expect("shown").editor().text(), "<a>\n<b></c>\n</a>\n", "one step back");
}

#[test]
fn quick_fix_at_the_caret_fixes_what_is_wrong_there_and_says_so_when_nothing_is() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "q.xml", "<a>\n  <b x=1/>\n</a>\n"));
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().place_caret(0, false);
    assert!(h.state_mut().execute("edit.quick_fix"));
    assert!(h.state().status.contains("nothing to fix"), "{}", h.state().status);
    let at = "<a>\n  <b x=1/>\n</a>\n".find("1/>").expect("in the text");
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().place_caret(at, false);
    h.run_steps(2);
    assert!(h.state_mut().execute("edit.quick_fix"));
    assert_eq!(h.state().session.active_document().expect("shown").editor().text(), "<a>\n  <b x=\"1\"/>\n</a>\n");
    let caret = h.state().session.active_document().expect("shown").editor().primary().head();
    assert_eq!(caret, at + 3, "the caret is after the closing quote");
}

#[test]
fn a_fix_made_for_older_text_is_not_offered_and_one_on_the_same_line_is() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "q2.xml", "<a>\n  <b x=1/>\n</a>\n"));
    // The caret is on the line of the mistake, not on it: the fix of the problem on this line is the one offered.
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().place_caret(6, false);
    h.run_steps(2);
    assert!(!h.state_mut().quick_fixes_at_caret().is_empty(), "same line");
    // Edited since the analysis: nothing is offered (the places would be those of the older text).
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().type_input("z");
    assert!(h.state_mut().quick_fixes_at_caret().is_empty(), "the problems are those of older text");
    assert!(h.state_mut().execute("edit.quick_fix"));
    assert!(h.state().status.contains("nothing to fix"), "{}", h.state().status);
}

#[test]
fn two_problems_that_start_at_the_same_place_each_have_their_own_fix() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    // `<a` is both an unterminated tag and an unclosed element, both at the name: only the second has a fix.
    open(&mut h, &file(dir.path(), "q3.xml", "<a"));
    h.run_steps(3);
    assert!(h.query_by_label_contains("Fix: Close <a>").is_some(), "the unclosed element has its fix");
}
