//! `schema_check`: an XML Schema checked as a schema itself.

use std::path::Path;

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    a.call("schema_check", &args).map(|content| match content.into_iter().next() {
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

#[test]
fn a_valid_schema_is_valid_and_one_with_a_type_that_does_not_exist_is_not() {
    let mut a = Automation::new();
    let valid = call(
        &mut a,
        json!({ "text": r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="a" type="xs:string"/></xs:schema>"# }),
    )
    .expect("runs");
    assert_eq!((valid["valid"].clone(), valid["errors"].clone()), (json!(true), json!(0)), "{valid}");
    let invalid = call(
        &mut a,
        json!({ "text": r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="a" type="xs:notAType"/></xs:schema>"# }),
    )
    .expect("runs");
    assert_eq!((invalid["well_formed"].clone(), invalid["valid"].clone()), (json!(true), json!(false)), "{invalid}");
    assert!(invalid["errors"].as_u64().is_some_and(|n| n > 0), "{invalid}");
    let problem = invalid["problems"].as_array().and_then(|p| p.first()).expect("a problem");
    assert_eq!((problem["kind"].clone(), problem["file"].clone()), (json!("schema"), Value::Null), "{invalid}");
    assert!(problem["line"].as_u64().is_some(), "{invalid}");
}

#[test]
fn a_problem_in_an_included_document_names_that_document() {
    let dir = tempfile::tempdir().expect("dir");
    write(dir.path(), "inc.xsd", r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="b" type="xs:missing"/></xs:schema>"#);
    let root =
        write(dir.path(), "root.xsd", r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:include schemaLocation="inc.xsd"/></xs:schema>"#);
    let mut a = Automation::new();
    let result = call(&mut a, json!({ "path": root })).expect("runs");
    assert_eq!(result["valid"], json!(false), "{result}");
    let problem = result["problems"].as_array().and_then(|p| p.iter().find(|p| p["file"].is_string())).expect("a problem in a file");
    assert!(problem["file"].as_str().is_some_and(|f| f.ends_with("inc.xsd")), "{result}");
    assert!(problem["line"].as_u64().is_some(), "{result}");
    assert!(result["documents"].as_array().is_some_and(|d| d.iter().any(|f| f.as_str().is_some_and(|f| f.ends_with("inc.xsd")))), "{result}");
}

#[test]
fn a_document_that_is_not_a_schema_is_not_valid() {
    let mut a = Automation::new();
    let result = call(&mut a, json!({ "text": "<r/>" })).expect("runs");
    assert_eq!((result["well_formed"].clone(), result["valid"].clone()), (json!(true), json!(false)), "{result}");
    assert!(result["errors"].as_u64().is_some_and(|n| n > 0), "{result}");
}

#[test]
fn a_language_that_is_not_xml_is_refused() {
    let dir = tempfile::tempdir().expect("dir");
    let path = write(dir.path(), "not-a-schema.json", "{}");
    let mut a = Automation::new();
    let error = call(&mut a, json!({ "path": path })).expect_err("refused");
    assert!(error.to_string().contains("XML"), "{error}");
}

#[test]
fn a_schema_that_is_not_well_formed_says_what_is_wrong_with_its_syntax() {
    let mut a = Automation::new();
    let result = call(&mut a, json!({ "text": "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"><xs:element></xs:schema>" })).expect("runs");
    assert_eq!((result["well_formed"].clone(), result["valid"].clone()), (json!(false), json!(false)), "{result}");
    let problem = result["problems"].as_array().and_then(|p| p.first()).expect("the syntax problem is listed");
    assert_eq!(problem["kind"], json!("well-formedness"), "{result}");
    assert!(problem["line"].as_u64().is_some(), "{result}");
    assert!(result["errors"].as_u64().is_some_and(|n| n > 0), "{result}");
}
