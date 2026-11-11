//! What the SOAP window works from: the operations of a WSDL, the request made for one (the address, the HTTP headers and the envelope,
//! with a sample body made from the schema of its input), the headers as text a person edits, and what an answer is when it is a SOAP
//! message. Sending is the window's own (it needs the network); nothing here sends.

use xmlcraft_resolve::Resolver;
use xmlcraft_sample::Options;
pub use xmlcraft_wsdl::is_wsdl_namespace;
use xmlcraft_wsdl::{SoapVersion, Style, Wsdl, parse, read_message};
use xmlcraft_xsd::ResolvingLoader;

/// The most operations listed.
pub const MAX_OPERATIONS: usize = 2_000;
/// The most header lines read.
const MAX_HEADERS: usize = 100;

/// One operation of a description as the window lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationEntry {
    pub name: String,
    /// The binding, which tells the SOAP version when an operation is bound more than once.
    pub binding: Option<String>,
    /// `1.1` or `1.2`.
    pub soap: &'static str,
    pub rpc: bool,
}

impl OperationEntry {
    /// The text the list shows: `Add (CalcSoap12, SOAP 1.2)`.
    pub fn label(&self) -> String {
        match &self.binding {
            Some(binding) => format!("{} ({binding}, SOAP {})", self.name, self.soap),
            None => format!("{} (SOAP {})", self.name, self.soap),
        }
    }
}

fn soap_name(version: SoapVersion) -> &'static str {
    match version {
        SoapVersion::V11 => "1.1",
        SoapVersion::V12 => "1.2",
    }
}

/// The operations of the WSDL `text` that have a SOAP form, and what was left out.
pub fn operations(text: &str) -> Result<(Vec<OperationEntry>, Vec<String>), String> {
    let wsdl = parse(text).map_err(|e| e.to_string())?;
    let entries = wsdl
        .operations
        .iter()
        .take(MAX_OPERATIONS)
        .map(|o| OperationEntry { name: o.name.clone(), binding: o.binding.clone(), soap: soap_name(o.soap), rpc: o.style == Style::Rpc })
        .collect();
    Ok((entries, wsdl.warnings))
}

/// A request made for an operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    /// The address of the endpoint, when the description gives one.
    pub url: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub warnings: Vec<String>,
}

fn find<'a>(wsdl: &'a Wsdl, operation: &str, binding: Option<&str>) -> Result<&'a xmlcraft_wsdl::Operation, String> {
    wsdl.operation(operation, binding).ok_or_else(|| {
        let known: Vec<&str> = wsdl.operations.iter().take(20).map(|o| o.name.as_str()).collect();
        format!("the description has no operation {operation}; it has: {}", known.join(", "))
    })
}

/// Make the request of `operation` (of `binding`, when it is bound more than once) of the WSDL `text`, which is at `location`. The
/// schemas it imports are read through `resolver`.
pub fn prepare(text: &str, location: &str, operation: &str, binding: Option<&str>, resolver: &Resolver) -> Result<Prepared, String> {
    let wsdl = parse(text).map_err(|e| e.to_string())?;
    let op = find(&wsdl, operation, binding)?;
    let mut loader = ResolvingLoader::new(resolver);
    let (request, warnings) = wsdl.sample_request(op, &Options::default(), location, &mut loader);
    Ok(Prepared { url: request.url, headers: request.headers, body: request.body, warnings })
}

/// Header lines (`Name: value`, one to a line) as pairs; a blank line is skipped, a line with no colon or a name that is not a header
/// name is an error naming the line.
pub fn parse_headers(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if out.len() >= MAX_HEADERS {
            return Err(format!("more than {MAX_HEADERS} headers"));
        }
        let Some((name, value)) = line.split_once(':') else { return Err(format!("line {}: a header is written Name: value", i + 1)) };
        let name = name.trim();
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)) {
            return Err(format!("line {}: {name:?} is not a header name", i + 1));
        }
        if value.contains(['\r', '\n']) {
            return Err(format!("line {}: a header value is one line", i + 1));
        }
        out.push((name.to_owned(), value.trim().to_owned()));
    }
    Ok(out)
}

/// Headers as the text a person edits, one to a line.
pub fn headers_text(headers: &[(String, String)]) -> String {
    headers.iter().map(|(n, v)| format!("{n}: {v}")).collect::<Vec<_>>().join("\n")
}

/// What an answer is when its body is a SOAP message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub version: &'static str,
    /// `code: reason` when the message is a fault.
    pub fault: Option<String>,
}

