//! `xslt_transform`, through the tool table and the root confinement.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    a.call("xslt_transform", &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

const XSL: &str = r#"<xsl:stylesheet version="3.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:template match="/"><out><xsl:for-each select="//item"><i><xsl:value-of select="."/></i></xsl:for-each></out></xsl:template></xsl:stylesheet>"#;
const DOC: &str = "<doc><item>a</item><item>b</item></doc>";

#[test]
fn a_stylesheet_given_as_text_is_applied() {
    let mut a = Automation::new();
    let r = call(&mut a, json!({ "text": DOC, "stylesheet": { "text": XSL } })).expect("runs");
    assert_eq!(
        (r["text"].as_str(), r["items"].as_u64(), r["truncated"].as_bool()),
        (Some("<out><i>a</i><i>b</i></out>"), Some(1), Some(false)),
        "{r}"
    );
}

#[test]
fn the_stylesheet_a_document_names_is_read_from_beside_it_and_only_inside_the_root() {
    let outer = tempfile::tempdir().expect("dir");
    let root = outer.path().join("root");
    std::fs::create_dir(&root).expect("creates");
    std::fs::write(root.join("style.xsl"), XSL).expect("writes");
    std::fs::write(root.join("d.xml"), format!("<?xml-stylesheet type=\"text/xsl\" href=\"style.xsl\"?>\n{DOC}")).expect("writes");
    std::fs::write(root.join("e.xml"), format!("<?xml-stylesheet type=\"text/xsl\" href=\"../outside.xsl\"?>\n{DOC}")).expect("writes");
    std::fs::write(outer.path().join("outside.xsl"), XSL).expect("writes");
    let mut a = Automation::with_root(&root).expect("root");
    let r = call(&mut a, json!({ "path": "d.xml" })).expect("runs");
    assert_eq!(r["text"].as_str(), Some("<out><i>a</i><i>b</i></out>"), "{r}");
    let e = call(&mut a, json!({ "path": "e.xml" })).expect_err("outside the root");
    assert!(e.to_string().contains("outside the root"), "{e}");
}

#[test]
fn what_is_missing_wrong_or_not_implemented_is_an_error_that_says_so() {
    let mut a = Automation::new();
    let e = call(&mut a, json!({ "text": DOC })).expect_err("no stylesheet");
    assert!(e.to_string().contains("xml-stylesheet"), "{e}");
    let e = call(&mut a, json!({ "text": DOC, "stylesheet": { "text": "<nope" } })).expect_err("bad stylesheet");
    assert!(e.to_string().contains("cannot be used"), "{e}");
    assert!(matches!(call(&mut a, json!({ "text": DOC, "stylesheet": "x" })), Err(ToolError::InvalidArgs(_))));
    let e = call(&mut a, json!({ "text": "{}", "language": "json", "stylesheet": { "text": XSL } }));
    assert!(e.is_err(), "JSON is not XML");
}
