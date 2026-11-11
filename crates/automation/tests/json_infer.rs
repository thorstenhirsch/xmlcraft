//! `json_infer`: a JSON Schema generated from JSON, YAML and BSON documents.

use std::path::Path;

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, tool: &str, args: Value) -> Result<Value, ToolError> {
    a.call(tool, &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

fn infer(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    call(a, "json_infer", args)
}

fn write(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path.to_str().expect("utf-8").to_owned()
}

fn schema(result: &Value) -> Value {
    serde_json::from_str(result["schema"].as_str().expect("schema text")).expect("JSON")
}

#[test]
fn text_gives_a_schema_that_names_what_it_saw() {
    let mut a = Automation::new();
    let result = infer(&mut a, json!({ "text": "{\"name\": \"a\", \"age\": 3}", "language": "json" })).expect("runs");
    assert_eq!(result["documents"], 1);
    let schema = schema(&result);
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["name"]["type"], "string");
    assert_eq!(schema["properties"]["age"]["type"], "integer");
    assert_eq!(schema["required"], json!(["age", "name"]));
}

#[test]
fn several_documents_are_learned_together_from_files_and_open_documents() {
    let dir = tempfile::tempdir().expect("dir");
    let yaml = write(dir.path(), "one.yaml", "name: a\nage: 1\n");
    write(dir.path(), "two.json", "{\"name\": \"b\", \"home\": {\"city\": \"x\"}}");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let opened = call(&mut a, "doc_open", json!({ "path": "two.json" })).expect("opens");
    let result = infer(&mut a, json!({ "sources": [{ "path": yaml }, { "id": opened["id"] }, { "text": "{\"name\": \"c\"}", "language": "json" }] }))
        .expect("runs");
    assert_eq!(result["documents"], 3);
    let schema = schema(&result);
    assert_eq!(schema["required"], json!(["name"]));
    assert_eq!(schema["properties"]["home"]["properties"]["city"]["type"], "string");
}

#[test]
fn bson_lines_are_read_as_extended_json_and_the_enumeration_rule_is_optional() {
    let mut a = Automation::new();
    let args = json!({ "text": "{\"kind\": \"red\"}\n{\"kind\": \"green\"}\n{\"kind\": \"red\"}\n{\"kind\": \"green\"}\n", "language": "bson" });
    let result = infer(&mut a, args.clone()).expect("runs");
    let learned = schema(&result);
    assert_eq!(learned["properties"]["kind"]["enum"], json!(["green", "red"]));
    let mut plain = args;
    plain["enumerations"] = json!(false);
    let learned = schema(&infer(&mut a, plain).expect("runs"));
    assert_eq!(learned["properties"]["kind"].get("enum"), None);
}

#[test]
fn a_broken_document_is_named_and_the_arguments_are_checked() {
    let mut a = Automation::new();
    let err = infer(&mut a, json!({ "sources": [{ "text": "{\"a\": 1}", "language": "json" }, { "text": "{", "language": "json" }] }))
        .expect_err("the second is broken");
    assert!(matches!(&err, ToolError::Failed(m) if m.starts_with("document 2:")), "{err:?}");
    for args in [
        json!({}),
        json!({ "text": "{}", "path": "x.json" }),
        json!({ "sources": [] }),
        json!({ "sources": [{ "text": "{}", "language": "json" }], "text": "{}" }),
        json!({ "text": "<a/>", "language": "xml" }),
        json!({ "text": "{}", "language": "json", "enumerations": "many" }),
        json!({ "text": "{}", "language": "json", "enumerations": { "max_values": 0 } }),
        json!({ "text": "{}", "language": "json", "surprise": true }),
    ] {
        let err = infer(&mut a, args.clone()).expect_err(&args.to_string());
        assert!(matches!(err, ToolError::InvalidArgs(_) | ToolError::Failed(_)), "{args}: {err:?}");
    }
}

#[test]
fn documents_too_large_together_are_refused_once_the_cap_is_passed() {
    let dir = tempfile::tempdir().expect("dir");
    // Three files of 12 MiB: the third passes the 32 MiB cap of the engine.
    let big = format!("[{}0]", "0,".repeat(6 * 1024 * 1024));
    for name in ["a.json", "b.json", "c.json"] {
        write(dir.path(), name, &big);
    }
    let mut a = Automation::with_root(dir.path()).expect("root");
    let err = infer(&mut a, json!({ "sources": [{ "path": "a.json" }, { "path": "b.json" }, { "path": "c.json" }] })).expect_err("too large");
    assert!(matches!(&err, ToolError::Failed(m) if m.contains("reading stopped at document 3")), "{err:?}");
    let ok =
        infer(&mut a, json!({ "text": "[\"x\", \"x\"]", "language": "json", "enumerations": { "max_values": 1 } })).expect("one value is allowed");
    assert_eq!(schema(&ok)["items"]["enum"], json!(["x"]));
}
