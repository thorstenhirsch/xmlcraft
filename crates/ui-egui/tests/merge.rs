//! The Merge window in the real shell: three documents merged line by line into a new one.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
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

/// Base, theirs and ours are opened in that order, so ours is the one shown and the window offers the base and theirs in that order.
fn three(h: &mut Harness<'static, XmlCraftApp>, dir: &Path, base: &str, theirs: &str, ours: &str) {
    open(h, &file(dir, "base.xml", base));
    open(h, &file(dir, "theirs.xml", theirs));
    open(h, &file(dir, "ours.xml", ours));
}

fn shown(h: &Harness<'static, XmlCraftApp>) -> (String, String) {
    let d = h.state().session.active_document().expect("shown");
    (d.name().to_owned(), d.editor().text())
}

#[test]
fn changes_that_do_not_meet_are_both_kept_in_a_new_document() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    three(&mut h, dir.path(), "<a>\n<b/>\n<m/>\n<c/>\n</a>\n", "<a>\n<b/>\n<m/>\n<c x=\"1\"/>\n</a>\n", "<a>\n<b y=\"2\"/>\n<m/>\n<c/>\n</a>\n");
    assert!(h.state_mut().execute("file.merge"));
    h.run_steps(3);
    assert!(h.query_by_label("Merge documents").is_some(), "the window is up");
    h.get_by_label("Merge").click();
    h.run_steps(3);
    assert_eq!(shown(&h), ("ours-merged.xml".to_owned(), "<a>\n<b y=\"2\"/>\n<m/>\n<c x=\"1\"/>\n</a>\n".to_owned()));
    assert!(h.state().status.contains("no conflicts"), "{}", h.state().status);
}

#[test]
fn what_both_changed_is_marked_as_a_conflict_or_taken_as_chosen() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    three(&mut h, dir.path(), "<a>\n<b/>\n</a>\n", "<a>\n<b t=\"theirs\"/>\n</a>\n", "<a>\n<b o=\"ours\"/>\n</a>\n");
    assert!(h.state_mut().execute("file.merge"));
    h.run_steps(3);
    h.get_by_label("Merge").click();
    h.run_steps(3);
    let (name, text) = shown(&h);
    assert_eq!(name, "ours-merged.xml");
    assert!(text.contains("<<<<<<<") && text.contains("o=\"ours\"") && text.contains("t=\"theirs\"") && text.contains(">>>>>>>"), "{text}");
    assert!(h.state().status.contains("1 conflicts are marked"), "{}", h.state().status);
}

#[test]
fn a_merge_needs_three_documents_and_two_different_others() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.xml", "<a/>"));
    open(&mut h, &file(dir.path(), "b.xml", "<b/>"));
    assert!(h.state_mut().execute("file.merge"));
    h.run_steps(3);
    assert!(h.query_by_label("Merge documents").is_none());
    assert!(h.state().status.contains("three documents"), "{}", h.state().status);
}
