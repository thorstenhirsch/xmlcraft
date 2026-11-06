//! `xslt_complete`, through the tool table.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    a.call("xslt_complete", &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

const HEAD: &str = "<xsl:stylesheet version=\"3.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\">\n";

fn labels(r: &Value) -> Vec<&str> {
    r["items"].as_array().expect("items").iter().filter_map(|i| i["label"].as_str()).collect()
}

#[test]
fn elements_attributes_values_and_functions_are_offered_where_they_fit() {
    let mut a = Automation::new();
    let text = format!("{HEAD}  <xsl:template match=\"/\">\n    <xsl:va");
    let r = call(&mut a, json!({ "text": text, "language": "xml", "line": 3, "column": 12 })).expect("runs");
    assert_eq!((r["kind"].as_str(), r["stylesheet"].as_bool()), (Some("element"), Some(true)), "{r}");
    assert_eq!(labels(&r), ["xsl:value-of", "xsl:variable"]);
    assert_eq!(r["items"][0]["insert"].as_str(), Some("<xsl:value-of select=\"\"/>"));
    // The same by byte offset, for an attribute value and a function.
    let text = format!("{HEAD}  <xsl:template match=\"/\"><xsl:sort order=\"a\"/>");
    let offset = text.find("order=\"a").expect("found") + 8;
    let r = call(&mut a, json!({ "text": text, "language": "xml", "offset": offset })).expect("runs");
    assert_eq!((r["kind"].as_str(), labels(&r)), (Some("value"), vec!["ascending"]));
    let text = format!("{HEAD}  <xsl:template match=\"/\"><xsl:value-of select=\"conc\"/>");
    let offset = text.find("conc").expect("found") + 4;
    let r = call(&mut a, json!({ "text": text, "language": "xml", "offset": offset })).expect("runs");
    assert_eq!((r["kind"].as_str(), labels(&r)), (Some("function"), vec!["concat"]));
    assert_eq!(r["items"][0]["detail"].as_str(), Some("concat($a, $b, …)"));
}

#[test]
fn a_document_that_is_no_stylesheet_has_nothing_and_a_bad_request_is_refused() {
    let mut a = Automation::new();
    let r = call(&mut a, json!({ "text": "<a><b", "language": "xml", "offset": 5 })).expect("runs");
    assert_eq!((r["stylesheet"].as_bool(), r["count"].as_u64()), (Some(false), Some(0)));
    assert!(call(&mut a, json!({ "text": "{}", "language": "json", "offset": 1 })).is_err(), "not XML");
    assert!(call(&mut a, json!({ "text": "<a/>", "language": "xml", "offset": 99 })).is_err(), "outside the text");
}

#[test]
fn variables_and_parameters_in_scope_are_offered_after_a_dollar() {
    let mut a = Automation::new();
    let text = format!("{HEAD}<xsl:variable name=\"g\"/><xsl:template match=\"/\"><xsl:param name=\"p\"/><xsl:value-of select=\"$\"/>");
    let offset = text.find("$\"").expect("found") + 1;
    let r = call(&mut a, json!({ "text": text, "language": "xml", "offset": offset })).expect("runs");
    assert_eq!((r["kind"].as_str(), labels(&r)), (Some("variable"), vec!["g", "p"]), "{r}");
    assert_eq!(r["items"][1]["detail"].as_str(), Some("parameter"));
}
