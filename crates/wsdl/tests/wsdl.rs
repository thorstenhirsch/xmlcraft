//! WSDL 1.1 and 2.0 read, and SOAP requests made from them.

use xmlcraft_sample::Options;
use xmlcraft_wsdl::{NoSchemas, SoapVersion, Style, WsdlVersion, parse};

const CALCULATOR: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/" xmlns:soap="http://schemas.xmlsoap.org/wsdl/soap/"
    xmlns:soap12="http://schemas.xmlsoap.org/wsdl/soap12/" xmlns:xs="http://www.w3.org/2001/XMLSchema"
    xmlns:tns="http://example.com/calc" targetNamespace="http://example.com/calc">
  <wsdl:types>
    <xs:schema targetNamespace="http://example.com/calc" elementFormDefault="qualified">
      <xs:element name="Add"><xs:complexType><xs:sequence><xs:element name="a" type="xs:int"/><xs:element name="b" type="xs:int"/></xs:sequence></xs:complexType></xs:element>
      <xs:element name="AddResponse"><xs:complexType><xs:sequence><xs:element name="result" type="xs:int"/></xs:sequence></xs:complexType></xs:element>
    </xs:schema>
  </wsdl:types>
  <wsdl:message name="AddIn"><wsdl:part name="parameters" element="tns:Add"/></wsdl:message>
  <wsdl:message name="AddOut"><wsdl:part name="parameters" element="tns:AddResponse"/></wsdl:message>
  <wsdl:portType name="CalcPort">
    <wsdl:operation name="Add"><wsdl:input message="tns:AddIn"/><wsdl:output message="tns:AddOut"/><wsdl:fault name="Overflow" message="tns:AddOut"/></wsdl:operation>
  </wsdl:portType>
  <wsdl:binding name="CalcSoap11" type="tns:CalcPort">
    <soap:binding style="document" transport="http://schemas.xmlsoap.org/soap/http"/>
    <wsdl:operation name="Add"><soap:operation soapAction="http://example.com/calc/Add"/><wsdl:input><soap:body use="literal"/></wsdl:input><wsdl:output><soap:body use="literal"/></wsdl:output></wsdl:operation>
  </wsdl:binding>
  <wsdl:binding name="CalcSoap12" type="tns:CalcPort">
    <soap12:binding style="document" transport="http://schemas.xmlsoap.org/soap/http"/>
    <wsdl:operation name="Add"><soap12:operation soapAction="http://example.com/calc/Add"/><wsdl:input><soap12:body use="literal"/></wsdl:input><wsdl:output><soap12:body use="literal"/></wsdl:output></wsdl:operation>
  </wsdl:binding>
  <wsdl:service name="Calc">
    <wsdl:port name="CalcSoap11" binding="tns:CalcSoap11"><soap:address location="http://example.com/calc11"/></wsdl:port>
    <wsdl:port name="CalcSoap12" binding="tns:CalcSoap12"><soap12:address location="http://example.com/calc12"/></wsdl:port>
  </wsdl:service>
</wsdl:definitions>"#;

#[test]
fn a_wsdl_11_description_gives_an_operation_for_each_soap_binding_with_its_endpoint() {
    let w = parse(CALCULATOR).expect("reads");
    assert_eq!((w.version, w.operations.len(), w.endpoints.len(), w.schemas.len()), (WsdlVersion::V11, 2, 2, 1));
    let (a, b) = (&w.operations[0], &w.operations[1]);
    assert_eq!(
        (a.name.as_str(), a.soap, a.style, a.binding.as_deref(), a.address.as_deref()),
        ("Add", SoapVersion::V11, Style::Document, Some("CalcSoap11"), Some("http://example.com/calc11"))
    );
    assert_eq!((b.soap, b.address.as_deref()), (SoapVersion::V12, Some("http://example.com/calc12")));
    assert_eq!(a.soap_action.as_deref(), Some("http://example.com/calc/Add"));
    assert_eq!((a.input.len(), a.output.len(), a.faults.as_slice()), (1, 1, ["Overflow".to_owned()].as_slice()));
    assert_eq!(a.input[0].element.as_ref().map(|q| q.local.as_str()), Some("Add"));
    assert!(w.warnings.is_empty(), "{:?}", w.warnings);
    assert_eq!(w.operation("Add", Some("CalcSoap12")).map(|o| o.soap), Some(SoapVersion::V12));
    assert!(w.operation("Nope", None).is_none());
}

