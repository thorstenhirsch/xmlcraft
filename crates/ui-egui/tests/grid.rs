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
    // "qty" is also in the Outline pane, which comes from an analysis that runs on a thread: on a slow runner it is a moment behind the
    // text, so wait for it (counting frames was not enough on macOS CI).
    let started = Instant::now();
    while h.query_by_label_contains("qty").is_some() {
        assert!(started.elapsed() < Duration::from_secs(10), "the row, and the outline, no longer show qty");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(10));
    }
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

// --- tables ------------------------------------------------------------------------------------------------------------------

const ITEMS: &str = "[\n  {\"name\": \"Tea\", \"qty\": 2},\n  {\"name\": \"Milk\", \"qty\": 10, \"tags\": [\"a\"]},\n  {\"qty\": 1}\n]\n";

/// Open `text` as `name` and show it as a grid.
fn grid_of(h: &mut Harness<'static, XmlCraftApp>, dir: &Path, name: &str, text: &str) {
    open(h, &file(dir, name, text));
    assert!(h.state_mut().execute("view.grid"));
    h.run_steps(3);
}

/// Type into the cell field that is open, replacing what is in it, and commit with Enter.
fn type_cell(h: &mut Harness<'static, XmlCraftApp>, typed: &str) {
    let field = h.get_by_label("The value of the cell");
    field.focus();
    h.run_steps(1);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get_by_label("The value of the cell").type_text(typed);
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
}

fn select_cell(h: &mut Harness<'static, XmlCraftApp>, label: &str) {
    h.get_by_label(label).click_accesskit();
    h.run_steps(2);
}

#[test]
fn a_json_array_of_objects_opens_as_a_table_with_the_union_of_its_keys() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    grid_of(&mut h, dir.path(), "items.json", ITEMS);
    for column in ["Column name", "Column qty", "Column tags"] {
        assert!(h.query_by_label(column).is_some(), "{column}");
    }
    assert!(h.query_by_label("Row 2, qty: 10").is_some() && h.query_by_label("Row 3, name: empty").is_some());
    assert!(h.query_by_label("Row 2, tags: [1]").is_some(), "a nested cell shows what it holds");
    assert_eq!(text(&h), ITEMS, "looking changed nothing");
}

#[test]
fn a_cell_edited_in_place_changes_that_value_only_and_one_undo_restores_the_bytes() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    grid_of(&mut h, dir.path(), "items.json", ITEMS);
    select_cell(&mut h, "Row 1, qty: 2");
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(h.get_by_label("The value of the cell").accesskit_node().value().as_deref(), Some("2"));
    type_cell(&mut h, "15");
    assert_eq!(text(&h), ITEMS.replace("\"qty\": 2}", "\"qty\": 15}"));
    assert!(h.query_by_label("Row 1, qty: 15").is_some(), "the table shows the new value");
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    assert_eq!(text(&h), ITEMS);
}

#[test]
fn typing_into_a_missing_cell_creates_the_member_in_that_row_only() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    grid_of(&mut h, dir.path(), "items.json", ITEMS);
    select_cell(&mut h, "Row 3, name: empty");
    // Typing on a chosen cell starts editing it with what was typed.
    h.event(egui::Event::Text("O".into()));
    h.run_steps(3);
    assert_eq!(h.get_by_label("The value of the cell").accesskit_node().value().as_deref(), Some("O"));
    type_cell(&mut h, "Oat");
    assert_eq!(text(&h), ITEMS.replace("{\"qty\": 1}", "{\"name\": \"Oat\", \"qty\": 1}"));
}

#[test]
fn a_sorted_view_changes_nothing_until_its_order_is_applied() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    grid_of(&mut h, dir.path(), "items.json", ITEMS);
    h.get_by_label("Column qty").click_accesskit();
    h.run_steps(3);
    assert!(h.query_by_label("Column qty, sorted ascending").is_some());
    assert_eq!(text(&h), ITEMS, "a view");
    h.get_by_label("Apply order to document").click_accesskit();
    h.run_steps(3);
    assert_eq!(text(&h), "[\n  {\"qty\": 1},\n  {\"name\": \"Tea\", \"qty\": 2},\n  {\"name\": \"Milk\", \"qty\": 10, \"tags\": [\"a\"]}\n]\n");
    assert!(h.query_by_label("Apply order to document").is_none(), "no longer sorted: the text is in that order");
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    assert_eq!(text(&h), ITEMS, "one undo");
}

