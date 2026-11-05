//! `wsdl_describe` and `soap_sample`, through the tool table and the root confinement.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, tool: &str, args: Value) -> Result<Value, ToolError> {
    a.call(tool, &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

const WSDL: &str = r#"<definitions xmlns="http://schemas.xmlsoap.org/wsdl/" xmlns:soap="http://schemas.xmlsoap.org/wsdl/soap/" xmlns:soap12="http://schemas.xmlsoap.org/wsdl/soap12/" xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:tns="urn:c" targetNamespace="urn:c">
  <types><xs:schema targetNamespace="urn:c" xmlns:o="urn:other" elementFormDefault="qualified"><xs:import namespace="urn:other" schemaLocation="other.xsd"/>
    <xs:element name="Add"><xs:complexType><xs:sequence><xs:element name="a" type="xs:int"/><xs:element name="extra" type="o:Code"/></xs:sequence></xs:complexType></xs:element></xs:schema></types>
  <message name="In"><part name="p" element="tns:Add"/></message>
  <portType name="P"><operation name="Add"><input message="tns:In"/></operation></portType>
  <binding name="B11" type="tns:P"><soap:binding style="document" transport="http://schemas.xmlsoap.org/soap/http"/><operation name="Add"><soap:operation soapAction="urn:add"/></operation></binding>
  <binding name="B12" type="tns:P"><soap12:binding style="document" transport="http://schemas.xmlsoap.org/soap/http"/><operation name="Add"><soap12:operation soapAction="urn:add"/></operation></binding>
  <service name="S"><port name="B11" binding="tns:B11"><soap:address location="http://example.com/c"/></port></service>
</definitions>"#;

const OTHER: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:other"><xs:simpleType name="Code"><xs:restriction base="xs:string"><xs:enumeration value="OK"/></xs:restriction></xs:simpleType></xs:schema>"#;

#[test]
fn the_operations_are_described_with_their_soap_version_and_endpoint() {
    let mut a = Automation::new();
    let r = call(&mut a, "wsdl_describe", json!({ "text": WSDL })).expect("runs");
    assert_eq!((r["version"].as_str(), r["operations"].as_array().map(Vec::len), r["schemas"].as_u64()), (Some("1.1"), Some(2), Some(1)), "{r}");
    let op = &r["operations"][0];
    assert_eq!(
        (op["name"].as_str(), op["soap"].as_str(), op["style"].as_str(), op["soap_action"].as_str(), op["address"].as_str()),
        (Some("Add"), Some("1.1"), Some("document"), Some("urn:add"), Some("http://example.com/c"))
    );
    assert_eq!(r["operations"][1]["soap"].as_str(), Some("1.2"));
    assert_eq!(op["input"][0]["element"].as_str(), Some("{urn:c}Add"));
}

#[test]
fn a_request_is_made_with_schemas_imported_by_location_from_beside_the_wsdl() {
    let dir = tempfile::tempdir().expect("dir");
    std::fs::write(dir.path().join("c.wsdl"), WSDL).expect("writes");
    std::fs::write(dir.path().join("other.xsd"), OTHER).expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let r = call(&mut a, "soap_sample", json!({ "path": "c.wsdl", "operation": "Add", "binding": "B11" })).expect("runs");
    assert_eq!(r["warnings"], json!([]), "{r}");
    let body = r["body"].as_str().expect("body");
    assert!(body.contains("<ns1:a>") && body.contains("<ns1:extra>OK</ns1:extra>"), "the enumeration of the imported type is used: {body}");
    assert_eq!((r["soap"].as_str(), r["url"].as_str()), (Some("1.1"), Some("http://example.com/c")));
    assert_eq!(r["headers"][1], json!({ "name": "SOAPAction", "value": "\"urn:add\"" }));
    // Without the imported file the request is still made, with the part that needs it said to be missing.
    let r = call(&mut a, "soap_sample", json!({ "text": WSDL, "operation": "Add", "binding": "B12" })).expect("runs");
    assert_eq!(r["soap"].as_str(), Some("1.2"));
    assert!(r["warnings"].as_array().is_some_and(|w| !w.is_empty()), "{r}");
}

#[test]
fn an_unknown_operation_names_the_known_ones_and_other_input_is_refused() {
    let mut a = Automation::new();
    let e = call(&mut a, "soap_sample", json!({ "text": WSDL, "operation": "Nope" })).expect_err("unknown");
    assert!(e.to_string().contains("no operation Nope") && e.to_string().contains("Add"), "{e}");
    let e = call(&mut a, "wsdl_describe", json!({ "text": "<a/>" })).expect_err("not wsdl");
    assert!(e.to_string().contains("not a WSDL"), "{e}");
    assert!(matches!(call(&mut a, "soap_sample", json!({ "text": WSDL })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(call(&mut a, "soap_sample", json!({ "text": WSDL, "operation": "Add", "optional": "yes" })), Err(ToolError::InvalidArgs(_))));
}
