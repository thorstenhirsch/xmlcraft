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

const XSD: &str = "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\" targetNamespace=\"urn:o\" elementFormDefault=\"qualified\">\
    <xs:element name=\"order\"><xs:complexType><xs:sequence>\
      <xs:element name=\"customer\" type=\"xs:string\"/>\
      <xs:element name=\"state\"><xs:simpleType><xs:restriction base=\"xs:string\"><xs:enumeration value=\"open\"/><xs:enumeration value=\"closed\"/></xs:restriction></xs:simpleType></xs:element>\
      <xs:element name=\"item\" minOccurs=\"0\" maxOccurs=\"unbounded\"><xs:complexType><xs:attribute name=\"sku\" type=\"xs:string\" use=\"required\"/>\
        <xs:attribute name=\"kind\"><xs:simpleType><xs:restriction base=\"xs:string\"><xs:enumeration value=\"pen\"/><xs:enumeration value=\"ink\"/></xs:restriction></xs:simpleType></xs:attribute></xs:complexType></xs:element>\
    </xs:sequence></xs:complexType></xs:element></xs:schema>";

const XSI: &str = "xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"";

fn xsd_document(inside: &str) -> String {
    format!("<order xmlns=\"urn:o\" {XSI} xsi:schemaLocation=\"urn:o order.xsd\">{inside}</order>")
}

fn with_schema() -> (tempfile::TempDir, Automation) {
    let dir = tempfile::tempdir().expect("dir");
    std::fs::write(dir.path().join("order.xsd"), XSD).expect("writes");
    let a = Automation::with_root(dir.path()).expect("a root");
    (dir, a)
}

#[test]
fn a_document_that_names_a_schema_is_answered_from_it() {
    let (dir, mut a) = with_schema();
    let doc = dir.path().join("doc.xml");
    let text = xsd_document("<customer>c</customer><state>open</state></order>").replace("</order></order>", "</order>");
    std::fs::write(&doc, &text).expect("writes");
    let at = text.find("<customer>").expect("a place");
    let first = call(&mut a, json!({ "path": path_str(&doc), "offset": at })).expect("runs");
    assert_eq!((first["grammar"].clone(), first["kind"].clone()), (json!(true), json!("xsd")), "{first}");
    assert_eq!(names(&first["elements"]), [("customer".to_owned(), true)], "{first}");
    assert_eq!(first["elements"][0]["text"], "<customer></customer>", "{first}");
    assert_eq!(first["parent"], "order");
    // After the two elements the content may go on with items, and the text says what an item needs.
    let after = text.find("</order>").expect("the end");
    let second = call(&mut a, json!({ "path": path_str(&doc), "offset": after })).expect("runs");
    assert_eq!(names(&second["elements"]), [("item".to_owned(), false)], "{second}");
    assert_eq!(second["elements"][0]["text"], "<item sku=\"\"/>", "{second}");
    assert_eq!(second["elements"][0]["declare"], Value::Null, "the namespace is the default one already: nothing to declare");
}

#[test]
fn the_attributes_and_the_values_of_a_schema_type_are_listed() {
    let (dir, mut a) = with_schema();
    let doc = dir.path().join("doc.xml");
    let text = xsd_document("<customer/><state/><item sku=\"1\" /></order>").replace("</order></order>", "</order>");
    std::fs::write(&doc, &text).expect("writes");
    let at = text.find(" />").map(|i| i + 1).expect("a place");
    let result = call(&mut a, json!({ "path": path_str(&doc), "offset": at })).expect("runs");
    assert_eq!((result["element"].clone(), result["in_start_tag"].clone()), (json!("item"), json!(true)), "{result}");
    let attrs = result["attributes"].as_array().expect("attributes");
    let sku = attrs.iter().find(|t| t["name"] == "sku").expect("sku");
    let kind = attrs.iter().find(|t| t["name"] == "kind").expect("kind");
    assert_eq!((sku["required"].clone(), sku["used"].clone()), (json!(true), json!(true)));
    assert_eq!((kind["used"].clone(), kind["values"].clone()), (json!(false), json!(["pen", "ink"])));
    // The text of an element with an enumerated type lists its values.
    let in_state = text.find("<state/>").map(|i| i + "<state>".len() - 1).expect("in state");
    let with_content = text.replace("<state/>", "<state></state>");
    let values = call(&mut a, json!({ "text": with_content, "offset": in_state + 1 })).expect("runs");
    assert_eq!(values["element"], "state", "{values}");
    assert_eq!(values["values"], json!(["open", "closed"]), "{values}");
}

