//! `bundled_schemas`: the XML Schemas and DTDs that ship with XmlCraft, and assigning one with `xml_assign_schema`.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, tool: &str, args: Value) -> Result<Value, ToolError> {
    a.call(tool, &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

#[test]
fn the_bundled_schemas_are_listed_with_the_location_set_and_target_namespace() {
    let mut a = Automation::new();
    let listed = call(&mut a, "bundled_schemas", json!({})).expect("runs");
    assert!(listed["total"].as_u64().is_some_and(|n| n >= 30), "{listed}");
    assert_eq!(listed["total"], listed["matched"]);
    let schemas = listed["schemas"].as_array().expect("a list");
    let xml_schema = schemas.iter().find(|s| s["path"] == "w3c/xmlschema/XMLSchema.xsd").expect("the schema for schemas");
    assert_eq!(xml_schema["uri"], "bundled:/w3c/xmlschema/XMLSchema.xsd");
    assert_eq!(xml_schema["name"], "XMLSchema.xsd");
    assert_eq!(xml_schema["set"], "w3c");
    assert_eq!(xml_schema["format"], "xsd");
    assert_eq!(xml_schema["namespace"], "http://www.w3.org/2001/XMLSchema");
    assert_eq!(xml_schema["location"], "http://www.w3.org/2001/XMLSchema.xsd");
    let xhtml = schemas.iter().find(|s| s["path"] == "w3c/xhtml1/xhtml1-strict.dtd").expect("the XHTML DTD");
    assert_eq!(xhtml["format"], "dtd");
    assert_eq!(xhtml["namespace"], Value::Null);
    assert_eq!(
        (xhtml["location"].clone(), xhtml["public_id"].clone()),
        (json!("http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd"), json!("-//W3C//DTD XHTML 1.0 Strict//EN"))
    );
}

#[test]
fn a_query_and_a_format_keep_only_what_is_asked_for() {
    let mut a = Automation::new();
    let found = call(&mut a, "bundled_schemas", json!({ "query": "XHTML1-STRICT" })).expect("runs");
    assert_eq!(found["matched"], 1, "{found}");
    assert_eq!(found["schemas"][0]["name"], "xhtml1-strict.dtd");
    let xsd = call(&mut a, "bundled_schemas", json!({ "format": "xsd", "query": "UBL" })).expect("runs");
    let schemas = xsd["schemas"].as_array().expect("a list");
    assert!(!schemas.is_empty());
    assert!(schemas.iter().all(|s| s["format"] == "xsd"));
    assert!(schemas.iter().all(|s| s["set"] == "ubl"));
    let none = call(&mut a, "bundled_schemas", json!({ "query": "not a schema in the bundle" })).expect("runs");
    assert_eq!(none["matched"], 0);
    assert_eq!(none["count"], 0);
}

#[test]
fn a_bundled_xml_schema_is_assigned_with_the_namespace_read_from_the_bundle() {
    let mut a = Automation::new();
    let result = call(
        &mut a,
        "xml_assign_schema",
        json!({
            "text": "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"/>",
            "schema": "bundled:/w3c/xmlschema/XMLSchema.xsd",
        }),
    )
    .expect("runs");
    assert_eq!(result["namespace"], "http://www.w3.org/2001/XMLSchema");
    assert_eq!(result["namespace_read"], true);
    let text = result["text"].as_str().expect("text");
    assert!(
        text.contains("xsi:schemaLocation=\"http://www.w3.org/2001/XMLSchema http://www.w3.org/2001/XMLSchema.xsd\"") && !text.contains("bundled:"),
        "the public address is written, not the app's own URI: {text}"
    );
}

#[test]
fn a_bundled_file_known_only_by_its_namespace_is_not_written_into_a_document() {
    let mut a = Automation::new();
    let e = call(&mut a, "xml_assign_schema", json!({ "text": "<Invoice/>", "schema": "bundled:/ubl/maindoc/UBL-Invoice-2.1.xsd" }))
        .expect_err("refused");
    assert!(e.to_string().contains("no public address"), "{e}");
}

#[test]
fn a_bundled_dtd_is_assigned_and_then_validates_the_document_offline() {
    let dir = tempfile::tempdir().expect("dir");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let text = "<html><head><title>x</title></head><body><p>x</p></body></html>";
    let assigned = call(&mut a, "xml_assign_schema", json!({ "text": text, "schema": "bundled:/w3c/xhtml1/xhtml1-strict.dtd" })).expect("runs");
    let assigned = assigned["text"].as_str().expect("text");
    assert!(
        assigned.starts_with("<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \"http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd\">\n"),
        "{assigned}"
    );
    std::fs::write(dir.path().join("page.html"), assigned).expect("writes");
    let checked = call(&mut a, "xml_validate", json!({ "path": "page.html" })).expect("runs");
    assert_eq!((checked["valid"].clone(), checked["errors"].clone()), (json!(true), json!(0)), "{checked}");
}
