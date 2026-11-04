//! `xml_load_schema`: the schema or DTD an XML document names, found as the app finds it.

use std::path::Path;

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    a.call("xml_load_schema", &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

fn write(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path.to_str().expect("utf-8").to_owned()
}

const XSD: &str = "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"><xs:element name=\"r\" type=\"xs:string\"/></xs:schema>";
const XSI: &str = "xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"";

#[test]
fn a_schema_location_next_to_the_document_is_found_and_can_be_opened() {
    let dir = tempfile::tempdir().expect("dir");
    write(dir.path(), "r.xsd", XSD);
    let doc = write(dir.path(), "r.xml", &format!("<r {XSI} xsi:noNamespaceSchemaLocation=\"r.xsd\">x</r>"));
    let mut a = Automation::new();
    let found = call(&mut a, json!({ "path": doc })).expect("found");
    assert_eq!(found["origin"], "a file");
    assert_eq!(found["text"], XSD);
    assert_eq!(found["link"]["kind"], "schema_location");
    assert!(found.get("document").is_none(), "nothing is opened unless asked");
    let opened = call(&mut a, json!({ "path": doc, "open": true })).expect("found");
    assert_eq!(opened["document"]["name"], "r.xsd");
    assert!(opened["document"]["path"].as_str().is_some_and(|p| p.ends_with("r.xsd")), "a file keeps its path: {opened}");
    assert_eq!(opened["already_open"], false);
    let again = call(&mut a, json!({ "path": doc, "open": true })).expect("found");
    assert_eq!(again["already_open"], true, "the open document is reused");
}

#[test]
fn a_doctype_names_its_dtd() {
    let dir = tempfile::tempdir().expect("dir");
    write(dir.path(), "r.dtd", "<!ELEMENT r EMPTY>");
    let mut a = Automation::new();
    let found = call(&mut a, json!({ "path": write(dir.path(), "r.xml", "<!DOCTYPE r SYSTEM \"r.dtd\"><r/>") })).expect("found");
    assert_eq!(found["link"]["kind"], "doctype");
    assert_eq!(found["text"], "<!ELEMENT r EMPTY>");
}

#[test]
fn a_document_that_names_nothing_or_whose_grammar_is_missing_fails_saying_why() {
    let dir = tempfile::tempdir().expect("dir");
    let mut a = Automation::new();
    let nothing = call(&mut a, json!({ "text": "<r xmlns=\"urn:r\"/>" })).expect_err("names nothing").to_string();
    assert!(nothing.contains("names no schema"), "{nothing}");
    let missing = call(&mut a, json!({ "path": write(dir.path(), "m.xml", &format!("<r {XSI} xsi:noNamespaceSchemaLocation=\"none.xsd\"/>")) }))
        .expect_err("missing")
        .to_string();
    assert!(missing.contains("none.xsd"), "{missing}");
}
