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
    let lines = starts_of_lines(BROKEN, &[1, 2, 3, 4, 5, 6, 7]);
    // Each step lands on a line that has a mistake, going forward through the text.
    let mut seen = Vec::new();
    for _ in 0..errors {
        assert!(h.state_mut().execute("xml.next_problem"));
        h.run_steps(2);
        seen.push(caret(&h));
    }
    assert!(seen.windows(2).all(|w| w[0] < w[1]), "forward: {seen:?}");
    assert!(seen.iter().all(|c| lines.iter().any(|l| l <= c)), "inside the text");
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
