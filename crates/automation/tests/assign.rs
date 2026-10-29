//! `xml_assign_schema`: a DTD or a schema assigned to a document, as edits to an open one or as new text.

use std::path::Path;

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, tool: &str, args: Value) -> Result<Value, ToolError> {
    a.call(tool, &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

fn assign(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    call(a, "xml_assign_schema", args)
}

fn write(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path.to_str().expect("utf-8").to_owned()
}

const XSD: &str = "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\" targetNamespace=\"urn:o\" elementFormDefault=\"qualified\"><xs:element name=\"order\" type=\"xs:string\"/></xs:schema>";

#[test]
fn text_comes_back_with_a_doctype_or_the_attributes() {
    let mut a = Automation::new();
    let dtd = assign(&mut a, json!({ "text": "<order/>", "schema": "order.dtd" })).expect("runs");
    assert_eq!(dtd["text"], "<!DOCTYPE order SYSTEM \"order.dtd\">\n<order/>", "a .dtd is a DTD");
    let xsd = assign(&mut a, json!({ "text": "<order/>", "schema": "order.xsd" })).expect("runs");
    assert!(xsd["text"].as_str().is_some_and(|t| t.contains("xsi:noNamespaceSchemaLocation=\"order.xsd\"")), "{xsd}");
    let told = assign(&mut a, json!({ "text": "<order/>", "schema": "order.xml", "kind": "dtd" })).expect("runs");
    assert!(told["text"].as_str().is_some_and(|t| t.starts_with("<!DOCTYPE order SYSTEM \"order.xml\">")), "{told}");
    let ns = assign(&mut a, json!({ "text": "<o:order xmlns:o=\"urn:o\"/>", "schema": "o.xsd", "namespace": "urn:o" })).expect("runs");
    assert!(ns["text"].as_str().is_some_and(|t| t.contains("xsi:schemaLocation=\"urn:o o.xsd\"")), "{ns}");
}

#[test]
fn the_namespace_is_read_from_the_schema_when_it_is_a_file() {
    let dir = tempfile::tempdir().expect("dir");
    write(dir.path(), "order.xsd", XSD);
    let doc = write(dir.path(), "order.xml", "<order xmlns=\"urn:o\"/>");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let result = assign(&mut a, json!({ "path": doc, "schema": "order.xsd" })).expect("runs");
    assert_eq!((result["namespace"].clone(), result["namespace_read"].clone()), (json!("urn:o"), json!(true)), "{result}");
    assert!(result["text"].as_str().is_some_and(|t| t.contains("xsi:schemaLocation=\"urn:o order.xsd\"")), "{result}");
    // The file is not changed: the text is returned.
    assert_eq!(std::fs::read_to_string(dir.path().join("order.xml")).expect("reads"), "<order xmlns=\"urn:o\"/>");
    // The assigned schema then validates the result.
    write(dir.path(), "assigned.xml", result["text"].as_str().expect("text"));
    let checked = call(&mut a, "xml_validate", json!({ "path": "assigned.xml" })).expect("runs");
    assert_eq!((checked["valid"].clone(), checked["errors"].clone()), (json!(true), json!(0)), "{checked}");
}

#[test]
fn an_open_document_is_edited_in_one_step_that_can_be_undone() {
    let dir = tempfile::tempdir().expect("dir");
    let doc = write(dir.path(), "a.xml", "<?xml version=\"1.0\"?>\r\n<a>\r\n  <b/>\r\n</a>\r\n");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let opened = call(&mut a, "doc_open", json!({ "path": doc })).expect("opens");
    let result = assign(&mut a, json!({ "id": opened["id"], "schema": "a.dtd" })).expect("runs");
    assert_eq!(result["changed"], true, "{result}");
    let text = call(&mut a, "doc_text", json!({ "id": opened["id"] })).expect("text");
    assert_eq!(
        text["text"], "<?xml version=\"1.0\"?>\r\n<!DOCTYPE a SYSTEM \"a.dtd\">\r\n<a>\r\n  <b/>\r\n</a>\r\n",
        "the line endings are the document's"
    );
    call(&mut a, "doc_undo", json!({ "id": opened["id"] })).expect("undoes");
    let back = call(&mut a, "doc_text", json!({ "id": opened["id"] })).expect("text");
    assert_eq!(back["text"], "<?xml version=\"1.0\"?>\r\n<a>\r\n  <b/>\r\n</a>\r\n");
}

#[test]
fn what_cannot_be_assigned_is_refused_and_the_arguments_are_checked() {
    let mut a = Automation::new();
    let err = assign(&mut a, json!({ "text": "<a><b></a>", "schema": "a.xsd" })).expect_err("not well-formed");
    assert!(err.to_string().contains("not well-formed"), "{err}");
    let err = assign(&mut a, json!({ "text": "<!DOCTYPE a>\n<a/>", "schema": "a.xsd" })).expect_err("has a DOCTYPE");
    assert!(err.to_string().contains("DOCTYPE"), "{err}");
    let err = assign(&mut a, json!({ "text": "<a/>", "schema": "my schema.xsd" })).expect_err("a space");
    assert!(err.to_string().contains("space"), "{err}");
    for args in [
        json!({ "text": "<a/>" }),
        json!({ "text": "<a/>", "schema": "" }),
        json!({ "text": "<a/>", "schema": "a.xsd", "kind": "json" }),
        json!({ "schema": "a.xsd" }),
        json!({ "text": "<a/>", "schema": "a.xsd", "surprise": 1 }),
    ] {
        let err = assign(&mut a, args.clone()).expect_err(&args.to_string());
        assert!(matches!(err, ToolError::InvalidArgs(_)), "{args}: {err:?}");
    }
}

#[test]
fn a_schema_outside_the_root_is_not_read_for_its_namespace() {
    let outside = tempfile::tempdir().expect("outside");
    let inside = tempfile::tempdir().expect("inside");
    let secret = write(outside.path(), "secret.xsd", XSD);
    let mut a = Automation::with_root(inside.path()).expect("root");
    let result = assign(&mut a, json!({ "text": "<order/>", "schema": secret })).expect("runs");
    assert_eq!((result["namespace"].clone(), result["namespace_read"].clone()), (Value::Null, json!(false)), "{result}");
}

#[test]
fn kind_says_what_the_location_is_whatever_it_ends_in() {
    let mut a = Automation::new();
    let xsd = assign(&mut a, json!({ "text": "<order/>", "schema": "rules.dtd", "kind": "xsd" })).expect("runs");
    assert!(xsd["text"].as_str().is_some_and(|t| t.contains("xsi:noNamespaceSchemaLocation=\"rules.dtd\"") && !t.contains("DOCTYPE")), "{xsd}");
}

#[test]
fn the_namespace_that_is_told_is_not_read_from_the_schema() {
    let dir = tempfile::tempdir().expect("dir");
    write(dir.path(), "order.xsd", XSD);
    let doc = write(dir.path(), "order.xml", "<order xmlns=\"urn:other\"/>");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let result = assign(&mut a, json!({ "path": doc, "schema": "order.xsd", "namespace": "urn:other" })).expect("runs");
    assert_eq!((result["namespace"].clone(), result["namespace_read"].clone()), (json!("urn:other"), json!(false)), "{result}");
    assert!(result["text"].as_str().is_some_and(|t| t.contains("xsi:schemaLocation=\"urn:other order.xsd\"")), "{result}");
}

#[test]
fn a_relative_schema_is_looked_for_in_the_folder_of_the_document() {
    let dir = tempfile::tempdir().expect("dir");
    std::fs::create_dir(dir.path().join("docs")).expect("dir");
    std::fs::create_dir(dir.path().join("schemas")).expect("dir");
    // The schema is beside the document, not beside the root of the confinement, and there is another one there.
    write(dir.path(), "order.xsd", &XSD.replace("urn:o", "urn:wrong"));
    write(&dir.path().join("docs"), "order.xsd", XSD);
    let doc = write(&dir.path().join("docs"), "order.xml", "<order xmlns=\"urn:o\"/>");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let result = assign(&mut a, json!({ "path": doc, "schema": "order.xsd" })).expect("runs");
    assert_eq!(result["namespace"], "urn:o", "{result}");
}

#[test]
fn text_that_had_to_be_repaired_to_be_read_says_so() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("a.xml");
    std::fs::write(&path, b"<a>\xff</a>").expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let result = assign(&mut a, json!({ "path": path.to_str().expect("utf-8"), "schema": "a.dtd" })).expect("runs");
    assert_eq!(result["lossy"], true, "{result}");
    let clean = assign(&mut a, json!({ "text": "<a/>", "schema": "a.dtd" })).expect("runs");
    assert_eq!(clean["lossy"], false, "{clean}");
}

#[test]
fn assigning_what_is_assigned_already_changes_nothing_and_leaves_no_step() {
    let dir = tempfile::tempdir().expect("dir");
    let doc = write(dir.path(), "a.xml", "<a/>");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let opened = call(&mut a, "doc_open", json!({ "path": doc })).expect("opens");
    let first = assign(&mut a, json!({ "id": opened["id"], "schema": "a.dtd" })).expect("runs");
    assert_eq!((first["changed"].clone(), first["edits"].clone()), (json!(true), json!(1)), "{first}");
    let again = assign(&mut a, json!({ "id": opened["id"], "schema": "a.dtd" })).expect("runs");
    assert_eq!((again["changed"].clone(), again["edits"].clone()), (json!(false), json!(0)), "{again}");
    // One undo takes the first assignment back: the second left no step of its own.
    call(&mut a, "doc_undo", json!({ "id": opened["id"] })).expect("undoes");
    let text = call(&mut a, "doc_text", json!({ "id": opened["id"] })).expect("text");
    assert_eq!(text["text"], "<a/>");
    // For a text the answer is the same.
    let text_only = assign(&mut a, json!({ "text": "<!DOCTYPE a SYSTEM \"a.dtd\">\n<a/>", "schema": "a.dtd" })).expect("runs");
    assert_eq!((text_only["changed"].clone(), text_only["text"].clone()), (json!(false), json!("<!DOCTYPE a SYSTEM \"a.dtd\">\n<a/>")));
}
