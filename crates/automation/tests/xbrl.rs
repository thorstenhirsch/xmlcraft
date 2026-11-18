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

#[test]
fn the_taxonomy_a_schema_ref_names_is_read_from_beside_the_instance_and_its_findings_are_listed() {
    let dir = tempfile::tempdir().expect("dir");
    let schema = "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\" xmlns:xbrli=\"http://www.xbrl.org/2003/instance\" targetNamespace=\"urn:g\"><xs:element name=\"Assets\" type=\"xbrli:monetaryItemType\" substitutionGroup=\"xbrli:item\" xbrli:periodType=\"instant\"/></xs:schema>";
    std::fs::write(dir.path().join("g.xsd"), schema).expect("writes");
    let path = dir.path().join("i.xml");
    std::fs::write(&path, DOC).expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let r = call(&mut a, json!({ "path": path.to_str().expect("utf-8") })).expect("runs");
    let codes: Vec<&str> = r["findings"].as_array().expect("findings").iter().filter_map(|f| f["code"].as_str()).collect();
    // g:Assets is declared and fine; g:Cash is not declared in the taxonomy (and its context does not exist).
    assert!(codes.contains(&"xbrl.unknown-concept") && codes.contains(&"xbrl.unknown-context"), "{codes:?}");
    assert!(!codes.contains(&"xbrl.taxonomy-not-loaded"), "{codes:?}");
    // Without the taxonomy beside it, the instance still has its own findings and a warning that the taxonomy is not there.
    std::fs::remove_file(dir.path().join("g.xsd")).expect("removes");
    let r = call(&mut a, json!({ "path": path.to_str().expect("utf-8") })).expect("runs");
    let codes: Vec<&str> = r["findings"].as_array().expect("findings").iter().filter_map(|f| f["code"].as_str()).collect();
    assert!(codes.contains(&"xbrl.taxonomy-not-loaded") && !codes.contains(&"xbrl.unknown-concept"), "{codes:?}");
}

