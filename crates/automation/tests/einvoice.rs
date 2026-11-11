//! `einvoice_detect`, through the tool table.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content};

fn detect(a: &mut Automation, args: Value) -> Value {
    let content = a.call("einvoice_detect", &args).expect("runs");
    match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    }
}

const XRECHNUNG: &str = r#"<Invoice xmlns="urn:oasis:names:specification:ubl:schema:xsd:Invoice-2" xmlns:cbc="urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2"><cbc:CustomizationID>urn:cen.eu:en16931:2017#compliant#urn:xeinkauf.de:kosit:xrechnung_3.0</cbc:CustomizationID><cbc:ProfileID>urn:fdc:peppol.eu:2017:poacc:billing:01:1.0</cbc:ProfileID></Invoice>"#;

#[test]
fn text_an_open_document_and_a_file_are_recognised_the_same_way() {
    let dir = tempfile::tempdir().expect("dir");
    std::fs::write(dir.path().join("i.xml"), XRECHNUNG).expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let by_text = detect(&mut a, json!({ "text": XRECHNUNG }));
    assert_eq!(by_text["recognised"], json!(true), "{by_text}");
    assert_eq!(
        (by_text["syntax"].as_str(), by_text["standard"].as_str(), by_text["version"].as_str()),
        (Some("ubl"), Some("XRechnung"), Some("3.0"))
    );
    assert_eq!(by_text["process"], json!("urn:fdc:peppol.eu:2017:poacc:billing:01:1.0"));
    let by_file = detect(&mut a, json!({ "path": "i.xml" }));
    assert_eq!(by_file, by_text);
    let opened = a.call("doc_open", &json!({ "path": "i.xml" })).expect("opens");
    let Some(Content::Json(opened)) = opened.into_iter().next() else { panic!("json") };
    let id = opened["id"].as_u64().expect("id");
    assert_eq!(detect(&mut a, json!({ "id": id })), by_text);
}

#[test]
fn other_documents_are_not_recognised_and_say_why() {
    let mut a = Automation::new();
    let r = detect(&mut a, json!({ "text": "<a/>" }));
    assert_eq!(r["recognised"], json!(false), "{r}");
    assert!(r["reason"].as_str().is_some_and(|s| s.contains("not a known")), "{r}");
    let r = detect(&mut a, json!({ "text": "{\"a\": 1}", "language": "json" }));
    assert_eq!((r["recognised"].clone(), r["reason"].as_str()), (json!(false), Some("the document is not XML")));
    assert!(a.call("einvoice_detect", &json!({})).is_err(), "one of id, path and text");
    assert!(a.call("einvoice_detect", &json!({ "text": "<a/>", "path": "x" })).is_err());
}

#[test]
fn factur_x_in_cii_has_its_profile() {
    let cii = r#"<rsm:CrossIndustryInvoice xmlns:rsm="urn:un:unece:uncefact:data:standard:CrossIndustryInvoice:100" xmlns:ram="urn:un:unece:uncefact:data:standard:ReusableAggregateBusinessInformationEntity:100"><rsm:ExchangedDocumentContext><ram:GuidelineSpecifiedDocumentContextParameter><ram:ID>urn:factur-x.eu:1p0:basicwl</ram:ID></ram:GuidelineSpecifiedDocumentContextParameter></rsm:ExchangedDocumentContext></rsm:CrossIndustryInvoice>"#;
    let mut a = Automation::new();
    let r = detect(&mut a, json!({ "text": cii }));
    assert_eq!(
        (r["syntax"].as_str(), r["standard"].as_str(), r["profile"].as_str()),
        (Some("cii"), Some("Factur-X / ZUGFeRD 2.1+"), Some("basic WL")),
        "{r}"
    );
}

