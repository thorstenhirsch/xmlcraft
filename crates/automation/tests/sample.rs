//! `xml_sample`: a document generated from a schema or a DTD, read through the root confinement.

use std::path::Path;

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

const XS: &str = "http://www.w3.org/2001/XMLSchema";

fn call(a: &mut Automation, tool: &str, args: Value) -> Result<Value, ToolError> {
    a.call(tool, &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

fn sample(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    call(a, "xml_sample", args)
}

fn write(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path.to_str().expect("utf-8").to_owned()
}

fn text_of(result: &Value) -> String {
    result["text"].as_str().unwrap_or_default().to_owned()
}

const ORDER_XSD: &str = "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"><xs:element name=\"order\"><xs:complexType><xs:sequence>\
    <xs:element name=\"id\" type=\"xs:int\"/><xs:element name=\"item\" type=\"xs:string\" maxOccurs=\"unbounded\"/></xs:sequence>\
    <xs:attribute name=\"currency\" type=\"xs:string\" use=\"required\"/></xs:complexType></xs:element></xs:schema>";

#[test]
fn a_schema_file_gives_a_document_that_the_validation_tool_accepts() {
    let dir = tempfile::tempdir().expect("dir");
    let schema = write(dir.path(), "order.xsd", ORDER_XSD);
    let mut a = Automation::with_root(dir.path()).expect("root");
    let result = sample(&mut a, json!({ "path": schema, "repeat": 2 })).expect("runs");
    assert_eq!((result["kind"].clone(), result["root"].clone(), result["warnings"].clone()), (json!("xsd"), json!("order"), json!([])), "{result}");
    assert_eq!(result["roots"], json!(["order"]));
    let text = text_of(&result);
    assert!(text.contains("<order currency=\"string\">") && text.matches("<item>").count() == 2, "{text}");
    write(dir.path(), "made.xml", &text);
    let checked = call(&mut a, "xml_validate", json!({ "path": "made.xml", "schema": "order.xsd" })).expect("runs");
    assert_eq!((checked["valid"].clone(), checked["errors"].clone()), (json!(true), json!(0)), "{checked}");
}

#[test]
fn the_options_shape_the_document() {
    let mut a = Automation::new();
    let plain = text_of(&sample(&mut a, json!({ "text": ORDER_XSD })).expect("runs"));
    let least = text_of(&sample(&mut a, json!({ "text": ORDER_XSD, "optional": false, "fill": "named" })).expect("runs"));
    assert!(plain.contains("<id>1</id>") && least.contains("<item>item</item>"), "{plain}\n{least}");
    let hinted = text_of(&sample(&mut a, json!({ "text": ORDER_XSD, "schema_location": "order.xsd" })).expect("runs"));
    assert!(hinted.contains("xsi:noNamespaceSchemaLocation=\"order.xsd\""), "{hinted}");
    let shallow = sample(&mut a, json!({ "text": ORDER_XSD, "max_depth": 0, "repeat": 0, "choice": "rotate" })).expect("runs");
    assert_eq!(shallow["warnings"], json!([]));
}

#[test]
fn a_root_is_chosen_by_name_with_or_without_a_namespace() {
    let mut a = Automation::new();
    let schema = format!(
        "<xs:schema xmlns:xs=\"{XS}\" xmlns:t=\"urn:t\" targetNamespace=\"urn:t\" elementFormDefault=\"qualified\"><xs:element name=\"a\" type=\"xs:string\"/><xs:element name=\"b\" type=\"xs:int\"/></xs:schema>"
    );
    let result = sample(&mut a, json!({ "text": schema, "root": "{urn:t}b" })).expect("runs");
    assert!(text_of(&result).contains("<ns1:b xmlns:ns1=\"urn:t\">1</ns1:b>"), "{result}");
    assert_eq!(result["roots"], json!(["{urn:t}a", "{urn:t}b"]));
    let err = sample(&mut a, json!({ "text": schema, "root": "b" })).expect_err("b is in a namespace");
    assert!(err.to_string().contains("not declared"), "{err}");
}

#[test]
fn a_dtd_is_read_by_its_text_and_gives_a_valid_document() {
    let dir = tempfile::tempdir().expect("dir");
    let dtd = "<!ELEMENT library (book+)>\n<!ELEMENT book (title, author+)>\n<!ATTLIST book id ID #REQUIRED>\n<!ELEMENT title (#PCDATA)>\n<!ELEMENT author (#PCDATA)>\n";
    let dtd_path = write(dir.path(), "library.dtd", dtd);
    let mut a = Automation::with_root(dir.path()).expect("root");
    for args in [json!({ "text": dtd, "kind": "dtd" }), json!({ "path": dtd_path })] {
        let result = sample(&mut a, args.clone()).expect("runs");
        assert_eq!((result["kind"].clone(), result["root"].clone()), (json!("dtd"), json!("library")), "{args}: {result}");
        let text = text_of(&result);
        assert!(text.contains("<library>") && text.contains("<book id=\"id1\">"), "{text}");
        // With the DOCTYPE the validation tool knows, it is valid.
        let at = text.find("?>").map(|i| i + 3).unwrap_or(0);
        write(dir.path(), "made.xml", &format!("{}<!DOCTYPE library SYSTEM \"library.dtd\">\n{}", &text[..at], &text[at..]));
        let checked = call(&mut a, "xml_validate", json!({ "path": "made.xml" })).expect("runs");
        assert_eq!((checked["valid"].clone(), checked["errors"].clone()), (json!(true), json!(0)), "{checked}");
    }
    let named = sample(&mut a, json!({ "text": dtd, "root": "book" })).expect("runs");
    assert!(text_of(&named).contains("<book id=\"id1\">"), "{named}");
}

#[test]
fn a_schema_open_and_being_edited_is_used_as_it_is() {
    let dir = tempfile::tempdir().expect("dir");
    let path = write(dir.path(), "s.xsd", ORDER_XSD);
    let mut a = Automation::with_root(dir.path()).expect("root");
    let opened = call(&mut a, "doc_open", json!({ "path": path })).expect("opens");
    call(&mut a, "doc_set_text", json!({ "id": opened["id"], "text": ORDER_XSD.replace("name=\"order\"", "name=\"edited\"") })).expect("edits");
    let result = sample(&mut a, json!({ "id": opened["id"] })).expect("runs");
    assert_eq!(result["root"], "edited", "{result}");
    // The file on disk still says order.
    assert_eq!(sample(&mut a, json!({ "path": path })).expect("runs")["root"], "order");
}

#[test]
fn a_schema_with_errors_or_nothing_to_make_is_refused_and_the_arguments_are_checked() {
    let mut a = Automation::new();
    let broken = format!("<xs:schema xmlns:xs=\"{XS}\"><xs:element name=\"r\" type=\"nothing\"/></xs:schema>");
    let err = sample(&mut a, json!({ "text": broken })).expect_err("the schema is broken");
    assert!(err.to_string().contains("error") && err.to_string().contains("nothing"), "{err}");
    let empty = format!("<xs:schema xmlns:xs=\"{XS}\"/>");
    assert!(sample(&mut a, json!({ "text": empty })).expect_err("no element").to_string().contains("no element"));
    let broken_dtd = sample(&mut a, json!({ "text": "<!ELEMENT a (b", "kind": "dtd" })).expect_err("the DTD is broken");
    assert!(broken_dtd.to_string().contains("error"), "{broken_dtd}");
    for args in [
        json!({}),
        json!({ "text": ORDER_XSD, "repeat": 101 }),
        json!({ "text": ORDER_XSD, "max_depth": 51 }),
        json!({ "text": ORDER_XSD, "choice": "random" }),
        json!({ "text": ORDER_XSD, "fill": "lorem" }),
        json!({ "text": ORDER_XSD, "kind": "json-schema" }),
        json!({ "text": ORDER_XSD, "surprise": 1 }),
    ] {
        let err = sample(&mut a, args.clone()).expect_err(&args.to_string());
        assert!(matches!(err, ToolError::InvalidArgs(_)), "{args}: {err:?}");
    }
}

#[test]
fn nothing_outside_the_root_is_read_not_even_through_an_include() {
    let outside = tempfile::tempdir().expect("outside");
    let inside = tempfile::tempdir().expect("inside");
    let secret = write(outside.path(), "secret.xsd", ORDER_XSD);
    let mut a = Automation::with_root(inside.path()).expect("root");
    assert!(sample(&mut a, json!({ "path": secret })).is_err());
    write(
        inside.path(),
        "main.xsd",
        &format!("<xs:schema xmlns:xs=\"{XS}\"><xs:include schemaLocation=\"{}\"/></xs:schema>", secret.replace('\\', "/")),
    );
    let err = sample(&mut a, json!({ "path": "main.xsd" })).expect_err("the include is outside");
    assert!(err.to_string().contains("error"), "{err}");
}
