#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! WSDL 1.1 and 2.0: the operations of a web service read from its description, and SOAP 1.1 and 1.2 requests made from them.
//!
//! - **Layer:** L4, no UI and no I/O (schemas a description imports are found through a [`SchemaLoader`] the caller gives), so it compiles
//!   for the web.
//! - [`parse`] reads a description into a [`Wsdl`]: its operations (each with the binding's SOAP version, action and style, the parts of
//!   its input and output, and the address of the endpoint that serves it), its endpoints, and the XML Schemas embedded in its `types`.
//! - [`Wsdl::sample_request`] makes the envelope of a request for an operation: the body is a sample made from the schema of the input
//!   (`document` style) or the operation element with a part each (`rpc` style), with the HTTP headers the SOAP version needs.
//! - What a description cannot say (a binding that is not SOAP, an operation with no binding, an unknown message) is left out or is a
//!   warning of the description, never a panic.

mod message;

pub use message::{Fault, Message, read_message};

use std::collections::HashMap;

use xmlcraft_sample::{Options as SampleOptions, QName, sample_from_schema};
use xmlcraft_syntax::{Severity, parse as parse_xml};
use xmlcraft_xsd::dom::{self, Dom, El};
use xmlcraft_xsd::{LoadRequest, Loaded, SchemaLoader, assemble, load};

const WSDL11: &str = "http://schemas.xmlsoap.org/wsdl/";
const WSDL20: &str = "http://www.w3.org/ns/wsdl";
const SOAP11_BINDING: &str = "http://schemas.xmlsoap.org/wsdl/soap/";
const SOAP12_BINDING: &str = "http://schemas.xmlsoap.org/wsdl/soap12/";
const WSOAP: &str = "http://www.w3.org/ns/wsdl/soap";
const XSD: &str = "http://www.w3.org/2001/XMLSchema";
/// The SOAP 1.1 envelope namespace.
pub const SOAP11_ENVELOPE: &str = "http://schemas.xmlsoap.org/soap/envelope/";
/// The SOAP 1.2 envelope namespace.
pub const SOAP12_ENVELOPE: &str = "http://www.w3.org/2003/05/soap-envelope";

/// The most operations a description is read for.
const MAX_OPERATIONS: usize = 20_000;

/// A description that cannot be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WsdlError(pub String);

impl std::fmt::Display for WsdlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WsdlError {}

/// Which version of WSDL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WsdlVersion {
    V11,
    V20,
}

/// Which version of SOAP an operation is bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoapVersion {
    V11,
    V12,
}

impl SoapVersion {
    /// The namespace of its envelope.
    pub fn envelope_namespace(self) -> &'static str {
        match self {
            SoapVersion::V11 => SOAP11_ENVELOPE,
            SoapVersion::V12 => SOAP12_ENVELOPE,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            SoapVersion::V11 => "SOAP 1.1",
            SoapVersion::V12 => "SOAP 1.2",
        }
    }
}

/// How the body of a message is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// The body is the element of the message part.
    Document,
    /// The body is an element named for the operation, with an element for each part.
    Rpc,
}

/// A part of a message: an element, or a name with a type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    pub name: String,
    pub element: Option<QName>,
    pub type_name: Option<QName>,
}

/// One operation, as bound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Operation {
    pub name: String,
    /// The port type (WSDL 1.1) or interface (WSDL 2.0) it belongs to.
    pub interface: String,
    /// The binding that gives it a SOAP form.
    pub binding: Option<String>,
    pub soap: SoapVersion,
    pub style: Style,
    pub soap_action: Option<String>,
    /// For `rpc` style: the namespace of the operation element.
    pub namespace: Option<String>,
    pub input: Vec<Part>,
    pub output: Vec<Part>,
    /// The names of the faults it declares.
    pub faults: Vec<String>,
    /// Where the service is, from the endpoint (or port) that uses the binding.
    pub address: Option<String>,
}

/// An endpoint (WSDL 1.1: a port).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub service: String,
    pub name: String,
    pub binding: String,
    pub address: Option<String>,
}

/// A schema embedded in the `types` of a description.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedSchema {
    pub namespace: Option<String>,
    pub text: String,
}

/// A description, read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wsdl {
    pub version: WsdlVersion,
    pub target_namespace: Option<String>,
    pub operations: Vec<Operation>,
    pub endpoints: Vec<Endpoint>,
    pub schemas: Vec<EmbeddedSchema>,
    /// What was left out and why.
    pub warnings: Vec<String>,
}

