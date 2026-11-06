//! Autosave recovery in the real shell: unsaved text is kept on disk while it is unsaved, removed when it is saved or closed, and what a
//! session that did not end normally left is offered at the next start and recovered with its edits.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_platform::recovery::{self, Snapshot};
use xmlcraft_ui_egui::XmlCraftApp;

fn shell(dir: &Path) -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.state_mut().set_recovery_interval(Duration::from_millis(50));
    h.state_mut().set_recovery_dir(Some(dir.to_path_buf()));
    h.run_steps(4);
    h
}

fn file(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path
}

fn until(h: &mut Harness<'static, XmlCraftApp>, what: &str, done: impl Fn() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for {what}");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn snapshot(key: &str, name: &str, path: Option<&Path>, text: &str) -> Snapshot {
    Snapshot { key: key.to_owned(), name: name.to_owned(), path: path.map(|p| p.to_string_lossy().into_owned()), text: text.to_owned() }
}

#[test]
fn unsaved_text_is_kept_while_it_is_unsaved_and_removed_when_it_is_saved_or_closed() {
    let docs = tempfile::tempdir().expect("docs");
    let keep = tempfile::tempdir().expect("keep");
    let mut h = shell(keep.path());
    assert!(h.state_mut().open_path(&file(docs.path(), "a.xml", "<a/>")));
    h.run_steps(3);
    assert!(recovery::list(keep.path()).is_empty(), "nothing is kept for text that is saved");
    {
        let doc = h.state_mut().session.active_document_mut().expect("shown");
        doc.editor_mut().select_all();
        doc.editor_mut().type_text("<a>unsaved</a>");
    }
    until(&mut h, "the snapshot", || recovery::list(keep.path()).iter().any(|s| s.text == "<a>unsaved</a>"));
    let kept = recovery::list(keep.path());
    assert_eq!((kept.len(), kept[0].name.as_str()), (1, "a.xml"));
    assert!(kept[0].path.as_deref().is_some_and(|p| p.ends_with("a.xml")));
    // Saved: the snapshot goes.
    assert!(h.state_mut().execute("file.save"));
    until(&mut h, "the snapshot to go after the save", || recovery::list(keep.path()).is_empty());
    // Edited again, then closed without saving: gone as well.
    {
        let doc = h.state_mut().session.active_document_mut().expect("shown");
        doc.editor_mut().select_all();
        doc.editor_mut().type_text("<a>again</a>");
    }
    until(&mut h, "the second snapshot", || !recovery::list(keep.path()).is_empty());
    assert!(h.state_mut().execute("file.close"));
    h.run_steps(3);
    h.get_by_label("Don't Save").click_accesskit();
    h.run_steps(3);
    until(&mut h, "the snapshot to go after the close", || recovery::list(keep.path()).is_empty());
}

#[test]
fn what_an_earlier_session_left_is_offered_and_recovered_with_its_edits_unsaved() {
    let docs = tempfile::tempdir().expect("docs");
    let keep = tempfile::tempdir().expect("keep");
    let saved = file(docs.path(), "b.xml", "<b>on disk</b>");
    recovery::write(keep.path(), &snapshot("old-1", "b.xml", Some(&saved), "<b>typed and lost</b>")).expect("writes");
    recovery::write(keep.path(), &snapshot("old-2", "Untitled-3", None, "<new/>")).expect("writes");
    // A snapshot of text that is the same as the file's is nothing to recover.
    let same = file(docs.path(), "c.xml", "<c/>");
    recovery::write(keep.path(), &snapshot("old-3", "c.xml", Some(&same), "<c/>")).expect("writes");
    let mut h = shell(keep.path());
    assert_eq!(h.state().recoverable(), 3);
    assert!(h.query_by_label("Recover unsaved work?").is_some(), "the person is asked");
    h.get_by_label("Recover").click_accesskit();
    h.run_steps(4);
    assert_eq!(h.state().recoverable(), 0);
    assert!(recovery::list(keep.path()).iter().all(|s| !s.key.starts_with("old-")), "what was recovered is deleted");
    let docs_open: Vec<(String, String, bool)> =
        h.state().session.documents().iter().map(|d| (d.name().to_owned(), d.editor().text(), d.is_dirty())).collect();
    assert!(docs_open.contains(&("b.xml".to_owned(), "<b>typed and lost</b>".to_owned(), true)), "{docs_open:?}");
    assert!(docs_open.iter().any(|(n, t, dirty)| n.starts_with("Untitled") && t == "<new/>" && *dirty), "{docs_open:?}");
    assert!(docs_open.iter().any(|(n, t, dirty)| n == "c.xml" && t == "<c/>" && !dirty), "{docs_open:?}");
    assert_eq!(std::fs::read_to_string(&saved).expect("reads"), "<b>on disk</b>", "the file itself is untouched");
}

#[test]
fn discarding_deletes_what_was_left_and_later_keeps_it() {
    let keep = tempfile::tempdir().expect("keep");
    recovery::write(keep.path(), &snapshot("old-1", "x.xml", None, "<x/>")).expect("writes");
    let mut h = shell(keep.path());
    assert!(h.query_by_label("Recover unsaved work?").is_some());
    h.get_by_label("Later").click_accesskit();
    h.run_steps(3);
    assert_eq!(recovery::list(keep.path()).len(), 1, "Later keeps it");
    let mut h = shell(keep.path());
    h.get_by_label("Discard").click_accesskit();
    h.run_steps(3);
    assert!(recovery::list(keep.path()).is_empty() && h.state().session.documents().is_empty());
}

#[test]
fn without_a_folder_nothing_is_kept_and_nothing_is_offered() {
    let docs = tempfile::tempdir().expect("docs");
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(3);
    assert!(h.state_mut().open_path(&file(docs.path(), "a.xml", "<a/>")));
    {
        let doc = h.state_mut().session.active_document_mut().expect("shown");
        doc.editor_mut().type_text("x");
    }
    h.run_steps(10);
    assert_eq!(h.state().recoverable(), 0);
}