#[test]
fn a_calculation_linkbase_beside_the_schema_is_used_to_check_the_sums() {
    let dir = tempfile::tempdir().expect("dir");
    let schema = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xbrli="http://www.xbrl.org/2003/instance" xmlns:link="http://www.xbrl.org/2003/linkbase" xmlns:xlink="http://www.w3.org/1999/xlink" targetNamespace="urn:g">
<xs:annotation><xs:appinfo><link:linkbaseRef xlink:type="simple" xlink:href="g-cal.xml" xlink:role="http://www.xbrl.org/2003/role/calculationLinkbaseRef" xlink:arcrole="http://www.w3.org/1999/xlink/properties/linkbase"/></xs:appinfo></xs:annotation>
<xs:element id="g_Assets" name="Assets" type="xbrli:monetaryItemType" substitutionGroup="xbrli:item" xbrli:periodType="instant"/>
<xs:element id="g_Cash" name="Cash" type="xbrli:monetaryItemType" substitutionGroup="xbrli:item" xbrli:periodType="instant"/>
<xs:element id="g_Stock" name="Stock" type="xbrli:monetaryItemType" substitutionGroup="xbrli:item" xbrli:periodType="instant"/></xs:schema>"#;
    let calc = r#"<link:linkbase xmlns:link="http://www.xbrl.org/2003/linkbase" xmlns:xlink="http://www.w3.org/1999/xlink"><link:calculationLink xlink:type="extended" xlink:role="http://www.xbrl.org/2003/role/link">
<link:loc xlink:type="locator" xlink:href="g.xsd#g_Assets" xlink:label="a"/><link:loc xlink:type="locator" xlink:href="g.xsd#g_Cash" xlink:label="c"/><link:loc xlink:type="locator" xlink:href="g.xsd#g_Stock" xlink:label="s"/>
<link:calculationArc xlink:type="arc" xlink:arcrole="http://www.xbrl.org/2003/arcrole/summation-item" xlink:from="a" xlink:to="c" weight="1" order="1"/>
<link:calculationArc xlink:type="arc" xlink:arcrole="http://www.xbrl.org/2003/arcrole/summation-item" xlink:from="a" xlink:to="s" weight="1" order="2"/></link:calculationLink></link:linkbase>"#;
    let instance = |assets: &str| {
        format!(
            r#"<xbrli:xbrl xmlns:xbrli="http://www.xbrl.org/2003/instance" xmlns:link="http://www.xbrl.org/2003/linkbase" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:iso4217="http://www.xbrl.org/2003/iso4217" xmlns:g="urn:g"><link:schemaRef xlink:type="simple" xlink:href="g.xsd"/>
<xbrli:context id="c"><xbrli:entity><xbrli:identifier scheme="s">x</xbrli:identifier></xbrli:entity><xbrli:period><xbrli:instant>2025-12-31</xbrli:instant></xbrli:period></xbrli:context>
<xbrli:unit id="u"><xbrli:measure>iso4217:EUR</xbrli:measure></xbrli:unit>
<g:Cash contextRef="c" unitRef="u" decimals="0">70</g:Cash><g:Stock contextRef="c" unitRef="u" decimals="0">30</g:Stock><g:Assets contextRef="c" unitRef="u" decimals="0">{assets}</g:Assets></xbrli:xbrl>"#
        )
    };
    std::fs::write(dir.path().join("g.xsd"), schema).expect("writes");
    std::fs::write(dir.path().join("g-cal.xml"), calc).expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let codes = |a: &mut Automation, assets: &str| -> Vec<String> {
        let path = dir.path().join("i.xml");
        std::fs::write(&path, instance(assets)).expect("writes");
        let r = call(a, json!({ "path": path.to_str().expect("utf-8") })).expect("runs");
        r["findings"].as_array().expect("findings").iter().filter_map(|f| f["code"].as_str().map(str::to_owned)).collect()
    };
    assert!(!codes(&mut a, "100").contains(&"xbrl.calculation-inconsistent".to_owned()), "70 + 30 is 100");
    assert!(codes(&mut a, "101").contains(&"xbrl.calculation-inconsistent".to_owned()), "70 + 30 is not 101");
}

