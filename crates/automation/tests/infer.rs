//! `xml_infer`: a schema generated from documents, read through the root confinement.

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
    call(a, "xml_infer", args)
}

fn write(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path.to_str().expect("utf-8").to_owned()
}

fn text_of(result: &Value, index: usize) -> String {
    result["files"][index]["text"].as_str().unwrap_or_default().to_owned()
}

#[test]
fn text_gives_a_schema_that_names_what_it_saw() {
    let mut a = Automation::new();
    let result = infer(&mut a, json!({ "text": "<order id=\"7\"><item>Tea</item></order>" })).expect("runs");
    assert_eq!((result["documents"].clone(), result["files"].as_array().map(Vec::len)), (json!(1), Some(1)), "{result}");
    assert_eq!(result["files"][0]["name"], "schema.xsd");
    let text = text_of(&result, 0);
    assert!(
        text.contains("<xs:element name=\"order\">") && text.contains("<xs:attribute name=\"id\" use=\"required\" type=\"xs:integer\"/>"),
        "{text}"
    );
}

#[test]
fn several_documents_are_learned_together_from_files_and_open_documents() {
    let dir = tempfile::tempdir().expect("dir");
    let one = write(dir.path(), "one.xml", "<r><a/></r>");
    write(dir.path(), "two.xml", "<r><a/><b/></r>");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let opened = call(&mut a, "doc_open", json!({ "path": "two.xml" })).expect("opens");
    let result = infer(&mut a, json!({ "sources": [{ "path": one }, { "id": opened["id"] }, { "text": "<r><a/><a/></r>" }] })).expect("runs");
    assert_eq!(result["documents"], 3);
    let text = text_of(&result, 0);
    assert!(text.contains("<xs:element name=\"a\" maxOccurs=\"unbounded\">") && text.contains("<xs:element name=\"b\" minOccurs=\"0\">"), "{text}");
}

#[test]
fn the_options_choose_the_kind_and_the_layout() {
    let mut a = Automation::new();
    let doc = "<r b=\"true\"><a kind=\"x\"/><a kind=\"y\"/><a kind=\"x\"/><a kind=\"y\"/></r>";
    let dtd = infer(&mut a, json!({ "text": doc, "output": "dtd" })).expect("runs");
    assert_eq!(dtd["files"][0]["name"], "schema.dtd");
    assert!(text_of(&dtd, 0).contains("<!ELEMENT r (a+)>") && text_of(&dtd, 0).contains("kind (x|y) #REQUIRED"), "{}", text_of(&dtd, 0));
    let venetian = infer(&mut a, json!({ "text": doc, "layout": "venetian-blind" })).expect("runs");
    assert!(text_of(&venetian, 0).contains("<xs:complexType name=\"rType\">"), "{}", text_of(&venetian, 0));
    let plain = infer(&mut a, json!({ "text": doc, "types": false, "enumerations": false, "unbounded": false })).expect("runs");
    let text = text_of(&plain, 0);
    assert!(!text.contains("xs:boolean") && !text.contains("enumeration") && text.contains("maxOccurs=\"4\""), "{text}");
    let tight = infer(&mut a, json!({ "text": doc, "enumerations": { "max_values": 2, "min_occurrences": 3 } })).expect("runs");
    assert!(!text_of(&tight, 0).contains("enumeration"), "x and y are seen twice, not three times");
}

#[test]
fn a_document_that_is_not_well_formed_is_named() {
    let mut a = Automation::new();
    let err = infer(&mut a, json!({ "sources": [{ "text": "<a/>" }, { "text": "<a><b></a>" }] })).expect_err("the second is broken");
    assert!(matches!(&err, ToolError::Failed(m) if m.starts_with("Document 2 is not well-formed")), "{err:?}");
}

#[test]
fn the_arguments_are_checked() {
    let mut a = Automation::new();
    for args in [
        json!({}),
        json!({ "text": "<a/>", "path": "x.xml" }),
        json!({ "sources": [], }),
        json!({ "sources": [{ "text": "<a/>" }], "text": "<a/>" }),
        json!({ "text": "<a/>", "layout": "ziggurat" }),
        json!({ "text": "<a/>", "output": "json-schema" }),
        json!({ "text": "<a/>", "enumerations": "many" }),
        json!({ "text": "<a/>", "enumerations": { "max_values": 0 } }),
        json!({ "text": "<a/>", "surprise": true }),
    ] {
        let err = infer(&mut a, args.clone()).expect_err(&args.to_string());
        assert!(matches!(err, ToolError::InvalidArgs(_)), "{args}: {err:?}");
    }
}

#[test]
fn nothing_outside_the_root_is_read() {
    let outside = tempfile::tempdir().expect("outside");
    let inside = tempfile::tempdir().expect("inside");
    let secret = write(outside.path(), "secret.xml", "<secret/>");
    let mut a = Automation::with_root(inside.path()).expect("root");
    let err = infer(&mut a, json!({ "path": secret })).expect_err("outside the root");
    assert!(err.to_string().contains("outside") || err.to_string().contains("root"), "{err}");
    let err = infer(&mut a, json!({ "sources": [{ "text": "<a/>" }, { "path": secret }] })).expect_err("outside the root");
    assert!(err.to_string().contains("outside") || err.to_string().contains("root"), "{err}");
}

#[test]
fn what_is_generated_validates_its_source_through_the_other_tool() {
    let dir = tempfile::tempdir().expect("dir");
    let doc = "<order id=\"7\"><item sku=\"a\">Tea</item><item sku=\"b\">Coffee</item></order>";
    write(dir.path(), "order.xml", doc);
    let mut a = Automation::with_root(dir.path()).expect("root");
    let result = infer(&mut a, json!({ "path": "order.xml" })).expect("runs");
    write(dir.path(), "order.xsd", &text_of(&result, 0));
    let checked = call(&mut a, "xml_validate", json!({ "path": "order.xml", "schema": "order.xsd" })).expect("runs");
    assert_eq!(
        (checked["valid"].clone(), checked["errors"].clone(), checked["schema_errors"].clone()),
        (json!(true), json!(0), json!(0)),
        "{checked}"
    );
    // And it is a schema: another document of the same shape passes, one of another shape does not.
    write(dir.path(), "more.xml", "<order id=\"8\"><item sku=\"c\">Milk</item></order>");
    let other = call(&mut a, "xml_validate", json!({ "path": "more.xml", "schema": "order.xsd" })).expect("runs");
    assert_eq!(other["valid"], json!(true), "{other}");
    write(dir.path(), "wrong.xml", "<order id=\"x\"><thing/></order>");
    let wrong = call(&mut a, "xml_validate", json!({ "path": "wrong.xml", "schema": "order.xsd" })).expect("runs");
    assert_eq!(wrong["valid"], json!(false), "{wrong}");
}
