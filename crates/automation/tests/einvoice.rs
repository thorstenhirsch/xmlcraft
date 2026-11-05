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
