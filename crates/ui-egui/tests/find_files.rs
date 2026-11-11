//! Find in Files in the real shell: the Search tool window comes up below the editor, a pattern typed, the folder of the open document
//! searched on a thread, the hits listed by file, and a click on a hit opening that file with the caret on the line while the panel
//! stays.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::Value;
use xmlcraft_ui_egui::{ControlClient, Network, Panel, Place, XmlCraftApp};

fn shell() -> (Harness<'static, XmlCraftApp>, ControlClient) {
    let slot: Arc<Mutex<Option<ControlClient>>> = Arc::new(Mutex::new(None));
    let put = slot.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 900.0)).build_eframe(move |cc| {
        let mut app = XmlCraftApp::with_saved_preferences(cc, Network::offline(), None);
        *put.lock().expect("lock") = Some(app.attach_control(&cc.egui_ctx));
        app
    });
    h.run_steps(4);
    let client = slot.lock().expect("lock").take().expect("the app attached the control channel");
    (h, client)
}

/// Where each shown panel is, as `ui.state` reports it.
fn layout(h: &mut Harness<'static, XmlCraftApp>, client: &ControlClient) -> Value {
    let rx = client.send("ui.state", Value::Null);
    for _ in 0..30 {
        h.run_steps(1);
        if let Ok(reply) = rx.try_recv() {
            return reply.expect("ui.state answers")["layout"].clone();
        }
    }
    panic!("ui.state: no answer after 30 frames")
}

fn file(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("dirs");
    }
    std::fs::write(&path, text).expect("writes");
    path
}

fn wait_for(h: &mut Harness<'static, XmlCraftApp>, label: &str) {
    let started = Instant::now();
    while h.query_by_label_contains(label).is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for {label:?}");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_hits_in_a_folder_are_listed_and_a_click_opens_the_file_at_the_line() {
    let dir = tempfile::tempdir().expect("dir");
    let start = file(dir.path(), "start.xml", "<a/>");
    file(dir.path(), "sub/other.xml", "<r>\n  <i>one</i>\n  <i>needle</i>\n</r>\n");
    file(dir.path(), "notes.txt", "a needle in a haystack\n");
    let (mut h, c) = shell();
    assert!(h.state_mut().open_path(&start));
    h.run_steps(3);
    assert!(!h.state().panel_visible(Panel::Search), "hidden until asked for");
    assert!(h.state_mut().execute("edit.find_in_files"));
    h.run_steps(3);
    assert_eq!(h.state().panel_place(Panel::Search), Some(Place::Bottom));
    assert_eq!(layout(&mut h, &c)["search"], "bottom", "a tool window below the editor");
    assert_eq!(layout(&mut h, &c)["problems"], "bottom", "next to Problems");
    assert!(h.query_by_label("Close Search").is_some(), "the Search tab is shown");
    // The pattern field has the keyboard: typing goes there.
    h.get_by_label("What to find").type_text("needle");
    h.run_steps(2);
    h.get_by_label("Find").click();
    wait_for(&mut h, "2 hits in 2 files");
    assert!(h.query_by_label_contains("sub/other.xml").is_some() && h.query_by_label_contains("notes.txt").is_some());
    // (A pointer is over a row for a frame before it clicks it, as a hand does.)
    h.get_by_label_contains("3:6").hover();
    h.run_steps(2);
    h.get_by_label_contains("3:6").click();
    h.run_steps(4);
    let doc = h.state().session.active_document().expect("shown");
    assert_eq!(doc.name(), "other.xml", "the file of the hit is open");
    let caret = doc.editor().selections()[0].start();
    assert_eq!(doc.editor().text().get(..caret).map(|t| t.matches('\n').count()), Some(2), "the caret is on line 3");
    assert!(h.state().panel_visible(Panel::Search), "the panel stays");
    assert!(h.query_by_label_contains("2 hits in 2 files").is_some(), "with its result, for the next hit");
}

#[test]
fn a_bad_pattern_is_said_and_nothing_is_listed() {
    let dir = tempfile::tempdir().expect("dir");
    let start = file(dir.path(), "start.xml", "<a/>");
    let (mut h, _c) = shell();
    assert!(h.state_mut().open_path(&start));
    h.run_steps(3);
    assert!(h.state_mut().execute("edit.find_in_files"));
    h.run_steps(3);
    h.get_by_label("Regular expression").click();
    h.run_steps(2);
    h.get_by_label("What to find").focus();
    h.run_steps(1);
    h.get_by_label("What to find").type_text("(");
    h.run_steps(2);
    // Enter in the field runs the search.
    h.key_press(egui::Key::Enter);
    wait_for(&mut h, "not valid");
    assert!(h.query_by_label_contains("hits in").is_none());
}
