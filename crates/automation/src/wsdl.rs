//! `wsdl_describe` and `soap_sample`: the operations of a web service read from its WSDL, and a SOAP request made for one.
//!
//! The description is an open document (`id`), a file (`path`, through the root confinement) or `text`. Schemas it imports by location
//! are read from files, the open documents and the schemas that ship with the app. Nothing is sent and nothing is changed.

use serde_json::{Value, json};
use xmlcraft_platform::client::{Request, send};
use xmlcraft_sample::{ChoicePolicy, Fill, Options};
use xmlcraft_wsdl::{Operation, Part, SoapVersion, Style, Wsdl, WsdlVersion, parse, read_message};
use xmlcraft_xsd::ResolvingLoader;

use crate::documents::{fail, json_result, source, source_properties};
use crate::tools::ToolDef;
use crate::validate::schema_resolver;
use crate::{Automation, Content, Result, ToolError};

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut sample_properties = source_properties();
    if let Some(map) = sample_properties.as_object_mut() {
        map.insert("operation".into(), json!({ "type": "string", "description": "The operation to make a request for" }));
        map.insert(
            "binding".into(),
            json!({ "type": "string", "description": "The binding, when the operation is bound more than once (SOAP 1.1 and 1.2, say)" }),
        );
        map.insert("optional".into(), json!({ "type": "boolean", "description": "Include the optional elements (default true)" }));
    }
    let mut call_properties = sample_properties.clone();
    if let Some(map) = call_properties.as_object_mut() {
        map.insert(
            "envelope".into(),
            json!({ "type": "string", "description": "The envelope to send instead of the sample (edit the sample from soap_sample)" }),
        );
        map.insert("url".into(), json!({ "type": "string", "description": "The address to send to instead of the endpoint of the WSDL" }));
        map.insert(
            "headers".into(),
            json!({ "type": "object", "additionalProperties": { "type": "string" }, "description": "Extra HTTP headers (Authorization, say)" }),
        );
        map.insert("allow_private_hosts".into(), json!({ "type": "boolean", "description": "Allow sending to this computer or the local network" }));
        map.insert("timeout_seconds".into(), json!({ "type": "integer", "minimum": 1, "maximum": 300 }));
    }
    vec![
        ToolDef {
            name: "wsdl_describe",
            title: "Read the operations of a WSDL",
            description: "Read a WSDL 1.1 or 2.0 description: its `operations` (name, binding, SOAP version 1.1 or 1.2, style document or rpc, SOAP action, the address of \
                          its endpoint, the parts of its input and output, the faults it declares), its `endpoints` (services and ports), the number of embedded \
                          `schemas`, and `warnings` for what was left out (a binding that is not SOAP, a message that is not defined). Nothing is sent.",
            input_schema: json!({ "type": "object", "properties": source_properties(), "additionalProperties": false }),
            read_only: true,
            destructive: false,
            handler: wsdl_describe,
        },
        ToolDef {
            name: "soap_sample",
            title: "Make a SOAP request for an operation",
            description: "Make the SOAP request of an operation of a WSDL: the `body` (an envelope, SOAP 1.1 or 1.2 as the binding says, with a sample made from the schema \
                          of the input), the HTTP `headers` it needs (Content-Type, and SOAPAction for 1.1) and the `url` of the endpoint. Nothing is sent.",
            input_schema: json!({ "type": "object", "properties": sample_properties, "required": ["operation"], "additionalProperties": false }),
            read_only: true,
            destructive: false,
            handler: soap_sample,
        },
        ToolDef {
            name: "soap_call",
            title: "Send a SOAP request",
            description: "Send the SOAP request of an operation of a WSDL to its endpoint (or `url`) and read the answer: the request that was sent (`url`, `headers`, `body`), \
                          the HTTP `response` (`status`, `headers`, `body`, `elapsed_ms`) and, when the body is a SOAP message, `soap`: its version, header blocks, body and \
                          `fault` (code, subcode, reason, actor, detail). The `body` is the sample envelope unless `envelope` is given. SOAP 1.1 faults come with HTTP 500: that is an answer, \
                          not an error. This sends a request over the network, which a person must have allowed for the session; a host on this computer or the local network \
                          is refused unless `allow_private_hosts` is true. Redirects are not followed.",
            input_schema: json!({ "type": "object", "properties": call_properties, "required": ["operation"], "additionalProperties": false }),
            read_only: false,
            destructive: false,
            handler: soap_call,
        },
    ]
}