const UBL_INVOICE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Invoice xmlns="urn:oasis:names:specification:ubl:schema:xsd:Invoice-2"
         xmlns:cac="urn:oasis:names:specification:ubl:schema:xsd:CommonAggregateComponents-2"
         xmlns:cbc="urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2">
  <cbc:CustomizationID>urn:cen.eu:en16931:2017#compliant#urn:xeinkauf.de:kosit:xrechnung_3.0</cbc:CustomizationID>
  <cbc:ID>RE-1</cbc:ID>
  <cbc:IssueDate>2026-11-05</cbc:IssueDate>
  <cbc:InvoiceTypeCode>380</cbc:InvoiceTypeCode>
  <cbc:DocumentCurrencyCode>EUR</cbc:DocumentCurrencyCode>
  <cac:AccountingSupplierParty><cac:Party><cac:PartyName><cbc:Name>Seller</cbc:Name></cac:PartyName></cac:Party></cac:AccountingSupplierParty>
  <cac:AccountingCustomerParty><cac:Party><cac:PartyName><cbc:Name>Buyer</cbc:Name></cac:PartyName></cac:Party></cac:AccountingCustomerParty>
  <cac:LegalMonetaryTotal><cbc:PayableAmount currencyID="EUR">10.00</cbc:PayableAmount></cac:LegalMonetaryTotal>
  <cac:InvoiceLine>
    <cbc:ID>1</cbc:ID>
    <cbc:InvoicedQuantity unitCode="C62">1</cbc:InvoicedQuantity>
    <cbc:LineExtensionAmount currencyID="EUR">10.00</cbc:LineExtensionAmount>
    <cac:Item><cbc:Name>Tea</cbc:Name></cac:Item>
  </cac:InvoiceLine>
</Invoice>
"#;

#[test]
fn xml_validate_checks_a_ubl_invoice_against_the_bundled_schema_without_being_told_which() {
    let mut a = Automation::new();
    let ok = {
        let content = a.call("xml_validate", &json!({ "text": UBL_INVOICE })).expect("runs");
        let Some(Content::Json(v)) = content.into_iter().next() else { panic!("json") };
        v
    };
    assert_eq!((ok["valid"].clone(), ok["validated"].clone()), (json!(true), json!(true)), "{ok}");
    let broken = UBL_INVOICE.replace("<cbc:ID>RE-1</cbc:ID>", "<cbc:ID>RE-1</cbc:ID><cbc:Bogus/>");
    let content = a.call("xml_validate", &json!({ "text": broken })).expect("runs");
    let Some(Content::Json(bad)) = content.into_iter().next() else { panic!("json") };
    assert_eq!(bad["valid"], json!(false), "{bad}");
    assert!(bad.to_string().contains("Bogus"), "{bad}");
}

/// A synthetic XRechnung invoice in CII (`crates/einvoice/tests/fixtures`).
const CII_INVOICE: &str = include_str!("../../einvoice/tests/fixtures/xrechnung-cii.xml");

fn validate(a: &mut Automation, text: &str) -> Value {
    let content = a.call("xml_validate", &json!({ "text": text })).expect("runs");
    let Some(Content::Json(v)) = content.into_iter().next() else { panic!("json") };
    v
}

#[test]
fn xml_validate_checks_a_cii_invoice_against_the_bundled_d16b_schema_offline() {
    let mut a = Automation::new();
    let ok = validate(&mut a, CII_INVOICE);
    assert_eq!((ok["valid"].clone(), ok["validated"].clone()), (json!(true), json!(true)), "{ok}");
    // An element the schema does not allow, at a place the problem points to.
    let broken = CII_INVOICE.replace("<ram:TypeCode>380</ram:TypeCode>", "<ram:TypeCode>380</ram:TypeCode><ram:Bogus/>");
    let bad = validate(&mut a, &broken);
    assert_eq!(bad["valid"], json!(false), "{bad}");
    let problems = bad["problems"].as_array().expect("problems");
    let at = broken.find("<ram:Bogus/>").expect("there");
    let line = broken[..at].matches('\n').count() + 1;
    assert!(problems.iter().any(|p| p.to_string().contains("Bogus") && p["line"].as_u64() == Some(line as u64)), "a problem on line {line}: {bad}");
    // A value of the wrong type (a date that is no date is text to the decoupled schema, but an amount is a decimal).
    let bad = validate(
        &mut a,
        &CII_INVOICE.replace("<ram:LineTotalAmount>100.00</ram:LineTotalAmount>", "<ram:LineTotalAmount>hundred</ram:LineTotalAmount>"),
    );
    assert_eq!(bad["valid"], json!(false), "{bad}");
}

const UBL_XRECHNUNG: &str = include_str!("../../einvoice/tests/fixtures/xrechnung-ubl.xml");

fn einvoice_validate(a: &mut Automation, args: Value) -> Value {
    let content = a.call("einvoice_validate", &args).expect("runs");
    let Some(Content::Json(v)) = content.into_iter().next() else { panic!("json") };
    v
}

