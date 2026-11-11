//! `xsd_definition`: the type, element, group or attribute group an XML Schema name refers to.

use std::path::Path;

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    a.call("xsd_definition", &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

fn write(dir: &Path, name: &str, text: &str) -> String {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().expect("a folder")).expect("folder");
    std::fs::write(&path, text).expect("writes");
    path.to_str().expect("utf-8").to_owned()
}

const XS: &str = "http://www.w3.org/2001/XMLSchema";

#[test]
fn the_type_a_name_refers_to_is_found_by_offset_or_by_line_and_column() {
    let text = format!(
        r#"<xs:schema xmlns:xs="{XS}" targetNamespace="urn:t" xmlns:t="urn:t">
  <xs:element name="order" type="t:OrderType"/>
  <xs:complexType name="OrderType"><xs:sequence/></xs:complexType>
</xs:schema>"#
    );
    let mut a = Automation::new();
    let at = text.find("t:OrderType").expect("the name") + 2;
    let found = call(&mut a, json!({ "text": text, "offset": at })).expect("runs");
    assert_eq!(
        (found["found"].clone(), found["kind"].clone(), found["name"].clone()),
        (json!(true), json!("type"), json!("{urn:t}OrderType")),
        "{found}"
    );
    let (definition_line, definition_column) = (3, text.lines().nth(2).expect("line").find("OrderType").expect("name") + 1);
    assert_eq!((found["line"].clone(), found["column"].clone()), (json!(definition_line), json!(definition_column)), "{found}");
    let start = found["start"].as_u64().and_then(|n| usize::try_from(n).ok()).expect("start");
    let end = found["end"].as_u64().and_then(|n| usize::try_from(n).ok()).expect("end");
    assert_eq!(text.get(start..end), Some("OrderType"));
    // By the place of the reference.
    let (line, column) = (2, text.lines().nth(1).expect("line").find("t:OrderType").expect("name") + 3);
    let by_place = call(&mut a, json!({ "text": text, "line": line, "column": column })).expect("runs");
    assert_eq!((by_place["found"].clone(), by_place["name"].clone()), (json!(true), json!("{urn:t}OrderType")), "{by_place}");
}

#[test]
fn a_definition_in_an_included_document_names_that_file() {
    let dir = tempfile::tempdir().expect("dir");
    let types = write(
        dir.path(),
        "types.xsd",
        &format!(
            r#"<xs:schema xmlns:xs="{XS}" targetNamespace="urn:t"><xs:complexType name="OrderType"><xs:sequence/></xs:complexType></xs:schema>"#
        ),
    );
    let root = write(
        dir.path(),
        "root.xsd",
        &format!(
            r#"<xs:schema xmlns:xs="{XS}" targetNamespace="urn:t" xmlns:t="urn:t"><xs:include schemaLocation="types.xsd"/><xs:element name="order" type="t:OrderType"/></xs:schema>"#
        ),
    );
    let text = std::fs::read_to_string(&root).expect("reads the root");
    let at = text.find("t:OrderType").expect("the name") + 2;
    let mut a = Automation::new();
    let found = call(&mut a, json!({ "path": root, "offset": at })).expect("runs");
    assert_eq!((found["found"].clone(), found["kind"].clone()), (json!(true), json!("type")), "{found}");
    assert!(found["file"].as_str().is_some_and(|f| f.ends_with("types.xsd")), "{found}");
    let types_text = std::fs::read_to_string(&types).expect("reads the types");
    let start = found["start"].as_u64().and_then(|n| usize::try_from(n).ok()).expect("start");
    let end = found["end"].as_u64().and_then(|n| usize::try_from(n).ok()).expect("end");
    assert_eq!(types_text.get(start..end), Some("OrderType"), "the range is in the included file: {found}");
    let column = types_text.find("OrderType").expect("the name") + 1;
    assert_eq!((found["line"].clone(), found["column"].clone()), (json!(1), json!(column)), "{found}");
}

#[test]
fn a_definition_in_the_same_file_given_by_path_has_its_line_and_column_and_no_other_file() {
    let dir = tempfile::tempdir().expect("dir");
    let text = format!(
        "<xs:schema xmlns:xs=\"{XS}\" targetNamespace=\"urn:t\" xmlns:t=\"urn:t\">\n  <xs:element name=\"order\" type=\"t:OrderType\"/>\n  <xs:complexType name=\"OrderType\"><xs:sequence/></xs:complexType>\n</xs:schema>"
    );
    let path = write(dir.path(), "order.xsd", &text);
    let mut a = Automation::new();
    let found = call(&mut a, json!({ "path": path, "offset": text.find("t:OrderType").expect("the name") + 2 })).expect("runs");
    assert_eq!(found["found"], json!(true), "{found}");
    assert_eq!(found["file"], Value::Null, "{found}");
    let column = text.lines().nth(2).expect("line").find("OrderType").expect("the name") + 1;
    assert_eq!((found["line"].clone(), found["column"].clone()), (json!(3), json!(column)), "{found}");
}

#[test]
fn a_place_that_names_nothing_and_a_name_that_is_not_defined_are_not_found() {
    let text = format!(r#"<xs:schema xmlns:xs="{XS}"><xs:element name="a" type="missing"/></xs:schema>"#);
    let mut a = Automation::new();
    let missing = call(&mut a, json!({ "text": text, "offset": text.find("missing").expect("name") + 2 })).expect("runs");
    assert_eq!(missing["found"], json!(false), "{missing}");
    let root = call(&mut a, json!({ "text": text, "offset": text.find("<xs:schema").expect("root") })).expect("runs");
    assert_eq!(root["found"], json!(false), "{root}");
}

#[test]
fn a_language_that_is_not_xml_is_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let path = write(dir.path(), "not-a-schema.json", "{}");
    let mut a = Automation::new();
    let error = call(&mut a, json!({ "path": path, "offset": 0 })).expect_err("refused");
    assert!(error.to_string().contains("XML"), "{error}");
}
