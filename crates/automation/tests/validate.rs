//! `xml_validate`: a document against its DTD, the external parts read from files through the root confinement.

use std::path::Path;

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    a.call("xml_validate", &args).map(|content| match content.into_iter().next() {
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

fn codes(result: &Value) -> Vec<String> {
    result["problems"].as_array().into_iter().flatten().filter_map(|p| p["code"].as_str().map(str::to_owned)).collect()
}

#[test]
fn a_document_is_checked_against_its_internal_subset() {
    let mut a = Automation::new();
    let valid = call(&mut a, json!({ "text": "<!DOCTYPE r [<!ELEMENT r (a+)><!ELEMENT a EMPTY>]><r><a/><a/></r>" })).expect("runs");
    assert_eq!((valid["well_formed"].clone(), valid["valid"].clone(), valid["errors"].clone()), (json!(true), json!(true), json!(0)), "{valid}");
    let text = "<!DOCTYPE r [<!ELEMENT r (a+)><!ELEMENT a EMPTY>]>\n<r>\n  <b/>\n</r>";
    let invalid = call(&mut a, json!({ "text": text })).expect("runs");
    assert_eq!((invalid["well_formed"].clone(), invalid["valid"].clone()), (json!(true), json!(false)), "{invalid}");
    assert!(codes(&invalid).contains(&"vc.element-not-declared".to_owned()), "{invalid}");
    let problem = invalid["problems"].as_array().and_then(|p| p.iter().find(|p| p["code"] == "vc.element-not-declared")).expect("a problem");
    assert_eq!(
        (problem["kind"].clone(), problem["line"].clone(), problem["column"].clone(), problem["file"].clone()),
        (json!("validity"), json!(3), json!(4), Value::Null)
    );
}

#[test]
fn a_document_that_is_not_well_formed_is_not_validated() {
    let mut a = Automation::new();
    let result = call(&mut a, json!({ "text": "<!DOCTYPE r [<!ELEMENT r EMPTY>]><r><x></r>" })).expect("runs");
    assert_eq!(
        (result["well_formed"].clone(), result["valid"].clone(), result["validated"].clone()),
        (json!(false), json!(false), json!(false)),
        "{result}"
    );
    assert!(result["problems"].as_array().into_iter().flatten().all(|p| p["kind"] == "well-formedness"), "{result}");
}

#[test]
fn a_document_with_no_doctype_has_nothing_to_be_valid_against() {
    let mut a = Automation::new();
    let result = call(&mut a, json!({ "text": "<r/>" })).expect("runs");
    assert_eq!((result["well_formed"].clone(), result["valid"].clone()), (json!(true), json!(false)), "{result}");
    assert_eq!(codes(&result), ["vc.no-dtd"]);
}

#[test]
fn the_external_subset_is_a_file_next_to_the_document_and_problems_in_it_name_the_file() {
    let dir = tempfile::tempdir().expect("dir");
    write(dir.path(), "sub/r.dtd", "<!ELEMENT r (a)>\n<!ELEMENT a EMPTY>\n<!ATTLIST a x ID #FIXED 'v'>\n");
    let doc = write(dir.path(), "sub/doc.xml", "<!DOCTYPE r SYSTEM \"r.dtd\">\n<r><a/></r>");
    let mut a = Automation::new();
    let result = call(&mut a, json!({ "path": doc })).expect("runs");
    // The declaration in the DTD breaks a constraint, and the problem is in the DTD's file at its line.
    let problem = result["problems"].as_array().and_then(|p| p.iter().find(|p| p["code"] == "vc.id-attribute-default")).expect("a problem");
    assert!(problem["file"].as_str().is_some_and(|f| f.ends_with("sub/r.dtd") || f.ends_with("sub\\r.dtd")), "{result}");
    assert_eq!(problem["line"], json!(3), "{result}");
    // The document itself is fine against the elements the DTD declares.
    write(dir.path(), "sub/good.dtd", "<!ELEMENT r (a)><!ELEMENT a EMPTY>");
    let doc2 = write(dir.path(), "sub/doc2.xml", "<!DOCTYPE r SYSTEM \"good.dtd\"><r><a/></r>");
    let ok = call(&mut a, json!({ "path": doc2 })).expect("runs");
    assert_eq!(ok["valid"], json!(true), "{ok}");
    let bad = write(dir.path(), "sub/doc3.xml", "<!DOCTYPE r SYSTEM \"good.dtd\"><r><a/><a/></r>");
    assert_eq!(call(&mut a, json!({ "path": bad })).expect("runs")["valid"], json!(false));
}

#[test]
fn an_open_document_is_checked_from_where_it_is_saved() {
    let dir = tempfile::tempdir().expect("dir");
    write(dir.path(), "r.dtd", "<!ELEMENT r EMPTY>");
    let doc = write(dir.path(), "doc.xml", "<!DOCTYPE r SYSTEM \"r.dtd\"><r>text</r>");
    let mut a = Automation::new();
    let opened = a.call("doc_open", &json!({ "path": doc })).expect("opens");
    let Some(Content::Json(opened)) = opened.into_iter().next() else { panic!("json") };
    let result = call(&mut a, json!({ "id": opened["id"] })).expect("runs");
    assert_eq!(codes(&result), ["vc.empty-element-has-content"], "{result}");
}

#[test]
fn nothing_outside_the_root_is_read_and_nothing_is_downloaded() {
    let outside = tempfile::tempdir().expect("outside");
    let root = tempfile::tempdir().expect("root");
    write(outside.path(), "r.dtd", "<!ELEMENT r EMPTY>");
    let outside_dtd = outside.path().join("r.dtd");
    // The DTD is outside the root: not read, so the document cannot be called invalid for what is in it.
    let doc = write(root.path(), "doc.xml", &format!("<!DOCTYPE r SYSTEM \"{}\"><r>text</r>", outside_dtd.display()));
    let mut confined = Automation::with_root(root.path()).expect("a root");
    let result = call(&mut confined, json!({ "path": doc })).expect("runs");
    assert_eq!((result["valid"].clone(), result["errors"].clone()), (json!(true), json!(0)), "{result}");
    assert!(codes(&result).contains(&"dtd.external-not-read".to_owned()), "{result}");
    // Without the root it is read, which shows the test is about the root.
    let mut free = Automation::new();
    let result = call(&mut free, json!({ "path": doc })).expect("runs");
    assert_eq!(codes(&result), ["vc.empty-element-has-content"], "{result}");
    // A URL is never fetched.
    let url = write(root.path(), "url.xml", "<!DOCTYPE r SYSTEM \"http://example.invalid/r.dtd\"><r>text</r>");
    let result = call(&mut confined, json!({ "path": url })).expect("runs");
    assert!(codes(&result).contains(&"dtd.external-not-read".to_owned()) && result["valid"] == json!(true), "{result}");
    // Not even when a file by that name is there (on a system that allows a colon in a name).
    #[cfg(unix)]
    {
        write(root.path(), "http:/example.invalid/r.dtd", "<!ELEMENT r EMPTY>");
        let result = call(&mut confined, json!({ "path": url })).expect("runs");
        assert!(codes(&result).contains(&"dtd.external-not-read".to_owned()) && result["valid"] == json!(true), "{result}");
    }
    // Text has no folder: a relative identifier is not found.
    let result = call(&mut free, json!({ "text": "<!DOCTYPE r SYSTEM \"r.dtd\"><r/>" })).expect("runs");
    assert!(codes(&result).contains(&"dtd.external-not-read".to_owned()), "{result}");
}

#[test]
fn the_arguments_are_checked() {
    let mut a = Automation::new();
    assert!(call(&mut a, json!({})).is_err());
    assert!(call(&mut a, json!({ "text": "<r/>", "path": "x" })).is_err());
    assert!(call(&mut a, json!({ "path": "/definitely/not/there.xml" })).is_err());
}

#[test]
fn an_external_text_that_is_too_big_or_not_text_is_not_read() {
    let dir = tempfile::tempdir().expect("dir");
    let mut a = Automation::new();
    // Not valid in its encoding.
    std::fs::write(dir.path().join("bad.dtd"), b"<!ELEMENT r EMPTY> \xFF\xFE\xFD").expect("writes");
    let doc = write(dir.path(), "bad.xml", "<!DOCTYPE r SYSTEM \"bad.dtd\"><r>text</r>");
    let result = call(&mut a, json!({ "path": doc })).expect("runs");
    assert!(codes(&result).contains(&"dtd.external-not-read".to_owned()) && result["valid"] == json!(true), "{result}");
    // Over the limit: a sparse file, so nothing is written.
    #[cfg(unix)]
    {
        let big = std::fs::File::create(dir.path().join("big.dtd")).expect("creates");
        big.set_len(20 * 1024 * 1024).expect("sets the length");
        let doc = write(dir.path(), "big.xml", "<!DOCTYPE r SYSTEM \"big.dtd\"><r>text</r>");
        let result = call(&mut a, json!({ "path": doc })).expect("runs");
        let note = result["problems"].as_array().and_then(|p| p.iter().find(|p| p["code"] == "dtd.external-not-read")).expect("a note");
        assert!(note["message"].as_str().is_some_and(|m| m.contains("over")), "{result}");
    }
}

const XS: &str = "http://www.w3.org/2001/XMLSchema";

fn schema_of(body: &str) -> String {
    format!("<xs:schema xmlns:xs=\"{XS}\">{body}</xs:schema>")
}

const ORDER: &str =
    "<xs:element name=\"order\"><xs:complexType><xs:sequence><xs:element name=\"id\" type=\"xs:int\"/></xs:sequence></xs:complexType></xs:element>";

#[test]
fn a_document_is_validated_against_a_schema_that_is_given() {
    let dir = tempfile::tempdir().expect("dir");
    write(dir.path(), "s/order.xsd", &schema_of(ORDER));
    let ok = write(dir.path(), "s/ok.xml", "<order><id>7</id></order>");
    let bad = write(dir.path(), "s/bad.xml", "<order>\n  <id>seven</id>\n</order>");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let result = call(&mut a, json!({ "path": ok, "schema": "order.xsd" })).expect("runs");
    assert_eq!((result["kind"].clone(), result["valid"].clone(), result["errors"].clone()), (json!("xsd"), json!(true), json!(0)), "{result}");
    assert_eq!(result["schemas"].as_array().map(Vec::len), Some(1), "{result}");
    let result = call(&mut a, json!({ "path": bad, "schema": "order.xsd" })).expect("runs");
    assert_eq!((result["valid"].clone(), result["errors"].clone()), (json!(false), json!(1)), "{result}");
    let problem = &result["problems"][0];
    assert_eq!(
        (problem["code"].clone(), problem["kind"].clone(), problem["line"].clone()),
        (json!("cvc.invalid-value"), json!("validity"), json!(2)),
        "{result}"
    );
    assert!(problem["message"].as_str().is_some_and(|m| m.contains("cvc-datatype-valid")), "{result}");
    assert_eq!((problem["column"].clone(), problem["end_column"].clone()), (json!(4), json!(6)), "the name of the element: {result}");
}

#[test]
fn a_document_that_names_its_schema_is_validated_against_it() {
    let dir = tempfile::tempdir().expect("dir");
    write(dir.path(), "order.xsd", &schema_of(ORDER));
    let doc = write(
        dir.path(),
        "doc.xml",
        "<order xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"order.xsd\"><x/></order>",
    );
    let mut a = Automation::with_root(dir.path()).expect("root");
    let result = call(&mut a, json!({ "path": doc })).expect("runs");
    assert_eq!((result["kind"].clone(), result["valid"].clone()), (json!("xsd"), json!(false)), "{result}");
    assert!(codes(&result).contains(&"cvc.child-not-allowed".to_owned()), "{result}");
}

#[test]
fn the_problems_of_a_schema_name_its_file_and_line() {
    let dir = tempfile::tempdir().expect("dir");
    write(dir.path(), "inc.xsd", &schema_of("\n<xs:simpleType name=\"s\"><xs:restriction base=\"nothing\"/></xs:simpleType>"));
    write(dir.path(), "main.xsd", &schema_of(&format!("<xs:include schemaLocation=\"inc.xsd\"/>{ORDER}")));
    let mut a = Automation::with_root(dir.path()).expect("root");
    let result =
        call(&mut a, json!({ "text": "<order><id>1</id></order>", "schema": dir.path().join("main.xsd").to_str().expect("utf-8") })).expect("runs");
    assert_eq!((result["valid"].clone(), result["schema_errors"].clone()), (json!(true), json!(1)), "{result}");
    let problem = result["problems"].as_array().and_then(|p| p.iter().find(|p| p["kind"] == "schema")).expect("a schema problem");
    assert_eq!(problem["code"], "xsd.unresolved-reference");
    assert!(problem["file"].as_str().is_some_and(|f| f.ends_with("inc.xsd")), "{problem}");
    assert_eq!(problem["line"], 2);
    assert_eq!(result["schemas"].as_array().map(Vec::len), Some(2), "{result}");
}

#[test]
fn a_schema_outside_the_root_is_not_read() {
    let outside = tempfile::tempdir().expect("dir");
    let inside = tempfile::tempdir().expect("dir");
    let secret = write(outside.path(), "secret.xsd", &schema_of(ORDER));
    let mut a = Automation::with_root(inside.path()).expect("root");
    let result = call(&mut a, json!({ "text": "<order><id>1</id></order>", "schema": secret })).expect("runs");
    assert_eq!((result["validated"].clone(), result["valid"].clone()), (json!(false), json!(false)), "{result}");
    assert!(result["note"].as_str().is_some_and(|n| n.contains("could not be loaded")), "{result}");
    // Nor through an include.
    write(inside.path(), "main.xsd", &schema_of(&format!("<xs:include schemaLocation=\"{}\"/>", secret.replace('\\', "/"))));
    let result = call(&mut a, json!({ "text": "<order><id>1</id></order>", "schema": "main.xsd" })).expect("runs");
    assert_eq!(result["schema_errors"], 1, "{result}");
    assert!(codes(&result).contains(&"xsd.document-not-loaded".to_owned()), "{result}");
}

#[test]
fn without_a_schema_or_a_doctype_it_is_still_the_dtd_that_has_nothing() {
    let mut a = Automation::new();
    let result = call(&mut a, json!({ "text": "<r/>" })).expect("runs");
    assert_eq!(codes(&result), ["vc.no-dtd"]);
    // A DOCTYPE wins over the hints of the root, unless a schema is asked for.
    let text =
        "<!DOCTYPE r [<!ELEMENT r EMPTY>]><r xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"x.xsd\"/>";
    let result = call(&mut a, json!({ "text": text })).expect("runs");
    assert_eq!(result.get("kind"), None, "{result}");
}

#[test]
fn a_schema_that_is_open_is_used_as_it_is_not_as_it_was_saved() {
    let dir = tempfile::tempdir().expect("dir");
    let saved = write(dir.path(), "order.xsd", &schema_of(ORDER));
    let mut a = Automation::with_root(dir.path()).expect("root");
    let text = "<order><id>seven</id></order>";
    let result = call(&mut a, json!({ "text": text, "schema": saved })).expect("runs");
    assert_eq!(result["valid"], json!(false), "the saved schema wants an integer: {result}");
    let opened = a.call("doc_open", &json!({ "path": saved })).expect("opens");
    let Some(Content::Json(opened)) = opened.into_iter().next() else { panic!("json") };
    a.call("doc_set_text", &json!({ "id": opened["id"], "text": schema_of(&ORDER.replace("xs:int", "xs:string")) })).expect("edits");
    let result = call(&mut a, json!({ "text": text, "schema": saved })).expect("runs");
    assert_eq!(result["valid"], json!(true), "the unsaved text of the schema is what counts: {result}");
}

#[test]
fn a_document_that_is_not_well_formed_is_not_checked_against_the_schema() {
    let dir = tempfile::tempdir().expect("dir");
    let schema = write(dir.path(), "order.xsd", &schema_of(ORDER));
    let mut a = Automation::new();
    let result = call(&mut a, json!({ "text": "<order><id>1</order>", "schema": schema })).expect("runs");
    assert_eq!(
        (result["well_formed"].clone(), result["valid"].clone(), result["validated"].clone(), result["kind"].clone()),
        (json!(false), json!(false), json!(false), json!("xsd")),
        "{result}"
    );
    assert!(result["errors"].as_u64().is_some_and(|n| n > 0), "{result}");
    assert!(result["problems"].as_array().is_some_and(|p| !p.is_empty() && p.iter().all(|p| p["kind"] == "well-formedness")), "{result}");
}

#[test]
fn a_warning_about_the_document_is_counted_and_listed() {
    let dir = tempfile::tempdir().expect("dir");
    let schema = write(dir.path(), "order.xsd", &schema_of(ORDER));
    let mut a = Automation::new();
    let result = call(&mut a, json!({ "text": "<?xml version=\"1.1\"?><order><id>1</id></order>", "schema": schema })).expect("runs");
    assert_eq!((result["valid"].clone(), result["errors"].clone(), result["warnings"].clone()), (json!(true), json!(0), json!(1)), "{result}");
    let problem = &result["problems"][0];
    assert_eq!((problem["kind"].clone(), problem["severity"].clone()), (json!("well-formedness"), json!("warning")), "{result}");
}

#[test]
fn only_so_many_problems_are_listed_and_it_says_so() {
    let dir = tempfile::tempdir().expect("dir");
    let schema = write(
        dir.path(),
        "list.xsd",
        &schema_of(
            "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"n\" type=\"xs:int\" maxOccurs=\"unbounded\"/></xs:sequence></xs:complexType></xs:element>",
        ),
    );
    let mut a = Automation::new();
    let many = format!("<r>{}</r>", "<n>x</n>".repeat(600));
    let result = call(&mut a, json!({ "text": many, "schema": schema })).expect("runs");
    assert_eq!(result["errors"], json!(600), "{}", result["errors"]);
    assert_eq!((result["problems"].as_array().map(Vec::len), result["truncated"].clone()), (Some(500), json!(true)));
    let few = format!("<r>{}</r>", "<n>x</n>".repeat(3));
    let result = call(&mut a, json!({ "text": few, "schema": schema })).expect("runs");
    assert_eq!((result["problems"].as_array().map(Vec::len), result["truncated"].clone()), (Some(3), json!(false)));
}
