//! `schema_resolution`: the places the resolver looked in for the schema or DTD a document names.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, tool: &str, args: Value) -> Result<Value, ToolError> {
    a.call(tool, &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

#[test]
fn a_bundled_schema_is_found_and_the_trail_says_which_place_had_it() {
    let mut a = Automation::new();
    let text = "<x xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"bundled:/w3c/xmlschema/XMLSchema.xsd\"/>";
    let result = call(&mut a, "schema_resolution", json!({ "text": text })).expect("runs");
    assert_eq!(result["found"], true, "{result}");
    assert_eq!(result["origin"], "the schemas that ship with the app");
    assert_eq!(result["location"], "bundled:/w3c/xmlschema/XMLSchema.xsd");
    let steps = result["steps"].as_array().expect("steps");
    assert!(steps.iter().any(|s| s["outcome"] == "found"), "{result}");
    assert!(result["trail"].as_str().is_some_and(|t| t.contains("bundled")));
}

#[test]
fn a_missing_schema_says_every_place_that_did_not_have_it() {
    let dir = tempfile::tempdir().expect("dir");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let text = "<x xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"missing.xsd\"/>";
    let result = call(&mut a, "schema_resolution", json!({ "text": text })).expect("runs");
    assert_eq!(result["found"], false, "{result}");
    assert!(result["message"].as_str().is_some_and(|m| m.contains("missing.xsd")), "{result}");
    let steps = result["steps"].as_array().expect("steps");
    assert!(steps.iter().any(|s| s["stage"] == "file" && s["outcome"] == "failed"), "{result}");
    assert!(steps.iter().any(|s| s["stage"] == "bundled catalog" && s["outcome"] == "missed"), "{result}");
}

#[test]
fn a_document_that_names_no_grammar_is_an_error_with_a_reason() {
    let mut a = Automation::new();
    let error = call(&mut a, "schema_resolution", json!({ "text": "<x/>" })).expect_err("no link");
    assert!(error.to_string().contains("names no schema or DTD"), "{error}");
}