/// Read `body` as a SOAP message: its version and, for a fault, its code and reason. None when it is not one.
pub fn summarize(body: &str) -> Option<Summary> {
    let message = read_message(body).ok()?;
    let fault = message.fault.map(|f| format!("{}: {}", f.code, f.reason));
    Some(Summary { version: soap_name(message.version), fault })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::sync::Arc;

    use xmlcraft_resolve::{Buffers, Environment, FileAccess, FixedClock, Policy};

    use super::*;

    const WSDL: &str = r#"<?xml version="1.0"?>
<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/" xmlns:soap="http://schemas.xmlsoap.org/wsdl/soap/"
    xmlns:soap12="http://schemas.xmlsoap.org/wsdl/soap12/" xmlns:xs="http://www.w3.org/2001/XMLSchema"
    xmlns:tns="http://example.com/calc" targetNamespace="http://example.com/calc">
  <wsdl:types><xs:schema targetNamespace="http://example.com/calc" elementFormDefault="qualified">
    <xs:element name="Add"><xs:complexType><xs:sequence><xs:element name="a" type="xs:int"/></xs:sequence></xs:complexType></xs:element>
    <xs:element name="AddResponse"><xs:complexType><xs:sequence><xs:element name="r" type="xs:int"/></xs:sequence></xs:complexType></xs:element>
  </xs:schema></wsdl:types>
  <wsdl:message name="In"><wsdl:part name="p" element="tns:Add"/></wsdl:message>
  <wsdl:message name="Out"><wsdl:part name="p" element="tns:AddResponse"/></wsdl:message>
  <wsdl:portType name="P"><wsdl:operation name="Add"><wsdl:input message="tns:In"/><wsdl:output message="tns:Out"/></wsdl:operation></wsdl:portType>
  <wsdl:binding name="B11" type="tns:P"><soap:binding style="document" transport="http://schemas.xmlsoap.org/soap/http"/>
    <wsdl:operation name="Add"><soap:operation soapAction="urn:add"/><wsdl:input><soap:body use="literal"/></wsdl:input><wsdl:output><soap:body use="literal"/></wsdl:output></wsdl:operation></wsdl:binding>
  <wsdl:binding name="B12" type="tns:P"><soap12:binding style="document" transport="http://schemas.xmlsoap.org/soap/http"/>
    <wsdl:operation name="Add"><soap12:operation soapAction="urn:add"/><wsdl:input><soap12:body use="literal"/></wsdl:input><wsdl:output><soap12:body use="literal"/></wsdl:output></wsdl:operation></wsdl:binding>
  <wsdl:service name="S"><wsdl:port name="B11" binding="tns:B11"><soap:address location="http://example.com/s11"/></wsdl:port>
    <wsdl:port name="B12" binding="tns:B12"><soap12:address location="http://example.com/s12"/></wsdl:port></wsdl:service>
</wsdl:definitions>"#;

    struct NoFiles;

    impl FileAccess for NoFiles {
        fn read(&self, path: &str) -> Result<Vec<u8>, String> {
            Err(format!("{path}: no such file"))
        }
    }

    struct NoBuffers;

    impl Buffers for NoBuffers {
        fn text(&self, _: &str) -> Option<Arc<str>> {
            None
        }
    }

    fn resolver() -> Resolver {
        let env = Environment { buffers: Arc::new(NoBuffers), files: Arc::new(NoFiles), fetcher: None, cache: None, clock: Arc::new(FixedClock(0)) };
        Resolver::new(env, Policy { offline: true, ..Policy::default() }, &[])
    }

    #[test]
    fn an_operation_bound_twice_is_listed_once_for_each_soap_version() {
        let (entries, warnings) = operations(WSDL).expect("reads");
        assert!(warnings.is_empty(), "{warnings:?}");
        let labels: Vec<String> = entries.iter().map(OperationEntry::label).collect();
        assert_eq!(labels, ["Add (B11, SOAP 1.1)", "Add (B12, SOAP 1.2)"]);
        assert!(operations("<not-a-wsdl/>").is_err());
    }

    #[test]
    fn a_request_has_the_address_the_headers_and_an_envelope_of_the_bindings_version() {
        let r = resolver();
        let p11 = prepare(WSDL, "", "Add", Some("B11"), &r).expect("prepares");
        assert_eq!(p11.url.as_deref(), Some("http://example.com/s11"));
        assert!(p11.body.contains("http://schemas.xmlsoap.org/soap/envelope/") && p11.body.contains(":a>1</"), "{}", p11.body);
        assert!(p11.headers.iter().any(|(n, v)| n.eq_ignore_ascii_case("SOAPAction") && v.contains("urn:add")), "{:?}", p11.headers);
        let p12 = prepare(WSDL, "", "Add", Some("B12"), &r).expect("prepares");
        assert_eq!(p12.url.as_deref(), Some("http://example.com/s12"));
        assert!(p12.body.contains("http://www.w3.org/2003/05/soap-envelope"), "{}", p12.body);
        let e = prepare(WSDL, "", "Nope", None, &r).expect_err("no such operation");
        assert!(e.contains("no operation Nope") && e.contains("Add"), "{e}");
    }

    #[test]
    fn headers_are_text_one_to_a_line_and_a_bad_line_is_named() {
        let h = parse_headers("Content-Type: text/xml; charset=utf-8\n\n  SOAPAction : \"urn:add\" \n").expect("parses");
        assert_eq!(h, [("Content-Type".to_owned(), "text/xml; charset=utf-8".to_owned()), ("SOAPAction".to_owned(), "\"urn:add\"".to_owned())]);
        assert_eq!(headers_text(&h), "Content-Type: text/xml; charset=utf-8\nSOAPAction: \"urn:add\"");
        assert!(parse_headers("no colon here").expect_err("bad").contains("line 1"));
        assert!(parse_headers("ok: 1\nbad name: 2").expect_err("bad").contains("line 2"));
        assert!(parse_headers(": empty name").is_err());
        let many = "A: 1\n".repeat(MAX_HEADERS + 1);
        assert!(parse_headers(&many).is_err(), "capped");
    }

    #[test]
    fn an_answer_that_is_a_soap_message_says_its_version_and_its_fault() {
        let fault = r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><s:Fault><faultcode>s:Server</faultcode><faultstring>Boom</faultstring></s:Fault></s:Body></s:Envelope>"#;
        assert_eq!(summarize(fault), Some(Summary { version: "1.1", fault: Some("s:Server: Boom".to_owned()) }));
        let ok = r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope"><s:Body><r/></s:Body></s:Envelope>"#;
        assert_eq!(summarize(ok), Some(Summary { version: "1.2", fault: None }));
        assert_eq!(summarize("<html/>"), None);
        assert_eq!(summarize("not xml <"), None);
    }
}