#[test]
fn a_request_has_an_envelope_a_sample_body_and_the_headers_of_its_soap_version() {
    let w = parse(CALCULATOR).expect("reads");
    let (r11, warnings) = w.sample_request(&w.operations[0], &Options::default(), "", &mut NoSchemas);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(r11.url.as_deref(), Some("http://example.com/calc11"));
    assert!(r11.body.contains("xmlns:soapenv=\"http://schemas.xmlsoap.org/soap/envelope/\""), "{}", r11.body);
    assert!(
        r11.body.contains("<ns1:Add xmlns:ns1=\"http://example.com/calc\">") && r11.body.contains("<ns1:a>") && r11.body.contains("<ns1:b>"),
        "{}",
        r11.body
    );
    assert_eq!(
        r11.headers,
        [("Content-Type".to_owned(), "text/xml; charset=utf-8".to_owned()), ("SOAPAction".to_owned(), "\"http://example.com/calc/Add\"".to_owned())]
    );
    let (r12, _) = w.sample_request(&w.operations[1], &Options::default(), "", &mut NoSchemas);
    assert!(r12.body.contains("http://www.w3.org/2003/05/soap-envelope"), "{}", r12.body);
    assert_eq!(r12.headers, [("Content-Type".to_owned(), "application/soap+xml; charset=utf-8; action=\"http://example.com/calc/Add\"".to_owned())]);
    // The envelope is well-formed XML and the body element is in the namespace of the schema.
    let parsed = xmlcraft_syntax::parse(&r11.body);
    assert!(parsed.diagnostics().iter().all(|d| d.severity() != xmlcraft_syntax::Severity::Error), "{:?}", parsed.diagnostics());
}

#[test]
fn rpc_style_wraps_the_parts_in_an_element_named_for_the_operation() {
    let rpc = r#"<definitions xmlns="http://schemas.xmlsoap.org/wsdl/" xmlns:soap="http://schemas.xmlsoap.org/wsdl/soap/" xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:tns="urn:g" targetNamespace="urn:g">
      <message name="HelloIn"><part name="who" type="xs:string"/><part name="times" type="xs:int"/></message>
      <portType name="P"><operation name="Hello"><input message="tns:HelloIn"/></operation></portType>
      <binding name="B" type="tns:P"><soap:binding style="rpc" transport="http://schemas.xmlsoap.org/soap/http"/>
        <operation name="Hello"><soap:operation soapAction=""/><input><soap:body use="literal" namespace="urn:greeting"/></input></operation></binding>
      <service name="S"><port name="B" binding="tns:B"><soap:address location="http://example.com/g"/></port></service></definitions>"#;
    let w = parse(rpc).expect("reads");
    let op = &w.operations[0];
    assert_eq!((op.style, op.namespace.as_deref()), (Style::Rpc, Some("urn:greeting")));
    let (r, warnings) = w.sample_request(op, &Options::default(), "", &mut NoSchemas);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(
        r.body.contains("<ns:Hello xmlns:ns=\"urn:greeting\">") && r.body.contains("<who>?</who>") && r.body.contains("<times>?</times>"),
        "{}",
        r.body
    );
    assert_eq!(r.headers[1], ("SOAPAction".to_owned(), "\"\"".to_owned()));
}