#[test]
fn a_schema_being_edited_is_what_the_answer_comes_from() {
    let (dir, mut a) = with_schema();
    let doc = dir.path().join("doc.xml");
    let text = xsd_document("</order>").replace("</order></order>", "</order>");
    std::fs::write(&doc, &text).expect("writes");
    let schema = dir.path().join("order.xsd");
    // The schema is open, with an edit that is not saved: an element renamed.
    let opened = a.call("doc_open", &json!({ "path": path_str(&schema) })).expect("opens");
    let id = match opened.into_iter().next() {
        Some(Content::Json(v)) => v["id"].clone(),
        None => Value::Null,
    };
    a.call("doc_set_text", &json!({ "id": id, "text": XSD.replace("name=\"customer\"", "name=\"buyer\"") })).expect("edits");
    let at = text.find("</order>").expect("the end");
    let result = call(&mut a, json!({ "path": path_str(&doc), "offset": at })).expect("runs");
    assert_eq!(names(&result["elements"]), [("buyer".to_owned(), true)], "{result}");
}

#[test]
fn a_schema_that_cannot_be_read_or_is_outside_the_root_says_so() {
    let (dir, mut a) = with_schema();
    let missing = format!("<order xmlns=\"urn:o\" {XSI} xsi:schemaLocation=\"urn:o gone.xsd\"></order>");
    let result = call(&mut a, json!({ "text": missing, "offset": missing.find("</order>").expect("a place") })).expect("runs");
    assert_eq!(result["grammar"], false, "{result}");
    assert!(result["why"].as_str().is_some_and(|w| w.contains("gone.xsd")), "{result}");
    // A schema outside the root is not read, whoever names it.
    let elsewhere = tempfile::tempdir().expect("dir");
    std::fs::write(elsewhere.path().join("order.xsd"), XSD).expect("writes");
    let named = format!("<order xmlns=\"urn:o\" {XSI} xsi:schemaLocation=\"urn:o {}\"></order>", path_str(&elsewhere.path().join("order.xsd")));
    let outside = call(&mut a, json!({ "text": named, "offset": named.find("</order>").expect("a place") })).expect("runs");
    assert_eq!(outside["grammar"], false, "the schema outside the root was read: {outside}");
    drop(dir);
}

#[test]
fn a_doctype_wins_over_a_schema_the_document_also_names() {
    let (_dir, mut a) = with_schema();
    let text = format!(
        "<!DOCTYPE order [<!ELEMENT order (z)><!ELEMENT z EMPTY>]><order xmlns=\"urn:o\" {XSI} xsi:schemaLocation=\"urn:o order.xsd\"></order>"
    );
    let result = call(&mut a, json!({ "text": text, "offset": text.find("</order>").expect("a place") })).expect("runs");
    assert_eq!(result["kind"], "dtd", "{result}");
}

#[test]
fn what_an_element_requires_is_written_with_it_unless_that_is_asked_not_to_be() {
    let dir = tempfile::tempdir().expect("dir");
    std::fs::write(
        dir.path().join("r.xsd"),
        "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"><xs:element name=\"r\"><xs:complexType><xs:sequence>\
           <xs:element name=\"box\"><xs:complexType><xs:sequence><xs:element name=\"label\" type=\"xs:string\"/></xs:sequence></xs:complexType></xs:element>\
         </xs:sequence></xs:complexType></xs:element></xs:schema>",
    )
    .expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("a root");
    let text = format!("<r {XSI} xsi:noNamespaceSchemaLocation=\"r.xsd\"></r>");
    let at = text.find("</r>").expect("a place");
    let full = call(&mut a, json!({ "text": text, "offset": at })).expect("runs");
    assert_eq!(full["elements"][0]["text"], "<box><label></label></box>", "{full}");
    let bare = call(&mut a, json!({ "text": text, "offset": at, "required_content": false })).expect("runs");
    assert_eq!(bare["elements"][0]["text"], "<box></box>", "{bare}");
}

#[test]
fn a_namespace_declaration_a_name_needs_is_said_with_it() {
    let dir = tempfile::tempdir().expect("dir");
    // Unqualified local elements: under a default namespace they need `xmlns=""`; qualified attributes need a prefix.
    std::fs::write(
        dir.path().join("q.xsd"),
        "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\" targetNamespace=\"urn:q\" attributeFormDefault=\"qualified\">\
           <xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"local\" type=\"xs:string\"/></xs:sequence>\
             <xs:attribute name=\"a\" type=\"xs:string\"/></xs:complexType></xs:element></xs:schema>",
    )
    .expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("a root");
    let text = format!("<r xmlns=\"urn:q\" {XSI} xsi:schemaLocation=\"urn:q q.xsd\"></r>");
    let inside = call(&mut a, json!({ "text": text, "offset": text.find("</r>").expect("a place") })).expect("runs");
    assert_eq!(inside["elements"][0]["name"], "local", "{inside}");
    assert_eq!(inside["elements"][0]["declare"], json!({ "prefix": null, "namespace": "" }), "{inside}");
    assert_eq!(inside["elements"][0]["text"], "<local xmlns=\"\"></local>", "{inside}");
    let attribute = call(&mut a, json!({ "text": text, "offset": text.find("></r>").expect("the end of the start tag") })).expect("runs");
    assert_eq!(attribute["attributes"][0]["name"], "ns1:a", "{attribute}");
    assert_eq!(attribute["attributes"][0]["declare"], json!({ "prefix": "ns1", "namespace": "urn:q" }), "{attribute}");
}