#[test]
fn einvoice_validate_reports_each_stage_and_the_rules_a_broken_invoice_breaks() {
    let mut a = Automation::new();
    let ok = einvoice_validate(&mut a, json!({ "text": UBL_XRECHNUNG }));
    assert_eq!(ok["valid"], json!(true), "{ok}");
    let stages: Vec<(&str, &str)> =
        ok["stages"].as_array().expect("stages").iter().map(|s| (s["stage"].as_str().unwrap_or(""), s["status"].as_str().unwrap_or(""))).collect();
    assert_eq!(stages, [("well-formedness", "passed"), ("detection", "passed"), ("schema", "passed"), ("en16931", "passed"), ("cius", "passed")]);
    assert_eq!(ok["detection"]["standard"], json!("XRechnung"));
    assert_eq!(ok["totals"]["failed"], json!(0));
    assert_eq!(ok["not_evaluated_total"], json!(0));
    assert!(ok["totals"]["passed"].as_u64().is_some_and(|n| n > 800), "{}", ok["totals"]);
    // BR-CO-15, at the invoice element.
    let broken = UBL_XRECHNUNG
        .replace("<cbc:TaxInclusiveAmount currencyID=\"EUR\">119.00", "<cbc:TaxInclusiveAmount currencyID=\"EUR\">118.00")
        .replace("<cbc:PayableAmount currencyID=\"EUR\">119.00", "<cbc:PayableAmount currencyID=\"EUR\">118.00");
    let bad = einvoice_validate(&mut a, json!({ "text": broken, "stages": ["en16931"], "max_findings": 5 }));
    assert_eq!(bad["valid"], json!(false), "{bad}");
    let findings = bad["findings"].as_array().expect("findings");
    let rules: Vec<&str> = findings.iter().filter(|f| f["severity"] == json!("fatal")).filter_map(|f| f["rule"].as_str()).collect();
    assert_eq!(rules, ["BR-CO-15"], "{bad}");
    assert_eq!(findings[0]["line"], json!(3), "the invoice element starts on line 3: {}", findings[0]);
    assert!(findings[0]["test"].as_str().is_some_and(|t| t.contains("TaxInclusiveAmount")));
    let stage = |name: &str| bad["stages"].as_array().and_then(|s| s.iter().find(|s| s["stage"] == json!(name)).cloned()).expect("stage");
    assert_eq!((stage("schema")["status"].clone(), stage("schema")["reason"].clone()), (json!("not-run"), json!("not asked for")));
    assert_eq!(stage("en16931")["status"], json!("failed"));
}

#[test]
fn einvoice_validate_reads_a_file_in_the_root_and_an_open_document_and_refuses_what_is_wrong() {
    let dir = tempfile::tempdir().expect("dir");
    std::fs::write(dir.path().join("rechnung.xml"), UBL_XRECHNUNG).expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let by_file = einvoice_validate(&mut a, json!({ "path": "rechnung.xml" }));
    assert_eq!(by_file["valid"], json!(true), "{by_file}");
    let opened = a.call("doc_open", &json!({ "path": "rechnung.xml" })).expect("opens");
    let Some(Content::Json(opened)) = opened.into_iter().next() else { panic!("json") };
    let by_id = einvoice_validate(&mut a, json!({ "id": opened["id"] }));
    assert_eq!(by_id["totals"], by_file["totals"]);
    // Outside the root, bad arguments, and something that is not XML are errors an agent can act on.
    for args in [
        json!({ "path": "../outside.xml" }),
        json!({}),
        json!({ "text": "<a/>", "stages": ["everything"] }),
        json!({ "text": "<a/>", "stages": "schema" }),
        json!({ "text": "<a/>", "max_findings": -1 }),
        json!({ "text": "<a/>", "max_findings": 100000 }),
    ] {
        assert!(a.call("einvoice_validate", &args).is_err(), "{args}");
    }
    let not_invoice = einvoice_validate(&mut a, json!({ "text": "<a/>" }));
    assert_eq!((not_invoice["valid"].clone(), not_invoice["detection"]["recognised"].clone()), (json!(false), json!(false)));
    let broken = einvoice_validate(&mut a, json!({ "text": "<a>" }));
    assert_eq!(broken["stages"][0]["status"], json!("failed"), "{broken}");
}