#[test]
fn a_wsdl_20_description_is_soap_12_unless_it_says_1_1() {
    let w20 = r#"<description xmlns="http://www.w3.org/ns/wsdl" xmlns:wsoap="http://www.w3.org/ns/wsdl/soap" xmlns:xs="http://www.w3.org/2001/XMLSchema" xmlns:t="urn:t" targetNamespace="urn:t">
      <types><xs:schema targetNamespace="urn:t" elementFormDefault="qualified"><xs:element name="Ping"><xs:complexType><xs:sequence><xs:element name="msg" type="xs:string"/></xs:sequence></xs:complexType></xs:element><xs:element name="Pong" type="xs:string"/></xs:schema></types>
      <interface name="I"><operation name="Ping" pattern="http://www.w3.org/ns/wsdl/in-out"><input element="t:Ping"/><output element="t:Pong"/></operation></interface>
      <binding name="B12" interface="t:I" type="http://www.w3.org/ns/wsdl/soap" wsoap:protocol="http://www.w3.org/2003/05/soap/bindings/HTTP/"><operation ref="t:Ping" wsoap:action="urn:ping"/></binding>
      <binding name="B11" interface="t:I" type="http://www.w3.org/ns/wsdl/soap" wsoap:version="1.1"><operation ref="t:Ping"/></binding>
      <binding name="Rest" interface="t:I" type="http://www.w3.org/ns/wsdl/http"><operation ref="t:Ping"/></binding>
      <service name="S" interface="t:I"><endpoint name="e" binding="t:B12" address="http://example.com/ping"/></service></description>"#;
    let w = parse(w20).expect("reads");
    assert_eq!((w.version, w.operations.len()), (WsdlVersion::V20, 2));
    assert_eq!(
        (w.operations[0].soap, w.operations[0].soap_action.as_deref(), w.operations[0].address.as_deref()),
        (SoapVersion::V12, Some("urn:ping"), Some("http://example.com/ping"))
    );
    assert_eq!(w.operations[1].soap, SoapVersion::V11);
    assert_eq!(w.warnings.len(), 1, "{:?}", w.warnings);
    assert!(w.warnings[0].contains("Rest") && w.warnings[0].contains("not a SOAP binding"), "{:?}", w.warnings);
    let (r, warnings) = w.sample_request(&w.operations[0], &Options::default(), "", &mut NoSchemas);
    assert!(warnings.is_empty() && r.body.contains("<ns1:msg>"), "{warnings:?} {}", r.body);
}

#[test]
fn what_is_not_wsdl_or_not_xml_is_refused_and_what_is_missing_is_a_warning_not_a_stop() {
    for (text, why) in [("<a/>", "not a WSDL"), ("<definitions", "not well-formed"), ("", "not well-formed")] {
        let e = parse(text).expect_err(text);
        assert!(e.0.contains(why), "{text:?}: {e}");
    }
    let missing = r#"<definitions xmlns="http://schemas.xmlsoap.org/wsdl/" xmlns:soap="http://schemas.xmlsoap.org/wsdl/soap/" xmlns:tns="urn:g" targetNamespace="urn:g">
      <portType name="P"><operation name="Op"><input message="tns:Nowhere"/></operation></portType>
      <binding name="B" type="tns:P"><soap:binding/><operation name="Op"><soap:operation/></operation></binding></definitions>"#;
    let w = parse(missing).expect("reads");
    assert_eq!(w.operations.len(), 1);
    assert!(w.warnings.iter().any(|m| m.contains("Nowhere")), "{:?}", w.warnings);
    // With no schema at all, a part with an element is a warning and a comment in the body.
    let with_element = r#"<definitions xmlns="http://schemas.xmlsoap.org/wsdl/" xmlns:soap="http://schemas.xmlsoap.org/wsdl/soap/" xmlns:tns="urn:g" targetNamespace="urn:g">
      <message name="M"><part name="p" element="tns:E"/></message>
      <portType name="P"><operation name="Op"><input message="tns:M"/></operation></portType>
      <binding name="B" type="tns:P"><soap:binding/><operation name="Op"><soap:operation/></operation></binding></definitions>"#;
    let w = parse(with_element).expect("reads");
    let (r, warnings) = w.sample_request(&w.operations[0], &Options::default(), "", &mut NoSchemas);
    assert!(warnings.iter().any(|m| m.contains("no embedded schema")), "{warnings:?}");
    assert!(r.body.contains("<!-- p -->"), "{}", r.body);
}

proptest::proptest! {
    #[test]
    fn any_text_is_answered_without_a_panic(text in ".{0,300}") {
        if let Ok(w) = parse(&text) {
            for op in &w.operations {
                let _ = w.sample_request(op, &Options::default(), "", &mut NoSchemas);
            }
        }
    }
}
