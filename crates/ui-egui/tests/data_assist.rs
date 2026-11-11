//! Completion in JSON and YAML documents, in the real shell: the popup that follows a name or value being typed, from the JSON Schema the
//! document names.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_ui_egui::XmlCraftApp;

const SCHEMA: &str = r##"{
  "type": "object",
  "properties": {
    "name": {"type": "string", "description": "The full name."},
    "kind": {"enum": ["alpha", "beta"]},
    "home": {"type": "object", "properties": {"city": {"type": "string"}}}
  },
  "required": ["name"]
}"##;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).with_step_dt(0.02).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

fn file(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path
}

/// Open `marked` (the caret at `¦`) once the schema it names has been loaded.
fn open_at(h: &mut Harness<'static, XmlCraftApp>, dir: &Path, name: &str, marked: &str) {
    let caret = marked.find('¦').expect("a marker");
    assert!(h.state_mut().open_path(&file(dir, name, &marked.replacen('¦', "", 1))));
    let started = Instant::now();
    while !h.state().has_data_schema() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the schema");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(3);
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(caret, caret);
    h.run_steps(4);
}

fn text(h: &Harness<'static, XmlCraftApp>) -> String {
    h.state().session.active_document().expect("shown").editor().text()
}

fn type_text(h: &mut Harness<'static, XmlCraftApp>, typed: &str) {
    h.get_by_label("XML editor").type_text(typed);
    h.run_steps(4);
}

#[test]
fn a_name_being_typed_in_json_is_completed_with_the_value_its_type_needs() {
    let dir = tempfile::tempdir().expect("dir");
    file(dir.path(), "s.json", SCHEMA);
    let mut h = shell();
    open_at(&mut h, dir.path(), "d.json", "{\"$schema\": \"s.json\", \"ho¦\"}");
    type_text(&mut h, "m");
    assert!(h.query_by_label("Complete home").is_some(), "home fits what was typed");
    assert!(h.query_by_label("Complete name").is_none(), "name does not");
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert_eq!(text(&h), "{\"$schema\": \"s.json\", \"home\": {}}", "an object property gets braces, the quotes were replaced whole");
    assert!(h.query_by_label("Complete home").is_none(), "the popup closed");
}

#[test]
fn a_value_being_typed_is_completed_from_the_enum_and_a_line_break_is_not_taken_by_an_empty_popup() {
    let dir = tempfile::tempdir().expect("dir");
    file(dir.path(), "s.json", SCHEMA);
    let mut h = shell();
    open_at(&mut h, dir.path(), "d.json", "{\"$schema\": \"s.json\", \"kind\": \"¦\"}");
    type_text(&mut h, "b");
    assert!(h.query_by_label("Complete \"beta\"").is_some() && h.query_by_label("Complete \"alpha\"").is_none());
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert_eq!(text(&h), "{\"$schema\": \"s.json\", \"kind\": \"beta\"}");
    // Between names nothing is being typed, so no popup stands in the way of Enter; a quote starts a name and opens one.
    let mut other = shell();
    open_at(&mut other, dir.path(), "e.json", "{\"$schema\": \"s.json\", ¦}");
    type_text(&mut other, " ");
    assert!(other.query_by_label("Complete name").is_none(), "no popup before a name is started");
    type_text(&mut other, "\"");
    assert!(other.query_by_label("Complete name").is_some(), "a quote starts a name");
}

#[test]
fn yaml_is_completed_by_indentation_from_the_schema_of_its_modeline() {
    let dir = tempfile::tempdir().expect("dir");
    file(dir.path(), "s.json", SCHEMA);
    let mut h = shell();
    open_at(&mut h, dir.path(), "d.yaml", "# yaml-language-server: $schema=s.json\nname: x\nhome:\n  ci¦\n");
    type_text(&mut h, "t");
    assert!(h.query_by_label("Complete city").is_some());
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert_eq!(text(&h), "# yaml-language-server: $schema=s.json\nname: x\nhome:\n  city: \n");
}

