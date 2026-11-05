//! `openapi_describe`, `openapi_request` and `openapi_call`: the operations of an OpenAPI description, a request built for one, and the
//! request sent.
//!
//! The description is an open document (`id`), a file (`path`, through the root confinement) or `text` (JSON or YAML, by the document's
//! language or the `language` argument). Nothing is sent by the first two tools.

use serde_json::{Map, Value, json};
use xmlcraft_openapi::{Api, BuiltRequest, Operation, RequestInput, parse};
use xmlcraft_platform::client::{Request, send};

use crate::documents::{fail, json_result, language_source_properties, source};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

fn request_properties() -> Value {
    let mut properties = language_source_properties();
    if let Some(map) = properties.as_object_mut() {
        map.insert("operation".into(), json!({ "type": "string", "description": "The operationId, or `method path` (`get /pets/{id}`)" }));
        map.insert(
            "server".into(),
            json!({ "type": "string", "description": "The address to send to instead of the first server of the description" }),
        );
        for (name, what) in
            [("path_parameters", "Values of the path parameters"), ("query", "Values of the query parameters"), ("headers", "Values of the headers")]
        {
            map.insert(name.into(), json!({ "type": "object", "additionalProperties": { "type": "string" }, "description": what }));
        }
        map.insert("body".into(), json!({ "type": "string", "description": "The request body (default: an example made from the schema)" }));
        map.insert(
            "content_type".into(),
            json!({ "type": "string", "description": "The media type of the body (default: the first the operation takes)" }),
        );
    }
    properties
}

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut call_properties = request_properties();
    if let Some(map) = call_properties.as_object_mut() {
        map.insert("allow_private_hosts".into(), json!({ "type": "boolean", "description": "Allow sending to this computer or the local network" }));
        map.insert("timeout_seconds".into(), json!({ "type": "integer", "minimum": 1, "maximum": 300 }));
    }
    vec![
        ToolDef {
            name: "openapi_describe",
            title: "Read the operations of an OpenAPI description",
            description: "Read an OpenAPI 2.0 (Swagger), 3.0, 3.1 or 3.2 description (JSON or YAML): its `version`, `title`, `servers` (with their variables resolved), `operations` (id, \
                          method, path, summary, tags, deprecated, parameters with their place and whether they are required, the media types of the request body) and `warnings`. \
                          Local `$ref`s are followed. Nothing is sent.",
            input_schema: json!({ "type": "object", "properties": language_source_properties(), "additionalProperties": false }),
            read_only: true,
            destructive: false,
            handler: openapi_describe,
        },
        ToolDef {
            name: "openapi_request",
            title: "Build the HTTP request of an operation",
            description: "Build the request of an operation: `method`, `url` (the server, the path with its parameters filled in and percent-encoded, the query), `headers` and `body`. \
                          Values not given are examples made from the schemas (a path parameter, a required query or header parameter, a body from the request schema). A required value \
                          that is missing is an error. Nothing is sent.",
            input_schema: json!({ "type": "object", "properties": request_properties(), "required": ["operation"], "additionalProperties": false }),
            read_only: true,
            destructive: false,
            handler: openapi_request,
        },
        ToolDef {
            name: "openapi_call",
            title: "Send the request of an operation",
            description: "Build the request of an operation as openapi_request does and send it; returns the request that was sent and the HTTP `response` (`status`, `headers`, `body`, \
                          `elapsed_ms`, and `json` when the body is JSON). This sends a request over the network, which a person must have allowed for the session; a host on this \
                          computer or the local network is refused unless `allow_private_hosts` is true. Redirects are not followed.",
            input_schema: json!({ "type": "object", "properties": call_properties, "required": ["operation"], "additionalProperties": false }),
            read_only: false,
            destructive: false,
            handler: openapi_call,
        },
    ]
}

fn read(a: &Automation, args: &Value) -> Result<Api> {
    let s = source(a, args)?;
    // (The default language of `text` is XML; a description is JSON or YAML, and the text says which: a JSON one starts with a brace.)
    let yaml = s.language.is_yaml() || (s.language.is_xml() && !s.text.trim_start().starts_with(['{', '[']));
    parse(&s.text, yaml).map_err(|e| ToolError::Failed(e.to_string()))
}