fn attr<'a>(el: &'a El, name: &str) -> Option<&'a str> {
    el.attrs.iter().find(|a| a.namespace.is_none() && a.local == name).map(|a| a.value.as_str())
}

/// The expanded name an attribute value spells (`tns:Order`), with the prefixes of the element it is on.
fn qname(tree: &Dom, at: usize, value: &str) -> QName {
    let value = value.trim();
    match value.split_once(':') {
        Some((prefix, local)) => QName::new(tree.lookup(at, Some(prefix)).as_deref(), local),
        None => QName::new(tree.lookup(at, None).as_deref(), value),
    }
}

/// The children of `el` in `namespace` called `local`, with their index.
fn kids<'a>(tree: &'a Dom, el: &El, namespace: &'a str, local: &'a str) -> impl Iterator<Item = (usize, &'a El)> + 'a {
    el.children
        .clone()
        .into_iter()
        .filter_map(move |i| tree.elements.get(i).map(|c| (i, c)))
        .filter(move |(_, c)| c.local == local && c.namespace.as_deref() == Some(namespace))
}

fn kid_in<'a>(tree: &'a Dom, el: &El, namespaces: &[&str], local: &str) -> Option<&'a El> {
    el.children
        .iter()
        .filter_map(|i| tree.elements.get(*i))
        .find(|c| c.local == local && c.namespace.as_deref().is_some_and(|n| namespaces.contains(&n)))
}

/// Read a WSDL 1.1 or 2.0 description.
pub fn parse(text: &str) -> Result<Wsdl, WsdlError> {
    let parsed = parse_xml(text);
    if parsed.diagnostics().iter().any(|d| d.severity() == Severity::Error) {
        return Err(WsdlError("the description is not well-formed XML".to_owned()));
    }
    let tree = dom::build(&parsed.syntax());
    let Some(root) = tree.elements.first() else { return Err(WsdlError("the description has no root element".to_owned())) };
    match (root.namespace.as_deref(), root.local.as_str()) {
        (Some(WSDL11), "definitions") => parse_11(text, &tree),
        (Some(WSDL20), "description") => parse_20(text, &tree),
        _ => Err(WsdlError("this is not a WSDL description (the root is not definitions of WSDL 1.1 or description of WSDL 2.0)".to_owned())),
    }
}

fn schemas_of(text: &str, tree: &Dom, root: &El, types_ns: &str, out: &mut Vec<EmbeddedSchema>) {
    for (_, types) in kids(tree, root, types_ns, "types") {
        for i in &types.children {
            let Some(schema) = tree.elements.get(*i) else { continue };
            if schema.local == "schema" && schema.namespace.as_deref() == Some(XSD) {
                // The text of the element as written, with the namespace declarations of the elements around it that it may use.
                let own = text.get(schema.range.clone()).unwrap_or_default().to_owned();
                out.push(EmbeddedSchema { namespace: attr(schema, "targetNamespace").map(str::to_owned), text: with_declarations(tree, *i, &own) });
            }
        }
    }
}

/// An embedded schema uses the prefixes declared on the `definitions` around it: they are declared on the schema itself (those it does not
/// declare already) so that it stands alone.
fn with_declarations(tree: &Dom, at: usize, own: &str) -> String {
    let scope = tree.in_scope(at);
    let own_declared: Vec<&str> =
        tree.elements.get(at).map(|e| e.declarations.iter().filter_map(|(p, _)| p.as_deref()).collect()).unwrap_or_default();
    let mut extra = String::new();
    for (prefix, uri) in &scope {
        let name = prefix.as_deref().unwrap_or("");
        let declared_here = tree.elements.get(at).is_some_and(|e| e.declarations.iter().any(|(p, _)| p.as_deref() == prefix.as_deref()));
        if declared_here || own_declared.contains(&name) {
            continue;
        }
        let escaped = uri.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;");
        match prefix {
            Some(p) => extra.push_str(&format!(" xmlns:{p}=\"{escaped}\"")),
            None => extra.push_str(&format!(" xmlns=\"{escaped}\"")),
        }
    }
    if extra.is_empty() {
        return own.to_owned();
    }
    // Insert after the element name of the start tag.
    let name_end = own.find(|c: char| c.is_whitespace() || c == '>' || c == '/').unwrap_or(own.len());
    let (head, tail) = own.split_at_checked(name_end).unwrap_or((own, ""));
    format!("{head}{extra}{tail}")
}

