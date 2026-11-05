//! `xbrl_facts`, through the tool table.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    a.call("xbrl_facts", &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

const DOC: &str = "<xbrli:xbrl xmlns:xbrli=\"http://www.xbrl.org/2003/instance\" xmlns:link=\"http://www.xbrl.org/2003/linkbase\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" xmlns:iso4217=\"http://www.xbrl.org/2003/iso4217\" xmlns:g=\"urn:g\">\n<link:schemaRef xlink:type=\"simple\" xlink:href=\"g.xsd\"/>\n<xbrli:context id=\"c\"><xbrli:entity><xbrli:identifier scheme=\"s\">A</xbrli:identifier></xbrli:entity><xbrli:period><xbrli:instant>2025-12-31</xbrli:instant></xbrli:period></xbrli:context>\n<xbrli:unit id=\"u\"><xbrli:measure>iso4217:EUR</xbrli:measure></xbrli:unit>\n<g:Assets contextRef=\"c\" unitRef=\"u\" decimals=\"0\">10</g:Assets>\n<g:Cash contextRef=\"nope\" unitRef=\"u\" decimals=\"0\">1</g:Cash>\n</xbrli:xbrl>";

#[test]
fn facts_contexts_units_and_findings_are_listed_with_places() {
    let mut a = Automation::new();
    let r = call(&mut a, json!({ "text": DOC })).expect("runs");
    assert_eq!((r["kind"].as_str(), r["fact_count"].as_u64(), r["schema_refs"][0].as_str()), (Some("instance"), Some(2), Some("g.xsd")), "{r}");
    assert_eq!((r["contexts"][0]["period"].as_str(), r["units"][0]["measure"].as_str()), (Some("2025-12-31"), Some("EUR")));
    let assets = &r["facts"][0];
    assert_eq!(
        (assets["name"].as_str(), assets["value"].as_str(), assets["context"].as_str(), assets["line"].as_u64()),
        (Some("g:Assets"), Some("10"), Some("c"), Some(5))
    );
    let finding = r["findings"].as_array().and_then(|f| f.iter().find(|f| f["code"] == "xbrl.unknown-context")).expect("a finding");
    assert_eq!((finding["level"].as_str(), finding["line"].as_u64()), (Some("error"), Some(6)), "{r}");
    let only = call(&mut a, json!({ "text": DOC, "concept": "Cash", "max_facts": 1 })).expect("runs");
    assert_eq!((only["fact_count"].as_u64(), only["facts"][0]["name"].as_str()), (Some(1), Some("g:Cash")));
}

#[test]
fn other_documents_and_bad_arguments_are_refused() {
    let mut a = Automation::new();
    assert!(call(&mut a, json!({ "text": "<a/>" })).expect_err("not xbrl").to_string().contains("not an XBRL document"));
    assert!(call(&mut a, json!({ "text": "{}", "language": "json" })).is_err());
    assert!(matches!(call(&mut a, json!({ "text": DOC, "concept": 1 })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(call(&mut a, json!({ "text": DOC, "max_facts": 0 })), Err(ToolError::InvalidArgs(_))));
}
