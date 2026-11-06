//! `schemastore_find`, through the tool table: the schemas for a file name from a catalog that is given, and what is refused.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    a.call("schemastore_find", &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

const CATALOG: &str = r#"{"schemas": [
  {"name": "package.json", "description": "NPM", "fileMatch": ["package.json"], "url": "https://json.schemastore.org/package.json"},
  {"name": "GitHub Workflow", "fileMatch": [".github/workflows/*.yml"], "url": "https://json.schemastore.org/github-workflow.json"},
  {"name": "Any JSON", "fileMatch": ["*.json"], "url": "https://example.com/any.json"}]}"#;

#[test]
fn the_schemas_for_a_file_come_from_a_catalog_that_is_given_best_fit_first() {
    let mut a = Automation::new();
    let r = call(&mut a, json!({ "name": "/home/me/app/package.json", "catalog_text": CATALOG })).expect("runs");
    let urls: Vec<&str> = r["schemas"].as_array().expect("list").iter().filter_map(|s| s["url"].as_str()).collect();
    assert_eq!(urls, ["https://json.schemastore.org/package.json", "https://example.com/any.json"], "{r}");
    assert_eq!((r["catalog_entries"].as_u64(), r["count"].as_u64()), (Some(3), Some(2)));
    let r = call(&mut a, json!({ "name": "repo/.github/workflows/ci.yml", "catalog_text": CATALOG, "max": 1 })).expect("runs");
    assert_eq!(r["schemas"][0]["name"].as_str(), Some("GitHub Workflow"));
    // A catalog file is read through the root confinement.
    let dir = tempfile::tempdir().expect("dir");
    std::fs::write(dir.path().join("c.json"), CATALOG).expect("writes");
    let mut confined = Automation::with_root(dir.path()).expect("root");
    let r = call(&mut confined, json!({ "name": "package.json", "catalog": dir.path().join("c.json").to_str().expect("utf-8") })).expect("runs");
    assert_eq!(r["count"].as_u64(), Some(2));
}

#[test]
fn what_cannot_be_asked_is_refused_and_an_offline_session_downloads_nothing() {
    let mut a = Automation::new();
    assert!(matches!(call(&mut a, json!({ "catalog_text": CATALOG })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(call(&mut a, json!({ "name": "x.json", "catalog_text": CATALOG, "max": 0 })), Err(ToolError::InvalidArgs(_))));
    assert!(call(&mut a, json!({ "name": "x.json", "catalog_text": "{}" })).is_err(), "not a catalog");
    let e = call(&mut a, json!({ "name": "x.json" })).expect_err("offline").to_string();
    assert!(e.contains("offline"), "{e}");
    let outside = tempfile::tempdir().expect("dir");
    let root = tempfile::tempdir().expect("root");
    std::fs::write(outside.path().join("c.json"), CATALOG).expect("writes");
    let mut confined = Automation::with_root(root.path()).expect("root");
    assert!(
        call(&mut confined, json!({ "name": "x.json", "catalog": outside.path().join("c.json").to_str().expect("utf-8") })).is_err(),
        "outside the root"
    );
}