fn parse_11(text: &str, tree: &Dom) -> Result<Wsdl, WsdlError> {
    let Some(root) = tree.elements.first() else { return Err(WsdlError("empty".to_owned())) };
    let target_namespace = attr(root, "targetNamespace").map(str::to_owned);
    let mut wsdl = Wsdl {
        version: WsdlVersion::V11,
        target_namespace,
        operations: Vec::new(),
        endpoints: Vec::new(),
        schemas: Vec::new(),
        warnings: Vec::new(),
    };
    schemas_of(text, tree, root, WSDL11, &mut wsdl.schemas);
    // Messages: name → parts.
    let mut messages: HashMap<String, Vec<Part>> = HashMap::new();
    for (_, message) in kids(tree, root, WSDL11, "message") {
        let Some(name) = attr(message, "name") else { continue };
        let parts = kids(tree, message, WSDL11, "part")
            .filter_map(|(i, p)| {
                let part_name = attr(p, "name")?.to_owned();
                Some(Part {
                    name: part_name,
                    element: attr(p, "element").map(|v| qname(tree, i, v)),
                    type_name: attr(p, "type").map(|v| qname(tree, i, v)),
                })
            })
            .collect();
        messages.insert(name.to_owned(), parts);
    }
    let parts_of = |index: usize, el: &El, wsdl: &mut Wsdl| -> Vec<Part> {
        let Some(reference) = attr(el, "message") else { return Vec::new() };
        let local = qname(tree, index, reference).local;
        match messages.get(&local) {
            Some(parts) => parts.clone(),
            None => {
                wsdl.warnings.push(format!("the message {local} is not defined"));
                Vec::new()
            }
        }
    };
    // Port types: name → operations (name, input parts, output parts, fault names).
    type PortOp = (Vec<Part>, Vec<Part>, Vec<String>);
    let mut port_types: HashMap<String, HashMap<String, PortOp>> = HashMap::new();
    for (_, port_type) in kids(tree, root, WSDL11, "portType") {
        let Some(pt_name) = attr(port_type, "name") else { continue };
        let mut ops: HashMap<String, PortOp> = HashMap::new();
        for (_, op) in kids(tree, port_type, WSDL11, "operation") {
            let Some(name) = attr(op, "name") else { continue };
            let (mut input, mut output, mut faults) = (Vec::new(), Vec::new(), Vec::new());
            for (i, io) in kids(tree, op, WSDL11, "input") {
                input = parts_of(i, io, &mut wsdl);
            }
            for (i, io) in kids(tree, op, WSDL11, "output") {
                output = parts_of(i, io, &mut wsdl);
            }
            for (_, f) in kids(tree, op, WSDL11, "fault") {
                if let Some(n) = attr(f, "name") {
                    faults.push(n.to_owned());
                }
            }
            ops.insert(name.to_owned(), (input, output, faults));
        }
        port_types.insert(pt_name.to_owned(), ops);
    }
    // Services: ports with the binding they use and their address.
    for (_, service) in kids(tree, root, WSDL11, "service") {
        let service_name = attr(service, "name").unwrap_or_default().to_owned();
        for (i, port) in kids(tree, service, WSDL11, "port") {
            let Some(binding) = attr(port, "binding") else { continue };
            let address = kid_in(tree, port, &[SOAP11_BINDING, SOAP12_BINDING], "address").and_then(|a| attr(a, "location")).map(str::to_owned);
            wsdl.endpoints.push(Endpoint {
                service: service_name.clone(),
                name: attr(port, "name").unwrap_or_default().to_owned(),
                binding: qname(tree, i, binding).local,
                address,
            });
        }
    }
    // Bindings: each operation of a SOAP binding.
    for (_, binding) in kids(tree, root, WSDL11, "binding") {
        let Some(binding_name) = attr(binding, "name").map(str::to_owned) else { continue };
        let Some(soap_el) = kid_in(tree, binding, &[SOAP11_BINDING, SOAP12_BINDING], "binding") else {
            wsdl.warnings.push(format!("the binding {binding_name} is not a SOAP binding, so its operations are left out"));
            continue;
        };
        let soap = if soap_el.namespace.as_deref() == Some(SOAP12_BINDING) { SoapVersion::V12 } else { SoapVersion::V11 };
        let default_style = if attr(soap_el, "style") == Some("rpc") { Style::Rpc } else { Style::Document };
        let Some(pt_ref) = attr(binding, "type") else { continue };
        let binding_index = tree.elements.iter().position(|e| std::ptr::eq(e, binding)).unwrap_or(0);
        let pt_name = qname(tree, binding_index, pt_ref).local;
        let address = wsdl.endpoints.iter().find(|e| e.binding == binding_name).and_then(|e| e.address.clone());
        for (_, op) in kids(tree, binding, WSDL11, "operation") {
            if wsdl.operations.len() >= MAX_OPERATIONS {
                wsdl.warnings.push("the description has more operations than are read".to_owned());
                break;
            }
            let Some(name) = attr(op, "name") else { continue };
            let soap_op = kid_in(tree, op, &[SOAP11_BINDING, SOAP12_BINDING], "operation");
            let style = match soap_op.and_then(|s| attr(s, "style")) {
                Some("rpc") => Style::Rpc,
                Some("document") => Style::Document,
                _ => default_style,
            };
            let namespace = kid_in(tree, op, &[WSDL11], "input")
                .and_then(|i| kid_in(tree, i, &[SOAP11_BINDING, SOAP12_BINDING], "body"))
                .and_then(|b| attr(b, "namespace"))
                .map(str::to_owned);
            let (input, output, faults) = port_types.get(&pt_name).and_then(|ops| ops.get(name)).cloned().unwrap_or_default();
            wsdl.operations.push(Operation {
                name: name.to_owned(),
                interface: pt_name.clone(),
                binding: Some(binding_name.clone()),
                soap,
                style,
                soap_action: soap_op.and_then(|s| attr(s, "soapAction")).map(str::to_owned),
                namespace,
                input,
                output,
                faults,
                address: address.clone(),
            });
        }
    }
    Ok(wsdl)
}

