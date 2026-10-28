//! `xml_entry_helpers`: what the DTD of a document allows at a place.

use std::path::Path;

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    a.call("xml_entry_helpers", &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

fn names(list: &Value) -> Vec<(String, bool)> {
    list.as_array()
        .into_iter()
        .flatten()
        .map(|e| (e["name"].as_str().unwrap_or_default().to_owned(), e["required"].as_bool().unwrap_or(false)))
        .collect()
}

const DTD: &str = "<!DOCTYPE r [<!ELEMENT r (a, b?)><!ELEMENT a EMPTY><!ATTLIST a id ID #REQUIRED kind (p|q) #IMPLIED><!ELEMENT b (c)><!ELEMENT c (#PCDATA)>\
                   <!ATTLIST c x CDATA #REQUIRED><!ENTITY e 'text'>]>";

#[test]
fn the_elements_that_may_come_are_listed_with_the_text_that_would_be_put() {
    let mut a = Automation::new();
    let at = DTD.len() + "<r>".len();
    let first = call(&mut a, json!({ "text": format!("{DTD}<r></r>"), "offset": at })).expect("runs");
    assert_eq!(first["grammar"], true, "{first}");
    assert_eq!((first["parent"].clone(), first["element"].clone(), first["in_start_tag"].clone()), (json!("r"), json!("r"), json!(false)), "{first}");
    assert_eq!(names(&first["elements"]), [("a".to_owned(), true)], "{first}");
    assert_eq!(first["elements"][0]["text"], "<a id=\"\"/>", "{first}");
    // After a, b may come, and what b requires goes in with it unless that is asked not to be.
    let text = format!("{DTD}<r><a id='1'/></r>");
    let after = DTD.len() + "<r><a id='1'/>".len();
    let second = call(&mut a, json!({ "text": text, "offset": after })).expect("runs");
    assert_eq!(names(&second["elements"]), [("b".to_owned(), false)], "{second}");
    assert_eq!(second["elements"][0]["text"], "<b><c x=\"\"></c></b>", "{second}");
    let bare = call(&mut a, json!({ "text": text, "offset": after, "required_content": false })).expect("runs");
    assert_eq!(bare["elements"][0]["text"], "<b></b>", "{bare}");
}

#[test]
fn the_attributes_of_the_start_tag_say_which_are_given_and_where_a_new_one_goes() {
    let mut a = Automation::new();
    let text = format!("{DTD}<r><a id='1' /></r>");
    let at = text.find(" />").map(|i| i + 1).expect("a place");
    let result = call(&mut a, json!({ "text": text, "offset": at })).expect("runs");
    assert_eq!(result["element"], "a", "{result}");
    assert_eq!(result["in_start_tag"], true);
    let attrs = result["attributes"].as_array().expect("attributes");
    let id = attrs.iter().find(|t| t["name"] == "id").expect("id");
    let kind = attrs.iter().find(|t| t["name"] == "kind").expect("kind");
    assert_eq!((id["required"].clone(), id["used"].clone()), (json!(true), json!(true)));
    assert_eq!((kind["required"].clone(), kind["used"].clone(), kind["values"].clone()), (json!(false), json!(false), json!(["p", "q"])));
    assert_eq!(result["attribute_at"], json!(text.find(" />").expect("end of the last attribute")));
}

#[test]
fn the_entities_that_can_be_referred_to_come_with_the_reference() {
    let mut a = Automation::new();
    let text = format!("{DTD}<r><a id='1'/><b><c x='1'></c></b></r>");
    let at = text.find("</c>").expect("in c");
    let result = call(&mut a, json!({ "text": text, "line": 1, "column": text[..at].chars().count() + 1 })).expect("runs");
    assert_eq!(result["entities"], json!([{ "name": "e", "value": "text", "text": "&e;" }]), "{result}");
}

#[test]
fn a_document_without_a_usable_doctype_has_nothing_to_offer() {
    let mut a = Automation::new();
    let none = call(&mut a, json!({ "text": "<r/>", "offset": 3 })).expect("runs");
    assert_eq!(none["grammar"], false, "{none}");
    assert!(none["why"].as_str().is_some_and(|w| w.contains("no DOCTYPE")), "{none}");
    let big = format!("<!DOCTYPE r [<!--{}--><!ELEMENT r ANY>]><r/>", " ".repeat(xmlcraft_engine::schema::MAX_DOCTYPE_BYTES));
    let too_big = call(&mut a, json!({ "text": big, "offset": 0 })).expect("runs");
    assert_eq!(too_big["grammar"], false, "{too_big}");
}

#[test]
fn the_external_subset_is_a_file_next_to_the_document() {
    let dir = tempfile::tempdir().expect("dir");
    std::fs::write(dir.path().join("r.dtd"), "<!ELEMENT r (x, y)><!ELEMENT x EMPTY><!ELEMENT y EMPTY>").expect("writes");
    let doc = dir.path().join("doc.xml");
    let text = "<!DOCTYPE r SYSTEM \"r.dtd\"><r><x/></r>";
    std::fs::write(&doc, text).expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("a root");

    let at = text.find("</r>").expect("a place");
    let result = call(&mut a, json!({ "path": path_str(&doc), "offset": at })).expect("runs");
    assert_eq!(names(&result["elements"]), [("y".to_owned(), true)], "{result}");
    // Outside the root nothing is read.
    let elsewhere = tempfile::tempdir().expect("dir");
    std::fs::write(elsewhere.path().join("r.dtd"), "<!ELEMENT r ANY>").expect("writes");
    let outside = elsewhere.path().join("doc.xml");
    std::fs::write(&outside, text).expect("writes");
    assert!(call(&mut a, json!({ "path": path_str(&outside), "offset": at })).is_err(), "the document is outside the root");
    // A DTD outside the root is not read either, whoever names it.
    let named = format!("<!DOCTYPE r SYSTEM \"{}\"><r></r>", path_str(&elsewhere.path().join("r.dtd")));
    let result = call(&mut a, json!({ "text": named, "offset": named.find("</r>").expect("a place") })).expect("runs");
    assert_eq!(result["elements"], json!([]), "the DTD outside the root was read: {result}");
}

fn path_str(path: &Path) -> String {
    path.to_str().expect("utf-8").to_owned()
}

#[test]
fn a_position_that_is_not_in_the_text_is_an_error() {
    let mut a = Automation::new();
    let err = call(&mut a, json!({ "text": DTD, "offset": 10_000_000 })).expect_err("outside the text");
    assert!(err.to_string().contains("offset"), "{err}");
}
