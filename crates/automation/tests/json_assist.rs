//! `json_complete` and `json_hover`, driven through `Automation::call` with real files: the schema a document names gives the names and
//! values that may be written, for a document that is being typed, in JSON and in YAML, and nothing outside the root is read.

use std::path::Path;

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content};

fn call(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args).unwrap_or_else(|e| panic!("{tool} failed: {e}")).into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    }
}

fn file(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path.to_str().expect("utf-8 path").to_owned()
}

const SCHEMA: &str = r##"{
  "$defs": {"address": {"type": "object", "description": "Where.", "properties": {"city": {"type": "string"}, "zip": {"type": "string"}}, "required": ["city"]}},
  "type": "object",
  "properties": {
    "name": {"type": "string", "description": "The full name."},
    "kind": {"enum": ["alpha", "beta"]},
    "home": {"$ref": "#/$defs/address"}
  },
  "required": ["name"]
}"##;

fn labels(r: &Value) -> Vec<&str> {
    r["items"].as_array().expect("items").iter().filter_map(|i| i["label"].as_str()).collect()
}

#[test]
fn names_and_values_come_from_the_schema_the_document_names_even_while_it_is_being_typed() {
    let dir = tempfile::tempdir().expect("dir");
    file(dir.path(), "s.json", SCHEMA);
    let mut a = Automation::with_root(dir.path()).expect("root");
    let typed = "{\n  \"$schema\": \"s.json\",\n  \"name\": \"x\",\n  \n}";
    let doc = file(dir.path(), "d.json", typed);
    let r = call(&mut a, "json_complete", json!({ "path": doc, "line": 4, "column": 3 }));
    assert_eq!((r["schema"].as_bool(), labels(&r)), (Some(true), vec!["kind", "home"]), "{r}");
    assert_eq!(r["items"][0]["insert"].as_str(), Some("\"kind\": "));
    assert_eq!((r["items"][0]["start"].as_u64(), r["items"][0]["kind"].as_str()), (r["items"][0]["end"].as_u64(), Some("property")));
    let with_schema = file(dir.path(), "e.json", "{\"$schema\": \"s.json\", \"kind\": \"b\"}");
    let r = call(&mut a, "json_complete", json!({ "path": with_schema, "offset": 32 }));
    assert_eq!(labels(&r), ["\"beta\""], "{r}");
}

#[test]
fn hover_says_what_the_schema_says_and_yaml_works_with_the_modeline() {
    let dir = tempfile::tempdir().expect("dir");
    file(dir.path(), "s.json", SCHEMA);
    let mut a = Automation::with_root(dir.path()).expect("root");
    let doc = file(dir.path(), "d.json", "{\"$schema\": \"s.json\", \"name\": \"x\"}");
    let r = call(&mut a, "json_hover", json!({ "path": doc, "offset": 25 }));
    assert_eq!((r["found"].as_bool(), r["text"].as_str()), (Some(true), Some("The full name.\n\nType: string")), "{r}");
    assert_eq!((r["start"].as_u64(), r["end"].as_u64()), (Some(22), Some(28)));
    let nothing = call(&mut a, "json_hover", json!({ "path": doc, "offset": 2 }));
    assert_eq!(nothing["found"].as_bool(), Some(false));
    let yaml = file(dir.path(), "d.yaml", "# yaml-language-server: $schema=s.json\nname: x\nhome:\n  \n");
    let r = call(&mut a, "json_complete", json!({ "path": yaml, "line": 4, "column": 3 }));
    assert_eq!(labels(&r), ["city", "zip"], "{r}");
    assert_eq!(r["items"][0]["insert"].as_str(), Some("city: "));
}

#[test]
fn a_document_with_no_schema_or_in_the_wrong_language_offers_nothing_or_is_refused() {
    let mut a = Automation::new();
    let r = call(&mut a, "json_complete", json!({ "text": "{}", "language": "json", "offset": 1 }));
    assert_eq!((r["schema"].as_bool(), r["count"].as_u64()), (Some(false), Some(0)));
    let e = a.call("json_complete", &json!({ "text": "<a/>", "offset": 1 })).expect_err("xml is refused").to_string();
    assert!(e.contains("JSON"), "{e}");
    let e = a.call("json_hover", &json!({ "text": "{}", "language": "json", "offset": 99 })).expect_err("bad offset").to_string();
    assert!(e.contains("offset"), "{e}");
}

#[test]
fn a_schema_outside_the_root_is_not_read() {
    let root = tempfile::tempdir().expect("root");
    let outside = tempfile::tempdir().expect("outside");
    let schema = file(outside.path(), "s.json", SCHEMA);
    let doc = file(root.path(), "d.json", &format!("{{\"$schema\": {}, \n \n}}", json!(schema)));
    let mut a = Automation::with_root(root.path()).expect("root");
    let r = call(&mut a, "json_complete", json!({ "path": doc, "line": 2, "column": 2 }));
    assert_eq!(r["count"].as_u64(), Some(0), "{r}");
}

#[test]
fn a_bson_file_is_completed_and_hovered_in_its_extended_json_text() {
    use xmlcraft_bson::{Bson, Document, to_text, write_all};
    let dir = tempfile::tempdir().expect("dir");
    file(dir.path(), "s.json", SCHEMA);
    let docs = vec![Document { fields: vec![("$schema".into(), Bson::String("s.json".into())), ("name".into(), Bson::String("x".into()))] }];
    let path = dir.path().join("d.bson");
    std::fs::write(&path, write_all(&docs).expect("writes")).expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("root");
    // The positions are in the Extended JSON text the window shows, as the codec lays it out.
    let (text, _) = to_text(&docs, true).expect("text");
    let after = text.find('{').expect("an object") + 1;
    let r = call(&mut a, "json_complete", json!({ "path": path, "offset": after }));
    assert_eq!(r["schema"].as_bool(), Some(true), "{r}");
    assert!(["kind", "home"].iter().all(|n| labels(&r).contains(n)), "the schema's names are offered: {r}");
    let name = text.find("\"name\"").expect("the name") + 2;
    let r = call(&mut a, "json_hover", json!({ "path": path, "offset": name }));
    assert_eq!(r["text"].as_str(), Some("The full name.\n\nType: string"), "{r}");
    let r = call(&mut a, "json_complete", json!({ "text": "{\"$schema\": \"s.json\", }", "language": "bson", "offset": 22 }));
    assert_eq!(r["schema"].as_bool(), Some(true), "Extended JSON text names its schema too: {r}");
}

#[test]
fn an_object_or_an_array_the_schema_offers_no_value_for_is_offered_as_an_empty_shape() {
    let dir = tempfile::tempdir().expect("dir");
    file(dir.path(), "s.json", SCHEMA);
    let mut a = Automation::with_root(dir.path()).expect("root");
    let doc = file(dir.path(), "d.json", "{\"$schema\": \"s.json\", \"home\": }");
    let r = call(&mut a, "json_complete", json!({ "path": doc, "offset": 30 }));
    assert_eq!(labels(&r), ["{}"], "{r}");
    assert_eq!((r["items"][0]["cursor"].as_u64(), r["items"][0]["detail"].as_str()), (Some(1), Some("object")));
}
