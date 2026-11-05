//! `wsdl_describe` and `soap_sample`: the operations of a web service read from its WSDL, and a SOAP request made for one.
//!
//! The description is an open document (`id`), a file (`path`, through the root confinement) or `text`. Schemas it imports by location
//! are read from files, the open documents and the schemas that ship with the app. Nothing is sent and nothing is changed.

use serde_json::{Value, json};
use xmlcraft_sample::{ChoicePolicy, Fill, Options};
use xmlcraft_wsdl::{Operation, Part, SoapVersion, Style, Wsdl, WsdlVersion, parse};
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
