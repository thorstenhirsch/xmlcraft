//! `grid_rows` and `grid_edit`, through the tool table.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, tool: &str, args: Value) -> Result<Value, ToolError> {
    a.call(tool, &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

const JSON: &str = "{\n  \"name\": \"Tea\",\n  \"qty\": 2,\n  \"tags\": [\"a\", \"b\"]\n}";

fn open(a: &mut Automation, dir: &std::path::Path, name: &str, text: &str) -> u64 {
    std::fs::write(dir.join(name), text).expect("writes");
    call(a, "doc_open", json!({ "path": name })).expect("opens")["id"].as_u64().expect("id")
}

#[test]
fn rows_are_listed_with_paths_values_and_places_and_open_as_asked() {
    let mut a = Automation::new();
    let closed = call(&mut a, "grid_rows", json!({ "text": JSON, "language": "json" })).expect("runs");
    assert_eq!(closed["count"].as_u64(), Some(3), "{closed}");
    assert_eq!(
        (closed["rows"][2]["name"].as_str(), closed["rows"][2]["value"].as_str(), closed["rows"][2]["children"].as_u64()),
        (Some("tags"), Some("[2]"), Some(2))
    );
    let open_all = call(&mut a, "grid_rows", json!({ "text": JSON, "language": "json", "expand": "all" })).expect("runs");
    assert_eq!(open_all["count"].as_u64(), Some(5));
    let item = &open_all["rows"][3];
    assert_eq!(
        (item["name"].as_str(), item["path"].clone(), item["depth"].as_u64(), item["line"].as_u64()),
        (Some("[0]"), json!([2, 0]), Some(1), Some(4))
    );
    let some = call(&mut a, "grid_rows", json!({ "text": JSON, "language": "json", "expand": [[2]] })).expect("runs");
    assert_eq!(some["count"].as_u64(), Some(5));
    let xml = call(&mut a, "grid_rows", json!({ "text": "<a n=\"1\"><b>x</b></a>", "expand": "all" })).expect("runs");
    assert_eq!(xml["rows"].as_array().map(Vec::len), Some(3), "the text of <b> is in the row of b: {xml}");
    assert!(matches!(call(&mut a, "grid_rows", json!({ "text": JSON, "language": "json", "expand": "some" })), Err(ToolError::InvalidArgs(_))));
    assert!(call(&mut a, "grid_rows", json!({ "text": "{", "language": "json" })).is_err());
}

#[test]
fn an_edit_changes_the_open_document_with_the_smallest_edit_and_one_undo_takes_it_back() {
    let dir = tempfile::tempdir().expect("dir");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let id = open(&mut a, dir.path(), "d.json", JSON);
    call(&mut a, "grid_edit", json!({ "id": id, "action": "set_value", "path": [1], "value": "5" })).expect("edits");
    assert_eq!(
        call(&mut a, "doc_text", json!({ "id": id })).expect("text")["text"].as_str(),
        Some(JSON.replace("\"qty\": 2", "\"qty\": 5").as_str())
    );
    call(&mut a, "grid_edit", json!({ "id": id, "action": "rename", "path": [0], "value": "title" })).expect("edits");
    call(&mut a, "grid_edit", json!({ "id": id, "action": "remove", "path": [2, 0] })).expect("edits");
    let text = call(&mut a, "doc_text", json!({ "id": id })).expect("text")["text"].as_str().expect("text").to_owned();
    assert_eq!(text, "{\n  \"title\": \"Tea\",\n  \"qty\": 5,\n  \"tags\": [\"b\"]\n}");
    call(&mut a, "grid_edit", json!({ "id": id, "action": "set_literal", "path": [1], "value": "null" })).expect("edits");
    assert!(call(&mut a, "doc_text", json!({ "id": id })).expect("text")["text"].as_str().is_some_and(|t| t.contains("\"qty\": null")));
    call(&mut a, "doc_undo", json!({ "id": id })).expect("undo");
    assert!(
        call(&mut a, "doc_text", json!({ "id": id })).expect("text")["text"].as_str().is_some_and(|t| t.contains("\"qty\": 5")),
        "one undo is one edit"
    );
}

#[test]
fn what_cannot_be_done_is_an_error_that_says_why_and_leaves_the_text_alone() {
    let dir = tempfile::tempdir().expect("dir");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let id = open(&mut a, dir.path(), "d.json", JSON);
    let e = call(&mut a, "grid_edit", json!({ "id": id, "action": "set_value", "path": [1], "value": "two" })).expect_err("not a number");
    assert!(e.to_string().contains("not a number"), "{e}");
    let e = call(&mut a, "grid_edit", json!({ "id": id, "action": "set_value", "path": [9], "value": "x" })).expect_err("no such row");
    assert!(e.to_string().contains("not there"), "{e}");
    assert!(matches!(call(&mut a, "grid_edit", json!({ "id": id, "action": "set_value", "path": [1] })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(call(&mut a, "grid_edit", json!({ "id": id, "action": "frobnicate", "path": [1] })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(call(&mut a, "grid_edit", json!({ "id": id, "action": "remove", "path": ["x"] })), Err(ToolError::InvalidArgs(_))));
    assert_eq!(call(&mut a, "doc_text", json!({ "id": id })).expect("text")["text"].as_str(), Some(JSON));
    // XML and YAML documents too.
    let xml = open(&mut a, dir.path(), "d.xml", "<a n=\"1\">x</a>");
    call(&mut a, "grid_edit", json!({ "id": xml, "action": "set_value", "path": [0, 0], "value": "a&b" })).expect("edits");
    assert_eq!(call(&mut a, "doc_text", json!({ "id": xml })).expect("text")["text"].as_str(), Some("<a n=\"a&amp;b\">x</a>"));
    let yaml = open(&mut a, dir.path(), "d.yaml", "k: v\nn: 1\n");
    call(&mut a, "grid_edit", json!({ "id": yaml, "action": "remove", "path": [0] })).expect("edits");
    assert_eq!(call(&mut a, "doc_text", json!({ "id": yaml })).expect("text")["text"].as_str(), Some("n: 1\n"));
}