#[test]
fn yaml_flow_mappings_are_completed_too() {
    let dir = tempfile::tempdir().expect("dir");
    file(dir.path(), "s.json", SCHEMA);
    let mut h = shell();
    open_at(&mut h, dir.path(), "d.yaml", "# yaml-language-server: $schema=s.json\nname: x\nhome: {¦}\n");
    type_text(&mut h, "c");
    assert!(h.query_by_label("Complete city").is_some());
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert_eq!(text(&h), "# yaml-language-server: $schema=s.json\nname: x\nhome: {city: }\n");
}

#[test]
fn yaml_flow_mappings_are_completed_on_the_lines_after_they_open() {
    let dir = tempfile::tempdir().expect("dir");
    file(dir.path(), "s.json", SCHEMA);
    let mut h = shell();
    open_at(&mut h, dir.path(), "d.yaml", "# yaml-language-server: $schema=s.json\nname: x\nhome: {\n  ¦\n}\n");
    type_text(&mut h, "c");
    assert!(h.query_by_label("Complete city").is_some());
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert_eq!(text(&h), "# yaml-language-server: $schema=s.json\nname: x\nhome: {\n  city: \n}\n");
}

#[test]
fn edit_complete_opens_the_popup_between_names_and_after_a_colon_where_nothing_was_typed() {
    let dir = tempfile::tempdir().expect("dir");
    file(dir.path(), "s.json", SCHEMA);
    let mut h = shell();
    open_at(&mut h, dir.path(), "d.json", "{\"$schema\": \"s.json\", ¦}");
    assert!(h.query_by_label("Complete name").is_none(), "nothing pops up by itself");
    assert!(h.state_mut().execute("edit.complete"));
    h.run_steps(3);
    assert!(h.query_by_label("Complete name").is_some() && h.query_by_label("Complete home").is_some(), "the names of the schema");
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert!(text(&h).contains("\"name\": \"\""), "the first choice is written: {}", text(&h));
    // After a colon: the values.
    open_at(&mut h, dir.path(), "e.json", "{\"$schema\": \"s.json\", \"kind\": ¦}");
    assert!(h.state_mut().execute("edit.complete"));
    h.run_steps(3);
    assert!(h.query_by_label("Complete \"alpha\"").is_some());
    // Where there is nothing to offer it says so.
    open_at(&mut h, dir.path(), "f.json", "{\"$schema\": \"s.json\", \"name\": \"x\"}¦");
    assert!(h.state_mut().execute("edit.complete"));
    h.run_steps(3);
    assert!(h.query_by_label("Complete name").is_none());
    assert!(h.state().status.contains("Nothing to complete"), "{}", h.state().status);
}

#[test]
fn an_object_or_an_array_with_no_value_is_completed_as_an_empty_shape() {
    let dir = tempfile::tempdir().expect("dir");
    file(dir.path(), "s.json", SCHEMA);
    let mut h = shell();
    open_at(&mut h, dir.path(), "d.json", "{\"$schema\": \"s.json\", \"home\": ¦}");
    assert!(h.state_mut().execute("edit.complete"));
    h.run_steps(3);
    assert!(h.query_by_label("Complete {}").is_some(), "an object to write");
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert_eq!(text(&h), "{\"$schema\": \"s.json\", \"home\": {}}");
    file(dir.path(), "a.json", "{\"type\": \"object\", \"properties\": {\"list\": {\"type\": \"array\", \"items\": {\"type\": \"string\"}}}}");
    open_at(&mut h, dir.path(), "e.json", "{\"$schema\": \"a.json\", \"list\": ¦}");
    assert!(h.state_mut().execute("edit.complete"));
    h.run_steps(3);
    assert!(h.query_by_label("Complete []").is_some(), "an array to write");
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert_eq!(text(&h), "{\"$schema\": \"a.json\", \"list\": []}");
}