fn parse_20(text: &str, tree: &Dom) -> Result<Wsdl, WsdlError> {
    let Some(root) = tree.elements.first() else { return Err(WsdlError("empty".to_owned())) };
    let mut wsdl = Wsdl {
        version: WsdlVersion::V20,
        target_namespace: attr(root, "targetNamespace").map(str::to_owned),
        operations: Vec::new(),
        endpoints: Vec::new(),
        schemas: Vec::new(),
        warnings: Vec::new(),
    };
    schemas_of(text, tree, root, WSDL20, &mut wsdl.schemas);
    // Interfaces: name → operations (name, input elements, output elements, faults).
    type InterfaceOp = (Vec<Part>, Vec<Part>, Vec<String>);
    let mut interfaces: HashMap<String, HashMap<String, InterfaceOp>> = HashMap::new();
    for (_, interface) in kids(tree, root, WSDL20, "interface") {
        let Some(iname) = attr(interface, "name") else { continue };
        let mut ops: HashMap<String, InterfaceOp> = HashMap::new();
        for (_, op) in kids(tree, interface, WSDL20, "operation") {
            let Some(name) = attr(op, "name") else { continue };
            let part = |label: &str, i: usize, el: &El| -> Part {
                let element = attr(el, "element").filter(|e| !matches!(*e, "#any" | "#none" | "#other")).map(|e| qname(tree, i, e));
                Part { name: label.to_owned(), element, type_name: None }
            };
            let input: Vec<Part> = kids(tree, op, WSDL20, "input").map(|(i, el)| part("in", i, el)).collect();
            let output: Vec<Part> = kids(tree, op, WSDL20, "output").map(|(i, el)| part("out", i, el)).collect();
            let faults: Vec<String> = kids(tree, op, WSDL20, "outfault").filter_map(|(_, f)| attr(f, "ref")).map(str::to_owned).collect();
            ops.insert(name.to_owned(), (input, output, faults));
        }
        interfaces.insert(iname.to_owned(), ops);
    }
    for (_, service) in kids(tree, root, WSDL20, "service") {
        let service_name = attr(service, "name").unwrap_or_default().to_owned();
        for (i, endpoint) in kids(tree, service, WSDL20, "endpoint") {
            let Some(binding) = attr(endpoint, "binding") else { continue };
            wsdl.endpoints.push(Endpoint {
                service: service_name.clone(),
                name: attr(endpoint, "name").unwrap_or_default().to_owned(),
                binding: qname(tree, i, binding).local,
                address: attr(endpoint, "address").map(str::to_owned),
            });
        }
    }
    for (bi, binding) in kids(tree, root, WSDL20, "binding") {
        let Some(binding_name) = attr(binding, "name").map(str::to_owned) else { continue };
        if attr(binding, "type") != Some(WSOAP) {
            wsdl.warnings.push(format!("the binding {binding_name} is not a SOAP binding, so its operations are left out"));
            continue;
        }
        // SOAP 1.2 unless the binding says 1.1.
        let soap = match binding.attrs.iter().find(|a| a.namespace.as_deref() == Some(WSOAP) && a.local == "version").map(|a| a.value.as_str()) {
            Some("1.1") => SoapVersion::V11,
            _ => SoapVersion::V12,
        };
        let Some(iface_ref) = attr(binding, "interface") else { continue };
        let iname = qname(tree, bi, iface_ref).local;
        let address = wsdl.endpoints.iter().find(|e| e.binding == binding_name).and_then(|e| e.address.clone());
        for (oi, op) in kids(tree, binding, WSDL20, "operation") {
            let Some(reference) = attr(op, "ref") else { continue };
            let name = qname(tree, oi, reference).local;
            let action = op.attrs.iter().find(|a| a.namespace.as_deref() == Some(WSOAP) && a.local == "action").map(|a| a.value.clone());
            let (input, output, faults) = interfaces.get(&iname).and_then(|ops| ops.get(&name)).cloned().unwrap_or_default();
            if wsdl.operations.len() >= MAX_OPERATIONS {
                break;
            }
            wsdl.operations.push(Operation {
                name,
                interface: iname.clone(),
                binding: Some(binding_name.clone()),
                soap,
                style: Style::Document,
                soap_action: action,
                namespace: None,
                input,
                output,
                faults,
                address: address.clone(),
            });
        }
    }
    Ok(wsdl)
}

