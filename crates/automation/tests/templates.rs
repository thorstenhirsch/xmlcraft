//! The templates for a new document, checked with the tools: what a template promises is true of its text.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};
use xmlcraft_engine::templates::{TEMPLATES, template};

fn validate(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    a.call("xml_validate", &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

fn text_of(id: &str) -> &'static str {
    template(id).unwrap_or_else(|| panic!("no template {id}")).text
}

#[test]
fn the_document_with_a_dtd_is_valid_as_it_stands_and_the_dtd_means_something() {
    let mut a = Automation::new();
    let ok = validate(&mut a, json!({ "text": text_of("xml-dtd") })).expect("runs");
    assert_eq!((ok["well_formed"].clone(), ok["valid"].clone(), ok["errors"].clone()), (json!(true), json!(true), json!(0)), "{ok}");
    // The DTD it declares is the one it is checked against: an element it does not declare is found.
    let bad = text_of("xml-dtd").replace("<item id=\"first\">One</item>", "<item id=\"first\">One</item><other/>");
    let found = validate(&mut a, json!({ "text": bad })).expect("runs");
    assert_eq!(found["valid"], false, "{found}");
}

#[test]
fn the_schema_is_a_valid_xml_schema_and_a_document_of_it_is_valid() {
    let mut a = Automation::new();
    // Checked against the schema for schemas that ships with the app (found by its namespace).
    let ok = validate(&mut a, json!({ "text": text_of("xsd"), "schema": "http://www.w3.org/2001/XMLSchema" })).expect("runs");
    assert_eq!((ok["kind"].clone(), ok["valid"].clone(), ok["errors"].clone()), (json!("xsd"), json!(true), json!(0)), "{ok}");
    // The check means something: a misspelt element is found.
    let broken = text_of("xsd").replace("xs:sequence", "xs:sequenz");
    let found = validate(&mut a, json!({ "text": broken, "schema": "http://www.w3.org/2001/XMLSchema" })).expect("runs");
    assert_eq!(found["valid"], false, "{found}");
    // And the schema, used for a document of its own, accepts what it describes.
    let dir = tempfile::tempdir().expect("dir");
    std::fs::write(dir.path().join("schema.xsd"), text_of("xsd")).expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let doc = "<document xmlns=\"urn:example:document\" id=\"d1\"><title>Hello</title></document>";
    let valid = validate(&mut a, json!({ "text": doc, "schema": dir.path().join("schema.xsd").to_str().expect("utf-8") })).expect("runs");
    assert_eq!((valid["valid"].clone(), valid["errors"].clone()), (json!(true), json!(0)), "{valid}");
    let invalid =
        validate(&mut a, json!({ "text": doc.replace(" id=\"d1\"", ""), "schema": dir.path().join("schema.xsd").to_str().expect("utf-8") }))
            .expect("runs");
    assert_eq!(invalid["valid"], false, "the id is required: {invalid}");
}

#[test]
fn every_template_that_is_xml_is_well_formed_for_the_tools_too() {
    let mut a = Automation::new();
    for t in TEMPLATES.iter().filter(|t| !t.name.ends_with(".dtd")) {
        let result = a.call("xml_wellformed", &json!({ "text": t.text })).expect("runs");
        let Some(Content::Json(v)) = result.into_iter().next() else { panic!("no answer for {}", t.id) };
        assert_eq!(v["well_formed"], true, "{}: {v}", t.id);
    }
}

#[test]
fn doc_new_starts_a_document_from_a_template_with_its_name_and_text() {
    let mut a = Automation::new();
    let result = |a: &mut Automation, args: Value| -> Result<Value, ToolError> {
        a.call("doc_new", &args).map(|c| match c.into_iter().next() {
            Some(Content::Json(v)) => v,
            None => Value::Null,
        })
    };
    let made = result(&mut a, json!({ "template": "xsd" })).expect("runs");
    assert_eq!(made["name"], template("xsd").expect("a template").name, "{made}");
    let text = a.call("doc_text", &json!({ "id": made["id"] })).expect("text");
    let Some(Content::Json(text)) = text.into_iter().next() else { panic!("no text") };
    assert_eq!(text["text"], text_of("xsd"));
    // A template and a text together, and a template that is not one, are said.
    assert!(result(&mut a, json!({ "template": "xsd", "text": "<a/>" })).expect_err("both").to_string().contains("not both"));
    let unknown = result(&mut a, json!({ "template": "nope" })).expect_err("unknown").to_string();
    assert!(unknown.contains("nope") && unknown.contains("xsd") && unknown.contains("catalog"), "the ones there are are listed: {unknown}");
    // A plain call still makes an empty document, and a text still a document with it.
    let plain = result(&mut a, json!({})).expect("runs");
    assert!(plain["name"].as_str().is_some_and(|n| n.starts_with("Untitled")), "{plain}");
    let with_text = result(&mut a, json!({ "text": "<a/>" })).expect("runs");
    let got = a.call("doc_text", &json!({ "id": with_text["id"] })).expect("text");
    let Some(Content::Json(got)) = got.into_iter().next() else { panic!("no text") };
    assert_eq!(got["text"], "<a/>");
}

#[test]
fn the_xhtml_page_is_valid_against_the_dtd_that_ships_with_the_app_and_a_wrong_one_is_not() {
    let mut a = Automation::new();
    let ok = validate(&mut a, json!({ "text": text_of("xhtml") })).expect("runs");
    assert_eq!((ok["well_formed"].clone(), ok["valid"].clone(), ok["errors"].clone()), (json!(true), json!(true), json!(0)), "{ok}");
    assert_eq!(ok["problems"], json!([]), "not even a note that the DTD was not read: {ok}");
    // The DTD is the one that was read: the body of XHTML Strict takes block elements, not text.
    let bad = text_of("xhtml").replace("<p>Text.</p>", "Text.");
    let found = validate(&mut a, json!({ "text": bad })).expect("runs");
    assert_eq!(found["valid"], false, "{found}");
}