#[test]
fn a_definition_linkbase_beside_the_schema_judges_the_dimensions_and_the_taxonomy_is_summed_up() {
    let dir = tempfile::tempdir().expect("dir");
    let schema = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:xbrli="http://www.xbrl.org/2003/instance" xmlns:xbrldt="http://xbrl.org/2005/xbrldt" xmlns:link="http://www.xbrl.org/2003/linkbase" xmlns:xlink="http://www.w3.org/1999/xlink" targetNamespace="urn:g">
<xs:annotation><xs:appinfo><link:linkbaseRef xlink:type="simple" xlink:href="g-def.xml" xlink:role="http://www.xbrl.org/2003/role/definitionLinkbaseRef" xlink:arcrole="http://www.w3.org/1999/xlink/properties/linkbase"/></xs:appinfo></xs:annotation>
<xs:element id="g_Sales" name="Sales" type="xbrli:monetaryItemType" substitutionGroup="xbrli:item" xbrli:periodType="instant"/>
<xs:element id="g_Cube" name="Cube" type="xbrli:stringItemType" substitutionGroup="xbrldt:hypercubeItem" abstract="true" xbrli:periodType="instant"/>
<xs:element id="g_Region" name="Region" type="xbrli:stringItemType" substitutionGroup="xbrldt:dimensionItem" abstract="true" xbrli:periodType="instant"/>
<xs:element id="g_All" name="All" type="xbrli:stringItemType" substitutionGroup="xbrli:item" abstract="true" xbrli:periodType="instant"/>
<xs:element id="g_North" name="North" type="xbrli:stringItemType" substitutionGroup="xbrli:item" abstract="true" xbrli:periodType="instant"/></xs:schema>"#;
    let arc = |role: &str, from: &str, to: &str, extra: &str| {
        format!(
            r#"<link:definitionArc xlink:type="arc" xlink:arcrole="http://xbrl.org/int/dim/arcrole/{role}" xlink:from="{from}" xlink:to="{to}" {extra}/>"#
        )
    };
    let locs: String = ["Sales", "Cube", "Region", "All", "North"]
        .iter()
        .map(|c| format!(r#"<link:loc xlink:type="locator" xlink:href="g.xsd#g_{c}" xlink:label="{c}"/>"#))
        .collect();
    let def = format!(
        r#"<link:linkbase xmlns:link="http://www.xbrl.org/2003/linkbase" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:xbrldt="http://xbrl.org/2005/xbrldt"><link:definitionLink xlink:type="extended" xlink:role="http://www.xbrl.org/2003/role/link">{locs}{}{}{}{}{}</link:definitionLink></link:linkbase>"#,
        arc("all", "Sales", "Cube", r#"xbrldt:contextElement="segment" xbrldt:closed="true""#),
        arc("hypercube-dimension", "Cube", "Region", ""),
        arc("dimension-domain", "Region", "All", ""),
        arc("domain-member", "All", "North", ""),
        arc("dimension-default", "Region", "All", ""),
    );
    std::fs::write(dir.path().join("g.xsd"), schema).expect("writes");
    std::fs::write(dir.path().join("g-def.xml"), def).expect("writes");
    let instance = |segment: &str| {
        format!(
            r#"<xbrli:xbrl xmlns:xbrli="http://www.xbrl.org/2003/instance" xmlns:link="http://www.xbrl.org/2003/linkbase" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:xbrldi="http://xbrl.org/2006/xbrldi" xmlns:iso4217="http://www.xbrl.org/2003/iso4217" xmlns:g="urn:g"><link:schemaRef xlink:type="simple" xlink:href="g.xsd"/>
<xbrli:context id="c"><xbrli:entity><xbrli:identifier scheme="s">x</xbrli:identifier><xbrli:segment>{segment}</xbrli:segment></xbrli:entity><xbrli:period><xbrli:instant>2025-12-31</xbrli:instant></xbrli:period></xbrli:context>
<xbrli:unit id="u"><xbrli:measure>iso4217:EUR</xbrli:measure></xbrli:unit>
<g:Sales contextRef="c" unitRef="u" decimals="0">5</g:Sales></xbrli:xbrl>"#
        )
    };
    let mut a = Automation::with_root(dir.path()).expect("root");
    let mut run = |segment: &str| -> Value {
        let path = dir.path().join("i.xml");
        std::fs::write(&path, instance(segment)).expect("writes");
        call(&mut a, json!({ "path": path.to_str().expect("utf-8") })).expect("runs")
    };
    let member = |m: &str| format!(r#"<xbrldi:explicitMember dimension="g:Region">g:{m}</xbrldi:explicitMember>"#);
    let r = run(&member("North"));
    assert_eq!(r["findings"].as_array().map(Vec::len), Some(0), "{r}");
    let t = &r["taxonomy"];
    assert_eq!(
        (t["hypercubes"].as_u64(), t["dimensions"].as_u64(), t["members"].as_u64(), t["primary_items"].as_u64()),
        (Some(1), Some(1), Some(2), Some(1)),
        "{r}"
    );
    assert_eq!(r["contexts"][0]["dimensions"][0]["member"].as_str(), Some("{urn:g}North"));
    let r = run(&member("All"));
    assert_eq!(r["findings_by_code"], json!({ "xbrl.dimension-default-used": 1 }), "{r}");
    let r = run(&member("Sales"));
    assert_eq!(r["findings_by_code"], json!({ "xbrl.dimension-invalid": 1 }), "{r}");
    let finding = &r["findings"][0];
    assert_eq!((finding["level"].as_str(), finding["line"].as_u64()), (Some("error"), Some(4)), "at the fact: {r}");
    assert!(run_without_schema_ref()["taxonomy"].is_null());
}

fn run_without_schema_ref() -> Value {
    let mut a = Automation::new();
    let text = DOC.replace("<link:schemaRef xlink:type=\"simple\" xlink:href=\"g.xsd\"/>", "");
    call(&mut a, json!({ "text": text })).expect("runs")
}
