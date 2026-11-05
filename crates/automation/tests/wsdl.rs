//! `wsdl_describe` and `soap_sample`, through the tool table and the root confinement.

use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;

use xmlcraft_automation::{Automation, Content, Network, ToolError};

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

/// A server on this computer that answers one request and hands back what it received.
fn serve(answer: String) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let address = listener.local_addr().expect("address");
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accepts");
        let mut received = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            let n = stream.read(&mut buffer).expect("reads");
            received.extend_from_slice(buffer.get(..n).unwrap_or_default());
            let text = String::from_utf8_lossy(&received).into_owned();
            if let Some(split) = text.find("\r\n\r\n") {
                let length = text
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()))
                    .unwrap_or(0);
                if n == 0 || received.len() >= split + 4 + length {
                    break;
                }
            }
            if n == 0 {
                break;
            }
        }
        stream.write_all(answer.as_bytes()).expect("writes");
        String::from_utf8_lossy(&received).into_owned()
    });
    (format!("http://{address}/calc"), handle)
}

#[test]
fn a_call_sends_the_envelope_with_the_headers_of_its_version_and_reads_a_fault_from_the_answer() {
    let fault = "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><s:Fault><faultcode>s:Server</faultcode><faultstring>Overflow</faultstring></s:Fault></s:Body></s:Envelope>";
    let (url, server) = serve(format!(
        "HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/xml; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{fault}",
        fault.len()
    ));
    let dir = tempfile::tempdir().expect("dir");
    std::fs::write(dir.path().join("c.wsdl"), WSDL).expect("writes");
    std::fs::write(dir.path().join("other.xsd"), OTHER).expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("root").with_network(Network::standard());
    let r = call(&mut a, "soap_call", json!({ "path": "c.wsdl", "operation": "Add", "binding": "B11", "url": url, "allow_private_hosts": true, "headers": { "Authorization": "Bearer t" } })).expect("runs");
    assert_eq!((r["response"]["status"].as_u64(), r["response"]["reason"].as_str()), (Some(500), Some("Internal Server Error")), "{r}");
    assert_eq!(
        (r["soap"]["version"].as_str(), r["soap"]["fault"]["code"].as_str(), r["soap"]["fault"]["reason"].as_str()),
        (Some("1.1"), Some("s:Server"), Some("Overflow")),
        "{r}"
    );
    let received = server.join().expect("server");
    let lower = received.to_ascii_lowercase();
    assert!(received.starts_with("POST /calc HTTP/1.1"), "{received}");
    assert!(
        lower.contains("soapaction: \"urn:add\"")
            && lower.contains("authorization: bearer t")
            && lower.contains("content-type: text/xml; charset=utf-8"),
        "{received}"
    );
    assert!(received.contains("<ns1:Add") && received.contains("<ns1:extra>OK</ns1:extra>"), "the sample envelope is the body: {received}");
    assert!(r["request"]["body"].as_str().is_some_and(|b| b.contains("soapenv:Envelope")), "{r}");
}

#[test]
fn nothing_is_sent_offline_or_to_this_computer_unless_allowed_and_a_given_envelope_is_what_is_sent() {
    let mut offline = Automation::new();
    let e = call(&mut offline, "soap_call", json!({ "text": WSDL, "operation": "Add", "url": "http://127.0.0.1:1/x" })).expect_err("offline");
    assert!(e.to_string().contains("offline"), "{e}");
    let mut online = Automation::new().with_network(Network::standard());
    let e = call(&mut online, "soap_call", json!({ "text": WSDL, "operation": "Add", "url": "http://127.0.0.1:1/x" })).expect_err("private");
    assert!(e.to_string().contains("allow private hosts"), "{e}");
    let e = call(&mut online, "soap_call", json!({ "text": WSDL, "operation": "Add", "binding": "B12" })).expect_err("no address");
    assert!(e.to_string().contains("no address"), "{e}");
    assert!(matches!(
        call(&mut online, "soap_call", json!({ "text": WSDL, "operation": "Add", "timeout_seconds": 0 })),
        Err(ToolError::InvalidArgs(_))
    ));
    let (url, server) = serve("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_owned());
    let r =
        call(&mut online, "soap_call", json!({ "text": WSDL, "operation": "Add", "url": url, "allow_private_hosts": true, "envelope": "<mine/>" }))
            .expect("runs");
    assert_eq!((r["response"]["status"].as_u64(), r["response"]["body"].as_str(), r["soap"].clone()), (Some(200), Some("ok"), Value::Null));
    assert!(server.join().expect("server").ends_with("<mine/>"));
}
