//! `schematron_validate`, through the tool table and the root confinement.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    a.call("schematron_validate", &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

const RULES: &str = r#"<schema xmlns="http://purl.oclc.org/dsdl/schematron"><title>Orders</title><pattern><rule context="item"><assert test="@qty" id="Q-1" flag="warning">Item <name/> needs a quantity.</assert><report test="@qty = 0">zero</report></rule></pattern></schema>"#;
const ORDER: &str = "<order>\n  <item qty=\"1\"/>\n  <item/>\n  <item qty=\"0\"/>\n</order>";

#[test]
fn rules_given_as_text_fire_with_lines_and_counts() {
    let mut a = Automation::new();
    let r = call(&mut a, json!({ "text": ORDER, "schema": { "text": RULES } })).expect("runs");
    assert_eq!(
        (r["count"].as_u64(), r["errors"].as_u64(), r["fired_rules"].as_u64(), r["checks"].as_u64()),
        (Some(2), Some(0), Some(1), Some(2)),
        "{r}"
    );
    assert_eq!(r["valid"], json!(true), "warnings and information do not make it invalid: {r}");
    let f = &r["findings"][0];
    assert_eq!(
        (f["severity"].as_str(), f["id"].as_str(), f["message"].as_str(), f["line"].as_u64(), f["column"].as_u64()),
        (Some("warning"), Some("Q-1"), Some("Item item needs a quantity."), Some(3), Some(3)),
        "{f}"
    );
    assert_eq!(f["path"].as_str(), Some("/order[1]/item[2]"));
    assert_eq!((r["findings"][1]["kind"].as_str(), r["findings"][1]["severity"].as_str()), (Some("report"), Some("info")));
    assert_eq!(r["title"].as_str(), Some("Orders"));
}

#[test]
fn rules_named_by_the_document_are_found_next_to_it_and_includes_stay_inside_the_root() {
    let dir = tempfile::tempdir().expect("dir");
    let main = r#"<schema xmlns="http://purl.oclc.org/dsdl/schematron"><include href="more.sch"/></schema>"#;
    let more = r#"<pattern xmlns="http://purl.oclc.org/dsdl/schematron"><rule context="order"><assert test="false()" flag="fatal">no</assert></rule></pattern>"#;
    std::fs::write(dir.path().join("rules.sch"), main).expect("writes");
    std::fs::write(dir.path().join("more.sch"), more).expect("writes");
    let doc = "<?xml-model href=\"rules.sch\" schematypens=\"http://purl.oclc.org/dsdl/schematron\"?>\n<order/>";
    std::fs::write(dir.path().join("o.xml"), doc).expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let r = call(&mut a, json!({ "path": "o.xml" })).expect("runs");
    assert_eq!((r["valid"].as_bool(), r["errors"].as_u64()), (Some(false), Some(1)), "{r}");
    assert_eq!(r["findings"][0]["message"].as_str(), Some("no"));
    // An include that points outside the root is not read.
    let outside = tempfile::tempdir().expect("dir");
    std::fs::write(outside.path().join("x.sch"), more).expect("writes");
    let escaping = format!(
        "<schema xmlns=\"http://purl.oclc.org/dsdl/schematron\"><include href=\"{}\"/></schema>",
        outside.path().join("x.sch").to_str().expect("utf-8")
    );
    let e = call(&mut a, json!({ "text": "<order/>", "schema": { "text": escaping } })).expect_err("refused");
    assert!(e.to_string().contains("cannot be read"), "{e}");
}

#[test]
fn what_is_missing_or_wrong_is_said() {
    let mut a = Automation::new();
    let e = call(&mut a, json!({ "text": "<order/>" })).expect_err("no rules");
    assert!(e.to_string().contains("xml-model"), "{e}");
    let e = call(&mut a, json!({ "text": "<order/>", "schema": { "text": "<a/>" } })).expect_err("not Schematron");
    assert!(e.to_string().contains("not a Schematron"), "{e}");
    let e = call(&mut a, json!({ "text": "<order>", "schema": { "text": RULES } })).expect_err("not well-formed");
    assert!(e.to_string().contains("cannot be read"), "{e}");
    assert!(matches!(call(&mut a, json!({ "text": "<order/>", "schema": "x" })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(call(&mut a, json!({ "text": "<order/>", "schema": { "text": RULES }, "phase": 1 })), Err(ToolError::InvalidArgs(_))));
    let e = call(&mut a, json!({ "text": "{}", "language": "json", "schema": { "text": RULES } })).expect_err("not XML");
    assert!(e.to_string().contains("XML"), "{e}");
}