fn parts_json(parts: &[Part]) -> Vec<Value> {
    parts
        .iter()
        .map(|p| json!({ "name": p.name, "element": p.element.as_ref().map(ToString::to_string), "type": p.type_name.as_ref().map(ToString::to_string) }))
        .collect()
}

fn soap_name(v: SoapVersion) -> &'static str {
    match v {
        SoapVersion::V11 => "1.1",
        SoapVersion::V12 => "1.2",
    }
}

fn operation_json(o: &Operation) -> Value {
    json!({
        "name": o.name,
        "interface": o.interface,
        "binding": o.binding,
        "soap": soap_name(o.soap),
        "style": match o.style { Style::Document => "document", Style::Rpc => "rpc" },
        "soap_action": o.soap_action,
        "address": o.address,
        "input": parts_json(&o.input),
        "output": parts_json(&o.output),
        "faults": o.faults,
    })
}

fn read(a: &Automation, args: &Value) -> Result<(Wsdl, String)> {
    let s = source(a, args)?;
    let location = s.path.as_deref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let wsdl = parse(&s.text).map_err(|e| ToolError::Failed(e.to_string()))?;
    Ok((wsdl, location))
}

fn wsdl_describe(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let (wsdl, _) = read(a, args)?;
    json_result(json!({
        "version": match wsdl.version { WsdlVersion::V11 => "1.1", WsdlVersion::V20 => "2.0" },
        "target_namespace": wsdl.target_namespace,
        "operations": wsdl.operations.iter().map(operation_json).collect::<Vec<_>>(),
        "endpoints": wsdl.endpoints.iter().map(|e| json!({ "service": e.service, "name": e.name, "binding": e.binding, "address": e.address })).collect::<Vec<_>>(),
        "schemas": wsdl.schemas.len(),
        "warnings": wsdl.warnings,
    }))
}

fn soap_sample(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let operation = args.get("operation").and_then(Value::as_str).ok_or_else(|| ToolError::InvalidArgs("`operation` is required".into()))?;
    let binding = match args.get("binding") {
        None => None,
        Some(Value::String(b)) => Some(b.as_str()),
        Some(_) => return Err(ToolError::InvalidArgs("binding is a string".into())),
    };
    let optional = match args.get("optional") {
        None => true,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(ToolError::InvalidArgs("optional is true or false".into())),
    };
    let (wsdl, location) = read(a, args)?;
    let Some(op) = wsdl.operation(operation, binding) else {
        let known: Vec<&str> = wsdl.operations.iter().map(|o| o.name.as_str()).collect();
        return fail(format!("the description has no operation {operation}; it has: {}", known.join(", ")));
    };
    let resolver = schema_resolver(a);
    let mut loader = ResolvingLoader::new(&resolver);
    let options = Options { optional, repeat: 1, choice: ChoicePolicy::First, fill: Fill::Typed, max_depth: 6, schema_location: None };
    let (request, warnings) = wsdl.sample_request(op, &options, &location, &mut loader);
    json_result(json!({
        "operation": operation_json(op),
        "url": request.url,
        "soap": soap_name(request.soap),
        "headers": request.headers.iter().map(|(n, v)| json!({ "name": n, "value": v })).collect::<Vec<_>>(),
        "body": request.body,
        "warnings": warnings,
    }))
}