#[test]
fn cells_copied_as_tab_separated_text_paste_back_and_add_rows() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    grid_of(&mut h, dir.path(), "items.json", ITEMS);
    select_cell(&mut h, "Row 1, name: Tea");
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::ArrowRight);
    h.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::ArrowDown);
    h.run_steps(2);
    h.event(egui::Event::Copy);
    h.step();
    let copied = h.output().platform_output.commands.iter().find_map(|c| match c {
        egui::OutputCommand::CopyText(t) => Some(t.clone()),
        _ => None,
    });
    assert_eq!(copied.as_deref(), Some("Tea\t2\nMilk\t10\n"));
    h.run_steps(2);
    select_cell(&mut h, "Row 3, name: empty");
    h.event(egui::Event::Paste("Tea\t2\nMilk\t10\n".into()));
    h.run_steps(4);
    assert_eq!(
        text(&h),
        "[\n  {\"name\": \"Tea\", \"qty\": 2},\n  {\"name\": \"Milk\", \"qty\": 10, \"tags\": [\"a\"]},\n  {\"name\": \"Tea\", \"qty\": 2},\n  {\"name\": \"Milk\", \"qty\": 10}\n]\n"
    );
    h.state_mut().execute("edit.undo");
    h.run_steps(3);
    assert_eq!(text(&h), ITEMS, "the paste is one undoable step");
}

const LIBRARY: &str = "<library>\n  <book id=\"b1\" language=\"en\">\n    <title>Almanac</title>\n    <author>Ada</author>\n    <author>Bo</author>\n  </book>\n  <book id=\"b2\">\n    <title>Nebel</title>\n    <author>Jonas</author>\n  </book>\n</library>\n";

#[test]
fn repeated_xml_elements_show_attribute_columns_and_a_nested_cell_drills_in_and_back() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    grid_of(&mut h, dir.path(), "library.xml", LIBRARY);
    h.get_by_label("Open library as a table").click_accesskit();
    h.run_steps(3);
    for column in ["Column @id", "Column @language", "Column title", "Column author"] {
        assert!(h.query_by_label(column).is_some(), "{column}");
    }
    assert!(h.query_by_label("Row 2, @language: empty").is_some());
    // Two authors: a nested cell, opened as the table of those authors.
    h.get_by_label("Row 1, author: <2>").click_accesskit();
    h.run_steps(3);
    assert!(h.query_by_label("book › author").is_some(), "the breadcrumb names the step below the books");
    assert!(h.query_by_label("Column #text").is_some() && h.query_by_label("Row 2, #text: Bo").is_some(), "the authors of b1");
    assert!(h.query_by_label("Column @id").is_none());
    // The breadcrumb leads back.
    h.get_by_label("library › book").click_accesskit();
    h.run_steps(3);
    assert!(h.query_by_label("Column @id").is_some(), "back in the table of the books");
    h.get_by_label("Tree").click_accesskit();
    h.run_steps(3);
    assert!(h.query_by_label("Column @id").is_none() && h.query_by_label("library (element)").is_some(), "back in the tree");
    assert_eq!(text(&h), LIBRARY);
}

#[test]
fn json_lines_open_as_their_table_and_delete_clears_the_chosen_cell() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    let lines = "{\"id\": 1}\n{\"id\": 2}\n{\"id\": 3}\n";
    grid_of(&mut h, dir.path(), "events.jsonl", lines);
    assert!(h.query_by_label("Column id").is_some() && h.query_by_label("Row 3, id: 3").is_some());
    select_cell(&mut h, "Row 2, id: 2");
    h.key_press(egui::Key::Delete);
    h.run_steps(3);
    assert_eq!(text(&h), "{\"id\": 1}\n{}\n{\"id\": 3}\n", "Delete clears the cell");
}
