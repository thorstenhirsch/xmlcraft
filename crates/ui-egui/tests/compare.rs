//! The Compare tool window in the real shell: two open documents compared, the differences listed, and a click taking you to one.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use xmlcraft_ui_egui::XmlCraftApp;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 900.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

fn file(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path
}

fn open(h: &mut Harness<'static, XmlCraftApp>, path: &Path) {
    assert!(h.state_mut().open_path(path));
    let started = Instant::now();
    while h.state().problems().is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the first analysis");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
}

fn wait_for(h: &mut Harness<'static, XmlCraftApp>, what: &str) {
    let started = Instant::now();
    while h.query_by_label_contains(what).is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for {what:?}");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
}

const A: &str = "<order><item n=\"1\">Tea</item><item n=\"2\">Milk</item></order>";
const B: &str = "<order><item n=\"1\">Tea</item><item n=\"2\">Oat milk</item></order>";

fn two(h: &mut Harness<'static, XmlCraftApp>, dir: &Path, a: (&str, &str), b: (&str, &str)) {
    open(h, &file(dir, b.0, b.1));
    open(h, &file(dir, a.0, a.1));
}

fn selected(h: &Harness<'static, XmlCraftApp>) -> String {
    let doc = h.state().session.active_document().expect("shown");
    let text = doc.editor().text();
    let s = doc.editor().selections()[0];
    let (start, end) = (s.start().min(s.end()), s.start().max(s.end()));
    format!("{}:{}", doc.name(), &text[start..end])
}

#[test]
fn two_documents_are_compared_and_a_click_selects_the_difference_in_the_first() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    two(&mut h, dir.path(), ("a.xml", A), ("b.xml", B));
    assert!(h.state_mut().execute("file.compare"));
    h.run_steps(3);
    assert!(h.query_by_label("Close Compare").is_some(), "the tool window is up");
    assert!(h.query_by_label("→ a.xml").is_some(), "it says which document it works on");
    h.get_by_label("Compare Documents").click();
    wait_for(&mut h, "1 differences");
    // (A pointer is over a row for a frame before it clicks it, as a hand does.)
    h.get_by_label_contains("≠").hover();
    h.run_steps(2);
    h.get_by_label_contains("≠").click();
    h.run_steps(3);
    assert_eq!(selected(&h), "a.xml:Milk");
    assert!(h.query_by_label("Close Compare").is_some(), "the panel stays open");
    assert!(h.query_by_label_contains("1 differences").is_some(), "with the list, for the next click");
}

#[test]
fn a_right_click_goes_to_the_place_in_the_second_document() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    two(&mut h, dir.path(), ("a.xml", A), ("b.xml", B));
    assert!(h.state_mut().execute("file.compare"));
    h.run_steps(3);
    h.get_by_label("Compare Documents").click();
    wait_for(&mut h, "1 differences");
    h.get_by_label_contains("≠").hover();
    h.run_steps(2);
    h.get_by_label_contains("≠").click_secondary();
    h.run_steps(3);
    assert_eq!(selected(&h), "b.xml:Oat milk");
    assert!(h.query_by_label_contains("1 differences").is_some(), "the list is there with either document in front");
}

#[test]
fn the_same_documents_say_so_and_an_ignore_option_makes_a_difference_go_away() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    two(&mut h, dir.path(), ("a.xml", "<r>a  b</r>"), ("b.xml", "<r>a b</r>"));
    assert!(h.state_mut().execute("file.compare"));
    h.run_steps(3);
    h.get_by_label("Compare Documents").click();
    wait_for(&mut h, "1 differences");
    h.get_by_label("White space").hover();
    h.run_steps(2);
    h.get_by_label("White space").click();
    h.run_steps(2);
    h.get_by_label("Compare Documents").click();
    wait_for(&mut h, "The documents are the same");
}

#[test]
fn one_document_alone_has_nothing_to_compare_with_until_another_is_opened() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.xml", A));
    assert!(h.state_mut().execute("file.compare"));
    h.run_steps(3);
    assert!(h.query_by_label("Close Compare").is_some());
    assert!(h.query_by_label_contains("second document").is_some(), "the panel says what is missing");
    assert!(h.get_by_label("Compare Documents").accesskit_node().is_disabled());
    // The list of the other documents is recomputed from the session at every frame.
    open(&mut h, &file(dir.path(), "b.xml", B));
    h.state_mut().session.set_active(Some(0));
    h.run_steps(3);
    assert!(h.query_by_label_contains("second document").is_none());
    assert!(!h.get_by_label("Compare Documents").accesskit_node().is_disabled());
}

#[test]
fn a_difference_found_before_an_edit_no_longer_points_into_the_text() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    two(&mut h, dir.path(), ("a.xml", A), ("b.xml", B));
    assert!(h.state_mut().execute("file.compare"));
    h.run_steps(3);
    h.get_by_label("Compare Documents").click();
    wait_for(&mut h, "1 differences");
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().type_text("<!-- x -->");
    h.run_steps(3);
    assert!(h.query_by_label_contains("compare again").is_some(), "the list says it is out of date");
}