/// A request, ready to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoapRequest {
    /// Where to send it (the address of the endpoint), when the description says.
    pub url: Option<String>,
    /// The HTTP headers it needs (`Content-Type`, and `SOAPAction` for SOAP 1.1).
    pub headers: Vec<(String, String)>,
    /// The envelope.
    pub body: String,
    pub soap: SoapVersion,
}

/// Finds the schemas of a description that the embedded ones import by location.
struct Schemas<'a> {
    embedded: &'a [EmbeddedSchema],
    outside: &'a mut dyn SchemaLoader,
}

impl SchemaLoader for Schemas<'_> {
    fn load(&mut self, request: &LoadRequest<'_>) -> Result<Loaded, String> {
        // An import by namespace finds the embedded schema of that namespace first.
        if let Some(namespace) = request.namespace
            && let Some(found) = self.embedded.iter().find(|s| s.namespace.as_deref() == Some(namespace))
        {
            return Ok(Loaded { text: found.text.clone(), location: format!("wsdl:{namespace}") });
        }
        self.outside.load(request)
    }
}

fn strip_declaration(text: &str) -> &str {
    let t = text.trim_start();
    match t.strip_prefix("<?xml").and_then(|rest| rest.find("?>").map(|e| rest.get(e + 2..).unwrap_or_default())) {
        Some(rest) => rest.trim_start(),
        None => t,
    }
}

/// The qualified name of a prefix in an envelope: SOAP's own.
fn envelope(soap: SoapVersion, body: &str) -> String {
    let ns = soap.envelope_namespace();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<soapenv:Envelope xmlns:soapenv=\"{ns}\">\n  <soapenv:Header/>\n  <soapenv:Body>\n{body}  </soapenv:Body>\n</soapenv:Envelope>\n"
    )
}

fn indent(text: &str, by: &str) -> String {
    text.lines().map(|l| format!("{by}{l}\n")).collect()
}

impl Wsdl {
    /// The operation called `name` (of the binding `binding` when the name is used in more than one).
    pub fn operation(&self, name: &str, binding: Option<&str>) -> Option<&Operation> {
        self.operations.iter().find(|o| o.name == name && binding.is_none_or(|b| o.binding.as_deref() == Some(b)))
    }

