//! `json_definition` and `json_pointer_at`: where a `$ref` of a JSON or YAML document points, and the pointer of a place.

use std::path::Path;

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, tool: &str, args: Value) -> Result<Value, ToolError> {
    a.call(tool, &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

fn write(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().expect("a folder")).expect("folder");
    std::fs::write(&path, text).expect("writes");
    path.to_str().expect("utf-8").to_owned()
}

#[test]
fn a_local_ref_is_found_by_offset_or_by_line_and_column_in_json_and_yaml() {
    let text = "{\n  \"definitions\": {\"pet\": {}},\n  \"p\": {\"$ref\": \"#/definitions/pet\"}\n}";
    let mut a = Automation::new();
    let at = text.find("#/definitions/pet").expect("ref") + 3;
    let found = call(&mut a, "json_definition", json!({ "text": text, "language": "json", "offset": at })).expect("runs");
    assert_eq!(
        (found["found"].clone(), found["selected"].clone(), found["reference"].clone()),
        (json!(true), json!("pet"), json!("#/definitions/pet")),
        "{found}"
    );
    assert_eq!(
        (found["line"].as_u64(), found["column"].as_u64()),
        (Some(2), u64::try_from(text.lines().nth(1).expect("line").find("pet").expect("key") + 1).ok())
    );
    assert_eq!(found["file"], Value::Null, "the same document");
    let by_position = call(&mut a, "json_definition", json!({ "text": text, "language": "json", "line": 3, "column": 20 })).expect("runs");
    assert_eq!(by_position["start"], found["start"]);
    let yaml = "a:\n  b: 1\nr:\n  $ref: '#/a/b'\n";
    let y = call(&mut a, "json_definition", json!({ "text": yaml, "language": "yaml", "line": 4, "column": 12 })).expect("runs");
    assert_eq!((y["found"].clone(), y["selected"].clone()), (json!(true), json!("b")), "{y}");
    // Not a reference: nothing found, and no error.
    let none = call(&mut a, "json_definition", json!({ "text": text, "language": "json", "line": 2, "column": 6 })).expect("runs");
    assert_eq!(none["found"], json!(false));
}

#[test]
fn a_file_reference_is_read_through_the_root_and_what_is_outside_it_is_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let root = dir.path().join("project");
    write(&root, "api.json", "{\"x\": {\"$ref\": \"defs/pet.json#/pet\"}, \"y\": {\"$ref\": \"../secret.json#/k\"}}");
    write(&root, "defs/pet.json", "{\"pet\": {\"type\": \"object\"}}");
    write(dir.path(), "secret.json", "{\"k\": 1}");
    let mut a = Automation::with_root(&root).expect("root");
    let api = root.join("api.json").to_str().expect("utf-8").to_owned();
    let text = std::fs::read_to_string(&api).expect("reads");
    let at = text.find("defs/pet.json").expect("ref") + 2;
    let found = call(&mut a, "json_definition", json!({ "path": api.clone(), "offset": at })).expect("runs");
    assert_eq!((found["found"].clone(), found["selected"].clone()), (json!(true), json!("pet")), "{found}");
    assert!(found["file"].as_str().is_some_and(|f| f.ends_with("pet.json")), "{found}");
    let outside = text.find("../secret.json").expect("ref") + 2;
    let refused = call(&mut a, "json_definition", json!({ "path": api, "offset": outside })).expect_err("outside the root");
    assert!(!refused.to_string().contains("\"k\""), "{refused}");
}

#[test]
fn a_missing_target_and_a_text_without_a_language_are_errors_that_say_what_is_wrong() {
    let mut a = Automation::new();
    let text = "{\"a\": 1, \"r\": {\"$ref\": \"#/nope\"}}";
    let e = call(&mut a, "json_definition", json!({ "text": text, "language": "json", "offset": 20 })).expect_err("missing");
    assert!(e.to_string().contains("#/nope") && e.to_string().contains("does not exist"), "{e}");
    let e = call(&mut a, "json_definition", json!({ "text": text, "offset": 20 })).expect_err("no language");
    assert!(e.to_string().contains("language"), "{e}");
}

#[test]
fn the_pointer_of_a_place_is_given_for_json_yaml_and_bson_text() {
    let mut a = Automation::new();
    let text = "{\"a/b\": [10, {\"c\": true}]}";
    let p = call(&mut a, "json_pointer_at", json!({ "text": text, "language": "json", "offset": text.find("true").expect("true") })).expect("runs");
    assert_eq!(p["pointer"], json!("/a~1b/1/c"), "{p}");
    let root = call(&mut a, "json_pointer_at", json!({ "text": text, "language": "json", "offset": 0 })).expect("runs");
    assert_eq!(root["pointer"], json!(""));
    let y = call(&mut a, "json_pointer_at", json!({ "text": "k:\n  - 1\n  - 2\n", "language": "yaml", "line": 3, "column": 5 })).expect("runs");
    assert_eq!(y["pointer"], json!("/k/1"));
    let b = call(&mut a, "json_pointer_at", json!({ "text": "{\"n\": {\"$numberInt\": \"5\"}}", "language": "bson", "offset": 20 })).expect("runs");
    assert_eq!(b["pointer"], json!("/n"), "a type wrapper is one value");
    assert!(call(&mut a, "json_pointer_at", json!({ "text": "<a/>", "offset": 1 })).is_err(), "XML has XPaths");
}