fn operation_json(o: &Operation) -> Value {
    json!({
        "id": o.id,
        "method": o.method,
        "path": o.path,
        "summary": o.summary,
        "tags": o.tags,
        "deprecated": o.deprecated,
        "parameters": o.parameters.iter().map(|p| json!({ "name": p.name, "in": p.location.name(), "required": p.required, "description": p.description })).collect::<Vec<_>>(),
        "bodies": o.bodies.iter().map(|b| json!({ "content_type": b.content_type, "required": b.required })).collect::<Vec<_>>(),
    })
}

fn openapi_describe(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let api = read(a, args)?;
    json_result(json!({
        "version": api.version.name(),
        "title": api.title,
        "servers": api.servers.iter().map(|s| json!({ "url": s.url, "resolved": s.resolved(), "description": s.description })).collect::<Vec<_>>(),
        "operations": api.operations.iter().map(operation_json).collect::<Vec<_>>(),
        "warnings": api.warnings,
    }))
}

fn strings(args: &Value, key: &str) -> Result<std::collections::BTreeMap<String, String>> {
    match args.get(key) {
        None => Ok(Default::default()),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(k, v)| {
                v.as_str().map(|s| (k.clone(), s.to_owned())).ok_or_else(|| ToolError::InvalidArgs(format!("{key}: the value of {k} is a string")))
            })
            .collect(),
        Some(_) => Err(ToolError::InvalidArgs(format!("{key} is an object of names and values"))),
    }
}

fn build(a: &Automation, args: &Value) -> Result<(Api, Operation, BuiltRequest)> {
    let id = args.get("operation").and_then(Value::as_str).ok_or_else(|| ToolError::InvalidArgs("`operation` is required".into()))?;
    let api = read(a, args)?;
    let Some(operation) = api.operation(id).cloned() else {
        let known: Vec<&str> = api.operations.iter().map(|o| o.id.as_str()).take(40).collect();
        return fail(format!("the description has no operation {id}; it has: {}", known.join(", ")));
    };
    let mut input: RequestInput = api.sample_input(&operation);
    input.path.extend(strings(args, "path_parameters")?);
    input.query.extend(strings(args, "query")?);
    input.headers.extend(strings(args, "headers")?);
    match args.get("body") {
        None => {}
        Some(Value::String(b)) => input.body = Some(b.clone()),
        Some(_) => return Err(ToolError::InvalidArgs("body is a string".into())),
    }
    match args.get("content_type") {
        None => {}
        Some(Value::String(c)) => input.content_type = Some(c.clone()),
        Some(_) => return Err(ToolError::InvalidArgs("content_type is a string".into())),
    }
    let server = args.get("server").and_then(Value::as_str);
    let built = api.build_request(&operation, &input, server).map_err(ToolError::Failed)?;
    Ok((api, operation, built))
}

fn built_json(b: &BuiltRequest) -> Value {
    json!({
        "method": b.method,
        "url": b.url,
        "headers": b.headers.iter().map(|(n, v)| json!({ "name": n, "value": v })).collect::<Vec<_>>(),
        "body": String::from_utf8_lossy(&b.body),
    })
}

fn openapi_request(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let (_, operation, built) = build(a, args)?;
    json_result(json!({ "operation": operation_json(&operation), "request": built_json(&built) }))
}

fn openapi_call(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
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
    if !a.network().can_download() || a.network().is_offline() {
        return fail(
            "the session is offline, so nothing is sent: a person has to allow the network (the program gives the session a network that sends)",
        );
    }
    let (_, operation, built) = build(a, args)?;
    let mut request = Request::new(&built.method, &built.url);
    request.headers = built.headers.clone();
    request.body = built.body.clone();
    request.allow_private_hosts = allow_private;
    request.timeout = std::time::Duration::from_secs(timeout);
    let response = send("XmlCraft", &request).map_err(ToolError::Failed)?;
    let decoded = xmlcraft_text::encoding::decode(&response.body, &xmlcraft_text::encoding::sniff(&response.body));
    let json_body: Option<Value> = serde_json::from_str(&decoded.text).ok();
    let mut out = Map::new();
    out.insert("operation".into(), operation_json(&operation));
    out.insert("request".into(), built_json(&built));
    out.insert(
        "response".into(),
        json!({
            "status": response.status,
            "reason": response.reason,
            "headers": response.headers.iter().map(|(n, v)| json!({ "name": n, "value": v })).collect::<Vec<_>>(),
            "body": decoded.text,
            "json": json_body,
            "elapsed_ms": u64::try_from(response.elapsed.as_millis()).unwrap_or(u64::MAX),
        }),
    );
    json_result(Value::Object(out))
}
