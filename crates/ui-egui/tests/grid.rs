//! The Grid view in the real shell: a document shown as rows, a cell edited in place, and the text that results.

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

fn text(h: &Harness<'static, XmlCraftApp>) -> String {
    h.state().session.active_document().expect("shown").editor().text()
}

const JSON: &str = "{\n  \"name\": \"Tea\",\n  \"qty\": 2,\n  \"tags\": [\"a\", \"b\"]\n}";

/// Show the grid and choose the row whose label starts with `label`.
fn choose(h: &mut Harness<'static, XmlCraftApp>, label: &str) {
    h.get_by_label_contains(label).click_accesskit();
    h.run_steps(2);
}

#[test]
fn a_document_is_shown_as_rows_that_open_and_close() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "d.json", JSON));
    assert!(h.state_mut().execute("view.grid"));
    h.run_steps(3);
    assert!(h.query_by_label("name: Tea (string)").is_some() && h.query_by_label("qty: 2 (number)").is_some(), "the members are rows");
    assert!(h.query_by_label("tags: [2] (array)").is_some() && h.query_by_label_contains("[0]").is_none(), "an array is closed at first");
    choose(&mut h, "tags: [2]");
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(3);
    assert!(h.query_by_label("[0]: a (string)").is_some() && h.query_by_label("[1]: b (string)").is_some(), "the items are rows once it is open");
    h.key_press(egui::Key::ArrowLeft);
    h.run_steps(3);
    assert!(h.query_by_label_contains("[0]").is_none(), "closed again");
    // The same command shows the text again.
    assert!(h.state_mut().execute("view.grid"));
    h.run_steps(3);
    assert!(h.query_by_label_contains("qty: 2").is_none(), "the grid is gone");
    assert_eq!(text(&h), JSON, "looking at the grid changed nothing");
}

#[test]
fn a_value_is_edited_in_place_and_the_text_changes_by_that_value_only() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "d.json", JSON));
    assert!(h.state_mut().execute("view.grid"));
    h.run_steps(3);
    choose(&mut h, "qty: 2");
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let field = h.get_by_label("The value of the row");
    assert_eq!(field.accesskit_node().value().as_deref(), Some("2"));
    field.focus();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get_by_label("The value of the row").type_text("15");
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(text(&h), JSON.replace("\"qty\": 2", "\"qty\": 15"));
    assert!(h.query_by_label("qty: 15 (number)").is_some(), "the grid shows the new value");
    // One undo takes the edit back.
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    assert_eq!(text(&h), JSON);
}

#[test]
fn a_value_that_does_not_fit_its_type_is_refused_with_the_reason_and_the_text_is_left_alone() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "d.json", JSON));
    assert!(h.state_mut().execute("view.grid"));
    h.run_steps(3);
    choose(&mut h, "qty: 2");
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    let field = h.get_by_label("The value of the row");
    field.focus();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get_by_label("The value of the row").type_text("two");
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert!(h.state().status.contains("Not changed") && h.state().status.contains("not a number"), "{}", h.state().status);
    assert_eq!(text(&h), JSON);
    assert!(h.query_by_label("The value of the row").is_some(), "the field stays open to be put right");
}

#[test]
fn a_row_is_removed_with_delete_and_a_name_is_changed_with_f2() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "d.json", JSON));
    assert!(h.state_mut().execute("view.grid"));
    h.run_steps(3);
    choose(&mut h, "name: Tea");
    h.key_press(egui::Key::F2);
    h.run_steps(3);
    let field = h.get_by_label("The name of the row");
    field.focus();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get_by_label("The name of the row").type_text("title");
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(text(&h), JSON.replace("\"name\"", "\"title\""));
    choose(&mut h, "qty: 2");
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert_eq!(text(&h), "{\n  \"title\": \"Tea\",\n  \"tags\": [\"a\", \"b\"]\n}");
    assert!(h.query_by_label_contains("qty").is_none());
}

#[test]
fn a_document_that_cannot_be_read_says_so_instead_of_a_grid_and_xml_and_yaml_have_grids_too() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "bad.json", "{\"a\": "));
    assert!(h.state_mut().execute("view.grid"));
    h.run_steps(3);
    assert!(h.query_by_label_contains("The grid needs a document that can be read").is_some());
    open(&mut h, &file(dir.path(), "d.xml", "<order id=\"7\"><item>Tea</item></order>"));
    assert!(h.state_mut().execute("view.grid"));
    h.run_steps(3);
    assert!(h.query_by_label("order (element)").is_some());
    choose(&mut h, "order (element)");
    h.key_press(egui::Key::ArrowRight);
    h.run_steps(3);
    assert!(h.query_by_label("id: 7 (attribute)").is_some() && h.query_by_label("item: Tea (element)").is_some());
    open(&mut h, &file(dir.path(), "d.yaml", "k: v\nn: 1\n"));
    assert!(h.state_mut().execute("view.grid"));
    h.run_steps(3);
    assert!(h.query_by_label("k: v (string)").is_some() && h.query_by_label("n: 1 (number)").is_some());
}