fn soap_call(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let operation = args.get("operation").and_then(Value::as_str).ok_or_else(|| ToolError::InvalidArgs("`operation` is required".into()))?;
    let binding = match args.get("binding") {
        None => None,
        Some(Value::String(b)) => Some(b.as_str()),
        Some(_) => return Err(ToolError::InvalidArgs("binding is a string".into())),
    };
    let text_arg = |key: &str| -> Result<Option<String>> {
        match args.get(key) {
            None => Ok(None),
            Some(Value::String(t)) => Ok(Some(t.clone())),
            Some(_) => Err(ToolError::InvalidArgs(format!("{key} is a string"))),
        }
    };
    let (envelope, url_override) = (text_arg("envelope")?, text_arg("url")?);
    let allow_private = match args.get("allow_private_hosts") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(ToolError::InvalidArgs("allow_private_hosts is true or false".into())),
    };
    let timeout = match args.get("timeout_seconds") {
        None => 30,
        Some(v) => v
            .as_u64()
            .filter(|n| (1..=300).contains(n))
            .ok_or_else(|| ToolError::InvalidArgs("timeout_seconds is a whole number from 1 to 300".into()))?,
    };
    let mut extra: Vec<(String, String)> = Vec::new();
    match args.get("headers") {
        None => {}
        Some(Value::Object(map)) => {
            for (name, value) in map {
                let value = value.as_str().ok_or_else(|| ToolError::InvalidArgs(format!("the header {name} is a string")))?;
                extra.push((name.clone(), value.to_owned()));
            }
        }
        Some(_) => return Err(ToolError::InvalidArgs("headers is an object of names and values".into())),
    }
    if !a.network().can_download() || a.network().is_offline() {
        return fail(
            "the session is offline, so nothing is sent: a person has to allow the network (the program gives the session a network that sends)",
        );
    }
    let (wsdl, location) = read(a, args)?;
    let Some(op) = wsdl.operation(operation, binding) else {
        let known: Vec<&str> = wsdl.operations.iter().map(|o| o.name.as_str()).collect();
        return fail(format!("the description has no operation {operation}; it has: {}", known.join(", ")));
    };
    let resolver = schema_resolver(a);
    let mut loader = ResolvingLoader::new(&resolver);
    let (mut request, warnings) = wsdl.sample_request(op, &Options::default(), &location, &mut loader);
    if let Some(envelope) = envelope {
        request.body = envelope;
    }
    let Some(url) = url_override.or_else(|| request.url.clone()) else {
        return fail("the description gives no address for this operation: pass `url`");
    };
    let mut out = Request::new("POST", &url);
    out.headers = request.headers.clone();
    for (name, value) in extra {
        out.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
        out.headers.push((name, value));
    }
    out.body = request.body.clone().into_bytes();
    out.allow_private_hosts = allow_private;
    out.timeout = std::time::Duration::from_secs(timeout);
    let response = send("XmlCraft", &out).map_err(ToolError::Failed)?;
    let decoded = xmlcraft_text::encoding::decode(&response.body, &xmlcraft_text::encoding::sniff(&response.body));
    let soap = read_message(&decoded.text).ok().map(|m| {
        json!({
            "version": soap_name(m.version),
            "headers": m.headers,
            "body": m.body,
            "fault": m.fault.map(|f| json!({ "code": f.code, "subcode": f.subcode, "reason": f.reason, "actor": f.actor, "detail": f.detail })),
        })
    });
    json_result(json!({
        "request": { "url": url, "headers": out.headers.iter().map(|(n, v)| json!({ "name": n, "value": v })).collect::<Vec<_>>(), "body": request.body },
        "response": {
            "status": response.status,
            "reason": response.reason,
            "headers": response.headers.iter().map(|(n, v)| json!({ "name": n, "value": v })).collect::<Vec<_>>(),
            "body": decoded.text,
            "elapsed_ms": u64::try_from(response.elapsed.as_millis()).unwrap_or(u64::MAX),
        },
        "soap": soap,
        "warnings": warnings,
    }))
}
