//! `json_validate`, driven through `Automation::call` with real files: the schema as text, as a file, as the file the document names,
//! the documents it refers to, and the root confinement.

use std::path::Path;

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    a.call("json_validate", &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

fn ok(a: &mut Automation, args: Value) -> Value {
    call(a, args).unwrap_or_else(|e| panic!("json_validate failed: {e}"))
}

fn err(a: &mut Automation, args: Value) -> String {
    call(a, args).expect_err("the call should fail").to_string()
}

fn file(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("dir");
    }
    std::fs::write(&path, text).expect("writes");
    path.to_str().expect("utf-8 path").to_owned()
}

const SCHEMA: &str = r#"{"type":"object","properties":{"name":{"type":"string"},"age":{"type":"integer","minimum":0},"tags":{"type":"array","items":{"type":"string"}}},"required":["name"],"additionalProperties":false}"#;

#[test]
fn violations_are_listed_with_their_place_in_the_text() {
    let mut a = Automation::new();
    let text = "{\n  \"name\": 5,\n  \"age\": -1,\n  \"tags\": [\"a\", 2],\n  \"extra\": true\n}";
    let r = ok(&mut a, json!({ "text": text, "schema_text": SCHEMA }));
    assert_eq!((r["valid"].as_bool(), r["violation_count"].as_u64(), r["draft"].as_str()), (Some(false), Some(4), Some("2020-12")));
    let found: Vec<_> = r["violations"]
        .as_array()
        .expect("list")
        .iter()
        .map(|v| (v["keyword"].as_str().unwrap_or_default(), v["instance_pointer"].as_str().unwrap_or_default(), v["line"].as_u64()))
        .collect();
    assert_eq!(
        found,
        [("type", "/name", Some(2)), ("minimum", "/age", Some(3)), ("type", "/tags/1", Some(4)), ("additionalProperties", "/extra", Some(5))]
    );
    assert_eq!(r["violations"][0]["column"].as_u64(), Some(3), "a member is shown at its name");
    assert!(r["violations"][2]["message"].as_str().is_some_and(|m| m.contains("Expected string, found integer")));
    let valid = ok(&mut a, json!({ "text": r#"{"name":"x","age":3,"tags":[]}"#, "schema_text": SCHEMA }));
    assert_eq!((valid["valid"].as_bool(), valid["violations"].as_array().map(Vec::len)), (Some(true), Some(0)));
}

#[test]
fn syntax_errors_make_a_document_invalid_and_are_listed() {
    let mut a = Automation::new();
    let r = ok(&mut a, json!({ "text": "{\"name\": \"x\",}", "schema_text": SCHEMA }));
    assert_eq!(r["valid"].as_bool(), Some(false));
    assert_eq!(r["syntax_errors"][0]["code"].as_str(), Some("json.trailing-comma"));
    assert_eq!(r["violation_count"].as_u64(), Some(0), "the tree that could be read is valid");
    // JSONC allows it.
    let r = ok(&mut a, json!({ "text": "{\"name\": \"x\",}", "language": "jsonc", "schema_text": SCHEMA }));
    assert_eq!(r["valid"].as_bool(), Some(true));
}

#[test]
fn the_schema_is_a_file_or_the_one_the_document_names_and_refs_are_files_next_to_it() {
    let dir = tempfile::tempdir().expect("dir");
    let schema = file(
        dir.path(),
        "schemas/person.json",
        r##"{"$schema":"http://json-schema.org/draft-07/schema#","properties":{"address":{"$ref":"address.json"}}}"##,
    );
    file(dir.path(), "schemas/address.json", r#"{"type":"object","required":["city"]}"#);
    let doc = file(dir.path(), "data/p.json", r#"{"$schema":"../schemas/person.json","address":{}}"#);
    let mut a = Automation::new();
    let r = ok(&mut a, json!({ "path": doc, "schema": schema }));
    assert_eq!((r["valid"].as_bool(), r["draft"].as_str()), (Some(false), Some("07")));
    assert_eq!((r["violations"][0]["keyword"].as_str(), r["violations"][0]["instance_pointer"].as_str()), (Some("required"), Some("/address")));
    // The document names its schema (relative to itself).
    let r = ok(&mut a, json!({ "path": doc }));
    assert_eq!(r["violation_count"].as_u64(), Some(1));
    // An open document too.
    let id = ok_open(&mut a, &doc);
    assert_eq!(ok(&mut a, json!({ "id": id }))["violation_count"].as_u64(), Some(1));
    // No schema, or one that is not there.
    assert!(err(&mut a, json!({ "text": "{}" })).contains("no schema"));
    assert!(err(&mut a, json!({ "text": "{}", "schema": dir.path().join("none.json").to_str().expect("path") })).contains("none.json"));
    assert!(err(&mut a, json!({ "text": "{}", "schema_text": "{" })).contains("not valid JSON"));
    assert!(err(&mut a, json!({ "text": "{}", "schema_text": "{}", "draft": "2" })).contains("draft"));
    assert!(err(&mut a, json!({ "text": "<a/>", "language": "xml", "schema_text": "{}" })).contains("reads JSON"));
}

fn ok_open(a: &mut Automation, path: &str) -> u64 {
    a.call("doc_open", &json!({ "path": path }))
        .ok()
        .and_then(|c| match c.into_iter().next() {
            Some(Content::Json(v)) => v["id"].as_u64(),
            None => None,
        })
        .expect("opens")
}

#[test]
fn json_lines_are_validated_line_by_line() {
    let mut a = Automation::new();
    let text = "{\"name\":\"a\"}\n{\"name\":1}\n\n{\"age\":2}\n";
    let r = ok(&mut a, json!({ "text": text, "language": "jsonl", "schema_text": SCHEMA }));
    let found: Vec<_> =
        r["violations"].as_array().expect("list").iter().map(|v| (v["keyword"].as_str().unwrap_or_default(), v["line"].as_u64())).collect();
    assert_eq!(found, [("type", Some(2)), ("required", Some(4))]);
}

#[test]
fn files_the_schema_refers_to_outside_the_root_are_not_read() {
    let root = tempfile::tempdir().expect("root");
    let outside = tempfile::tempdir().expect("outside");
    file(outside.path(), "secret.json", r#"{"type":"string"}"#);
    let schema_inside = file(root.path(), "s.json", &format!(r#"{{"$ref":"file://{}/secret.json"}}"#, outside.path().display()));
    let mut confined = Automation::with_root(root.path()).expect("root");
    let r = ok(&mut confined, json!({ "text": "1", "schema": schema_inside }));
    assert_eq!(r["valid"].as_bool(), Some(false));
    assert_eq!(r["violations"][0]["keyword"].as_str(), Some("$ref"), "the reference cannot be resolved");
    let outside_schema = file(outside.path(), "o.json", "{}");
    assert!(err(&mut confined, json!({ "text": "1", "schema": outside_schema })).contains("outside the root"));
}