    /// The envelope of a request for `operation`: a sample body made from the schemas of the description (those it embeds, and those a
    /// schema imports by location through `outside`, relative to `base`, where the description is). Warnings say what could not be made as it should.
    pub fn sample_request(
        &self,
        operation: &Operation,
        options: &SampleOptions,
        base: &str,
        outside: &mut dyn SchemaLoader,
    ) -> (SoapRequest, Vec<String>) {
        let mut warnings = Vec::new();
        let mut content = String::new();
        match operation.style {
            Style::Document => {
                for part in &operation.input {
                    match &part.element {
                        Some(element) => match self.sample_element(element, options, base, outside) {
                            Ok((text, mut w)) => {
                                content.push_str(&indent(strip_declaration(&text).trim_end(), "    "));
                                warnings.append(&mut w);
                            }
                            Err(why) => {
                                warnings.push(format!("the part {} could not be made: {why}", part.name));
                                content.push_str(&format!("    <!-- {} -->\n", part.name));
                            }
                        },
                        None => {
                            warnings.push(format!("the part {} has a type, not an element; it is a placeholder", part.name));
                            content.push_str(&format!("    <{0}>?</{0}>\n", part.name));
                        }
                    }
                }
            }
            Style::Rpc => {
                let ns = operation.namespace.as_deref().or(self.target_namespace.as_deref()).unwrap_or_default();
                content.push_str(&format!("    <ns:{} xmlns:ns=\"{}\">\n", operation.name, ns.replace('&', "&amp;").replace('"', "&quot;")));
                for part in &operation.input {
                    match &part.element {
                        Some(element) => match self.sample_element(element, options, base, outside) {
                            Ok((text, mut w)) => {
                                content.push_str(&indent(strip_declaration(&text).trim_end(), "      "));
                                warnings.append(&mut w);
                            }
                            Err(why) => warnings.push(format!("the part {} could not be made: {why}", part.name)),
                        },
                        None => content.push_str(&format!("      <{0}>?</{0}>\n", part.name)),
                    }
                }
                content.push_str(&format!("    </ns:{}>\n", operation.name));
            }
        }
        let body = envelope(operation.soap, &content);
        let headers = match operation.soap {
            SoapVersion::V11 => vec![
                ("Content-Type".to_owned(), "text/xml; charset=utf-8".to_owned()),
                ("SOAPAction".to_owned(), format!("\"{}\"", operation.soap_action.clone().unwrap_or_default())),
            ],
            SoapVersion::V12 => {
                let action = operation.soap_action.as_deref().filter(|a| !a.is_empty());
                let content_type = match action {
                    Some(a) => format!("application/soap+xml; charset=utf-8; action=\"{a}\""),
                    None => "application/soap+xml; charset=utf-8".to_owned(),
                };
                vec![("Content-Type".to_owned(), content_type)]
            }
        };
        (SoapRequest { url: operation.address.clone(), headers, body, soap: operation.soap }, warnings)
    }

    fn sample_element(
        &self,
        element: &QName,
        options: &SampleOptions,
        base: &str,
        outside: &mut dyn SchemaLoader,
    ) -> Result<(String, Vec<String>), String> {
        let root = self
            .schemas
            .iter()
            .find(|s| s.namespace == element.namespace)
            .or_else(|| self.schemas.first())
            .ok_or_else(|| "the description has no embedded schema".to_owned())?;
        let mut loader = Schemas { embedded: &self.schemas, outside };
        let documents = load(&root.text, if base.is_empty() { "wsdl:types" } else { base }, &mut loader);
        let assembly = assemble(&documents);
        let errors: Vec<String> = documents
            .diagnostics
            .iter()
            .chain(&assembly.diagnostics)
            .filter(|d| d.diagnostic.severity() == Severity::Error)
            .map(|d| d.diagnostic.message())
            .take(3)
            .collect();
        if !errors.is_empty() {
            return Err(format!("the schema has errors: {}", errors.join(" ")));
        }
        let made = sample_from_schema(&assembly.schema, Some(element), options).map_err(|e| e.to_string())?;
        Ok((made.text, made.warnings))
    }
}

/// A loader that finds no other document (a description whose schemas are all embedded).
pub struct NoSchemas;

impl SchemaLoader for NoSchemas {
    fn load(&mut self, _: &LoadRequest<'_>) -> Result<Loaded, String> {
        Err("other documents are not read here".to_owned())
    }
}
