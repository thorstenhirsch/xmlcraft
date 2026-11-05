#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! OpenAPI 2.0 (Swagger), 3.0, 3.1 and 3.2: the operations of a description, and requests built from them.
//!
//! - **Layer:** L4, no UI and no I/O, so it compiles for the web.
//! - [`parse`] reads a description in JSON or YAML into an [`Api`]: its version, its servers (for 2.0 from `host`, `basePath` and
//!   `schemes`), and each operation with its parameters (path, query, header, cookie) and request body. Local `$ref`s are followed.
//! - [`example_of`] makes an example value from a JSON Schema (an `example`, a `default`, the first `enum` value, the properties that are
//!   required and the optional ones, an item of an array, the first of `oneOf` or `anyOf`, `allOf` merged); a recursive schema is cut.
//! - [`Api::build_request`] makes the HTTP request of an operation from the values given (path parameters filled in and percent-encoded,
//!   query parameters, headers, a body), and [`Api::sample_input`] gives those values as examples.
//! - A description that cannot be read is an error; one with parts that cannot be used (an unresolvable `$ref`) is read with a warning.

use std::collections::BTreeMap;

use serde_json::{Map, Value};
use xmlcraft_convert::{ConvertOptions, Format, convert};

/// The deepest `$ref` chain or schema nesting that is followed.
const MAX_DEPTH: usize = 12;
/// The most operations read.
const MAX_OPERATIONS: usize = 50_000;

/// A description that cannot be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiError(pub String);

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ApiError {}

/// Which specification the description is written to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    Swagger2,
    OpenApi30,
    OpenApi31,
    OpenApi32,
}

impl Version {
    pub fn name(self) -> &'static str {
        match self {
            Version::Swagger2 => "2.0",
            Version::OpenApi30 => "3.0",
            Version::OpenApi31 => "3.1",
            Version::OpenApi32 => "3.2",
        }
    }
}

/// Where a parameter goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum In {
    Path,
    Query,
    Header,
    Cookie,
}

impl In {
    fn parse(text: &str) -> Option<In> {
        match text {
            "path" => Some(In::Path),
            "query" => Some(In::Query),
            "header" => Some(In::Header),
            "cookie" => Some(In::Cookie),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            In::Path => "path",
            In::Query => "query",
            In::Header => "header",
            In::Cookie => "cookie",
        }
    }
}

/// A parameter of an operation.
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    pub location: In,
    pub required: bool,
    pub description: Option<String>,
    pub schema: Option<Value>,
    pub example: Option<Value>,
}

/// The body an operation takes.
#[derive(Clone, Debug, PartialEq)]
pub struct Body {
    pub content_type: String,
    pub schema: Option<Value>,
    pub example: Option<Value>,
    pub required: bool,
}

/// One operation (a method of a path).
#[derive(Clone, Debug, PartialEq)]
pub struct Operation {
    /// The `operationId`, else the method and the path (`get /pets/{id}`).
    pub id: String,
    /// Upper case: `GET`.
    pub method: String,
    pub path: String,
    pub summary: Option<String>,
    pub tags: Vec<String>,
    pub deprecated: bool,
    pub parameters: Vec<Param>,
    /// The bodies it takes, one per media type (the first is the default).
    pub bodies: Vec<Body>,
}

/// A server a description names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Server {
    /// The address with its variables as written (`https://{host}/v1`).
    pub url: String,
    pub description: Option<String>,
    /// The default of each variable of the address.
    pub variables: BTreeMap<String, String>,
}

impl Server {
    /// The address with each `{variable}` replaced by its default (or kept as written when it has none).
    pub fn resolved(&self) -> String {
        let mut out = self.url.clone();
        for (name, value) in &self.variables {
            out = out.replace(&format!("{{{name}}}"), value);
        }
        out
    }
}

/// A description, read.
#[derive(Clone, Debug, PartialEq)]
pub struct Api {
    pub version: Version,
    pub title: Option<String>,
    pub servers: Vec<Server>,
    pub operations: Vec<Operation>,
    pub warnings: Vec<String>,
    /// The whole description as a value (what `$ref`s point into).
    root: Value,
}

/// Read a description written in JSON or YAML (`yaml`: the text is YAML; else JSON, which may have comments and trailing commas).
pub fn parse(text: &str, yaml: bool) -> Result<Api, ApiError> {
    let root: Value = if yaml || serde_json::from_str::<Value>(text).is_err() {
        let from = if yaml { Format::Yaml } else { Format::Json };
        let json =
            convert(text, from, Format::Json, &ConvertOptions::default()).map_err(|e| ApiError(format!("the description cannot be read: {e}")))?;
        serde_json::from_str(&json.text).map_err(|e| ApiError(format!("the description cannot be read: {e}")))?
    } else {
        serde_json::from_str(text).map_err(|e| ApiError(format!("the description cannot be read: {e}")))?
    };
    Api::from_value(root)
}

fn text_of(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

/// Follow `$ref`s that point into the same document (`#/components/schemas/Pet`); what is not found is `None`.
fn follow<'a>(root: &'a Value, mut value: &'a Value, depth: usize) -> Option<&'a Value> {
    let mut steps = 0;
    while let Some(reference) = value.get("$ref").and_then(Value::as_str) {
        steps += 1;
        if steps > MAX_DEPTH || depth > MAX_DEPTH {
            return None;
        }
        let pointer = reference.strip_prefix('#')?;
        value = root.pointer(pointer)?;
    }
    Some(value)
}

impl Api {
    fn from_value(root: Value) -> Result<Api, ApiError> {
        let version = match (root.get("swagger").and_then(Value::as_str), root.get("openapi").and_then(Value::as_str)) {
            (Some(v), _) if v.starts_with("2.") => Version::Swagger2,
            (_, Some(v)) if v.starts_with("3.0") => Version::OpenApi30,
            (_, Some(v)) if v.starts_with("3.1") => Version::OpenApi31,
            (_, Some(v)) if v.starts_with("3.2") => Version::OpenApi32,
            (Some(v), _) => return Err(ApiError(format!("the Swagger version {v} is not known (2.0 is)"))),
            (_, Some(v)) => return Err(ApiError(format!("the OpenAPI version {v} is not known (3.0, 3.1 and 3.2 are)"))),
            (None, None) => return Err(ApiError("this is not an OpenAPI description: it has no `openapi` or `swagger` member".to_owned())),
        };
        let mut api = Api {
            version,
            title: text_of(root.pointer("/info/title")),
            servers: Vec::new(),
            operations: Vec::new(),
            warnings: Vec::new(),
            root: Value::Null,
        };
        api.servers = servers_of(&root, version);
        let operations = read_operations(&root, version, &mut api.warnings);
        api.operations = operations;
        api.root = root;
        Ok(api)
    }

    /// The operation with this `operationId` (or `method path`, in any case).
    pub fn operation(&self, id: &str) -> Option<&Operation> {
        self.operations.iter().find(|o| o.id == id).or_else(|| self.operations.iter().find(|o| o.id.eq_ignore_ascii_case(id)))
    }

    /// An example value for `schema`, following `$ref`s of this description.
    pub fn example_of(&self, schema: &Value) -> Value {
        example_of(&self.root, schema, 0)
    }

    /// The values to send for `operation`, as examples: each parameter, and the first body.
    pub fn sample_input(&self, operation: &Operation) -> RequestInput {
        let mut input = RequestInput::default();
        for p in &operation.parameters {
            if !p.required && p.location != In::Path {
                continue;
            }
            let value =
                p.example.clone().or_else(|| p.schema.as_ref().map(|s| self.example_of(s))).unwrap_or_else(|| Value::String("value".to_owned()));
            let target = match p.location {
                In::Path => &mut input.path,
                In::Query => &mut input.query,
                In::Header => &mut input.headers,
                In::Cookie => &mut input.headers,
            };
            target.insert(p.name.clone(), scalar_text(&value));
        }
        if let Some(body) = operation.bodies.first() {
            let example = body.example.clone().or_else(|| body.schema.as_ref().map(|s| self.example_of(s)));
            input.content_type = Some(body.content_type.clone());
            input.body = example.map(|v| match &v {
                Value::String(s) if !body.content_type.contains("json") => s.clone(),
                other => serde_json::to_string_pretty(other).unwrap_or_default(),
            });
        }
        input
    }

    /// The HTTP request of `operation` made from `input`: the address of the chosen server (or `server`), the path with its parameters
    /// filled in and percent-encoded, the query, the headers and the body. A required parameter that is missing is an error.
    pub fn build_request(&self, operation: &Operation, input: &RequestInput, server: Option<&str>) -> Result<BuiltRequest, String> {
        let base = match server {
            Some(s) => s.to_owned(),
            None => self.servers.first().map(Server::resolved).ok_or_else(|| "the description names no server: give the address".to_owned())?,
        };
        let mut path = operation.path.clone();
        for p in operation.parameters.iter().filter(|p| p.location == In::Path) {
            let value = input.path.get(&p.name).ok_or_else(|| format!("the path parameter {} is needed", p.name))?;
            path = path.replace(&format!("{{{}}}", p.name), &percent_encode(value, false));
        }
        if path.contains('{') && operation.parameters.iter().all(|p| p.location != In::Path) {
            return Err("the path has parameters the description does not declare".to_owned());
        }
        let mut query: Vec<String> = Vec::new();
        for p in operation.parameters.iter().filter(|p| p.location == In::Query) {
            match input.query.get(&p.name) {
                Some(v) => query.push(format!("{}={}", percent_encode(&p.name, true), percent_encode(v, true))),
                None if p.required => return Err(format!("the query parameter {} is needed", p.name)),
                None => {}
            }
        }
        // Query values the description does not declare are sent too (a person may know better).
        for (name, v) in &input.query {
            if operation.parameters.iter().all(|p| !(p.location == In::Query && &p.name == name)) {
                query.push(format!("{}={}", percent_encode(name, true), percent_encode(v, true)));
            }
        }
        let mut url = format!("{}{}", base.trim_end_matches('/'), if path.starts_with('/') { path } else { format!("/{path}") });
        if !query.is_empty() {
            url.push('?');
            url.push_str(&query.join("&"));
        }
        let mut headers: Vec<(String, String)> = Vec::new();
        for p in operation.parameters.iter().filter(|p| p.location == In::Header) {
            match input.headers.get(&p.name) {
                Some(v) => headers.push((p.name.clone(), v.clone())),
                None if p.required => return Err(format!("the header {} is needed", p.name)),
                None => {}
            }
        }
        for (name, v) in &input.headers {
            if operation.parameters.iter().all(|p| !(p.location == In::Header && &p.name == name)) && !headers.iter().any(|(n, _)| n == name) {
                headers.push((name.clone(), v.clone()));
            }
        }
        let body = input.body.clone().map(String::into_bytes).unwrap_or_default();
        if let Some(content_type) = input.content_type.as_ref().filter(|_| !body.is_empty()) {
            headers.push(("Content-Type".to_owned(), content_type.clone()));
        }
        if operation.bodies.iter().any(|b| b.required) && body.is_empty() {
            return Err("the request body is needed".to_owned());
        }
        Ok(BuiltRequest { method: operation.method.clone(), url, headers, body })
    }
}

/// The values to build a request from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RequestInput {
    pub path: BTreeMap<String, String>,
    pub query: BTreeMap<String, String>,
    pub headers: BTreeMap<String, String>,
    pub content_type: Option<String>,
    pub body: Option<String>,
}

/// A request, ready to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Array(items) => items.iter().map(scalar_text).collect::<Vec<_>>().join(","),
        other => other.to_string(),
    }
}

/// Percent-encode `text` (a query component also encodes what separates query parts; a path segment keeps `/` encoded too).
fn percent_encode(text: &str, query: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        let keep = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') || (query && matches!(byte, b'*' | b','));
        if keep {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn servers_of(root: &Value, version: Version) -> Vec<Server> {
    if version == Version::Swagger2 {
        let host = root.get("host").and_then(Value::as_str).unwrap_or_default();
        if host.is_empty() {
            return Vec::new();
        }
        let base = root.get("basePath").and_then(Value::as_str).unwrap_or_default();
        let schemes: Vec<&str> =
            root.get("schemes").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let scheme = if schemes.is_empty() || schemes.contains(&"https") { "https" } else { schemes.first().copied().unwrap_or("https") };
        return vec![Server { url: format!("{scheme}://{host}{base}"), description: None, variables: BTreeMap::new() }];
    }
    root.get("servers")
        .and_then(Value::as_array)
        .map(|servers| {
            servers
                .iter()
                .filter_map(|s| {
                    let url = s.get("url").and_then(Value::as_str)?.to_owned();
                    let variables = s
                        .get("variables")
                        .and_then(Value::as_object)
                        .map(|vars| {
                            vars.iter().filter_map(|(k, v)| Some((k.clone(), v.get("default").and_then(Value::as_str)?.to_owned()))).collect()
                        })
                        .unwrap_or_default();
                    Some(Server { url, description: text_of(s.get("description")), variables })
                })
                .collect()
        })
        .unwrap_or_default()
}

const METHODS: &[&str] = &["get", "put", "post", "delete", "options", "head", "patch", "trace", "query"];

fn read_param(root: &Value, value: &Value, warnings: &mut Vec<String>, swagger2: bool) -> Option<ParamOrBody> {
    let resolved = match follow(root, value, 0) {
        Some(v) => v,
        None => {
            warnings.push(format!("a parameter reference cannot be followed: {}", value.get("$ref").and_then(Value::as_str).unwrap_or("?")));
            return None;
        }
    };
    let location = resolved.get("in").and_then(Value::as_str)?;
    let name = resolved.get("name").and_then(Value::as_str)?.to_owned();
    let required = resolved.get("required").and_then(Value::as_bool).unwrap_or(location == "path");
    if swagger2 && location == "body" {
        return Some(ParamOrBody::Body(Body {
            content_type: String::new(),
            schema: resolved.get("schema").cloned(),
            example: resolved.get("x-example").cloned(),
            required,
        }));
    }
    let location = In::parse(location)?;
    // Swagger 2 has the type on the parameter itself.
    let schema = resolved.get("schema").cloned().or_else(|| {
        swagger2.then(|| {
            let mut m = Map::new();
            for key in ["type", "format", "enum", "items", "default", "minimum", "maximum"] {
                if let Some(v) = resolved.get(key) {
                    m.insert(key.to_owned(), v.clone());
                }
            }
            Value::Object(m)
        })
    });
    let example = resolved.get("example").cloned().or_else(|| resolved.get("x-example").cloned());
    Some(ParamOrBody::Param(Param { name, location, required, description: text_of(resolved.get("description")), schema, example }))
}

enum ParamOrBody {
    Param(Param),
    Body(Body),
}

fn read_operations(root: &Value, version: Version, warnings: &mut Vec<String>) -> Vec<Operation> {
    let swagger2 = version == Version::Swagger2;
    let mut out = Vec::new();
    let Some(paths) = root.get("paths").and_then(Value::as_object) else { return out };
    let global_consumes: Vec<String> =
        root.get("consumes").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default();
    for (path, item) in paths {
        let Some(item) = follow(root, item, 0) else { continue };
        let shared: Vec<&Value> = item.get("parameters").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
        for method in METHODS {
            let Some(op) = item.get(*method).filter(|o| o.is_object()) else { continue };
            if out.len() >= MAX_OPERATIONS {
                warnings.push("the description has more operations than are read".to_owned());
                return out;
            }
            let mut parameters: Vec<Param> = Vec::new();
            let mut bodies: Vec<Body> = Vec::new();
            let own: Vec<&Value> = op.get("parameters").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
            // (An operation's own parameter replaces a shared one of the same name and place.)
            for raw in shared.iter().chain(own.iter()) {
                match read_param(root, raw, warnings, swagger2) {
                    Some(ParamOrBody::Param(p)) => {
                        parameters.retain(|q| !(q.name == p.name && q.location == p.location));
                        parameters.push(p);
                    }
                    Some(ParamOrBody::Body(b)) => {
                        let consumes: Vec<String> = op
                            .get("consumes")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                            .filter(|v: &Vec<String>| !v.is_empty())
                            .unwrap_or_else(
                                || if global_consumes.is_empty() { vec!["application/json".to_owned()] } else { global_consumes.clone() },
                            );
                        for content_type in consumes {
                            bodies.push(Body { content_type, ..b.clone() });
                        }
                    }
                    None => {}
                }
            }
            if !swagger2 && let Some(request_body) = op.get("requestBody").and_then(|b| follow(root, b, 0)) {
                let required = request_body.get("required").and_then(Value::as_bool).unwrap_or(false);
                if let Some(content) = request_body.get("content").and_then(Value::as_object) {
                    for (content_type, media) in content {
                        let example = media.get("example").cloned().or_else(|| {
                            media
                                .get("examples")
                                .and_then(Value::as_object)
                                .and_then(|e| e.values().next())
                                .and_then(|e| follow(root, e, 0))
                                .and_then(|e| e.get("value"))
                                .cloned()
                        });
                        bodies.push(Body { content_type: content_type.clone(), schema: media.get("schema").cloned(), example, required });
                    }
                }
            }
            let id = op.get("operationId").and_then(Value::as_str).map_or_else(|| format!("{method} {path}"), str::to_owned);
            out.push(Operation {
                id,
                method: method.to_ascii_uppercase(),
                path: path.clone(),
                summary: text_of(op.get("summary")),
                tags: op
                    .get("tags")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                    .unwrap_or_default(),
                deprecated: op.get("deprecated").and_then(Value::as_bool).unwrap_or(false),
                parameters,
                bodies,
            });
        }
    }
    out
}

/// An example value for `schema`, following `$ref`s that point into `root`.
pub fn example_of(root: &Value, schema: &Value, depth: usize) -> Value {
    if depth > MAX_DEPTH {
        return Value::Null;
    }
    let Some(schema) = follow(root, schema, depth) else { return Value::Null };
    for key in ["example", "default", "const"] {
        if let Some(v) = schema.get(key) {
            return v.clone();
        }
    }
    if let Some(first) = schema.get("examples").and_then(Value::as_array).and_then(|a| a.first()) {
        return first.clone();
    }
    if let Some(first) = schema.get("enum").and_then(Value::as_array).and_then(|a| a.first()) {
        return first.clone();
    }
    if let Some(parts) = schema.get("allOf").and_then(Value::as_array) {
        let mut merged = Map::new();
        for part in parts {
            if let Value::Object(m) = example_of(root, part, depth + 1) {
                merged.extend(m);
            }
        }
        if !merged.is_empty() {
            return Value::Object(merged);
        }
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(first) = schema.get(key).and_then(Value::as_array).and_then(|a| a.first()) {
            return example_of(root, first, depth + 1);
        }
    }
    // 3.1 allows a list of types: the first that is not null.
    let kind = match schema.get("type") {
        Some(Value::String(t)) => Some(t.as_str()),
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).find(|t| *t != "null"),
        _ => None,
    };
    let kind = kind.or_else(|| schema.get("properties").map(|_| "object")).or_else(|| schema.get("items").map(|_| "array"));
    match kind {
        Some("string") => Value::String(string_example(schema)),
        Some("integer") => Value::from(schema.get("minimum").and_then(Value::as_i64).unwrap_or(0)),
        Some("number") => Value::from(schema.get("minimum").and_then(Value::as_f64).unwrap_or(0.0)),
        Some("boolean") => Value::Bool(true),
        Some("array") => match schema.get("items") {
            Some(items) => Value::Array(vec![example_of(root, items, depth + 1)]),
            None => Value::Array(Vec::new()),
        },
        Some("object") => {
            let mut object = Map::new();
            if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                for (name, property) in properties {
                    object.insert(name.clone(), example_of(root, property, depth + 1));
                }
            }
            Value::Object(object)
        }
        _ => Value::Null,
    }
}

fn string_example(schema: &Value) -> String {
    match schema.get("format").and_then(Value::as_str) {
        Some("date") => "2026-01-01".to_owned(),
        Some("date-time") => "2026-01-01T00:00:00Z".to_owned(),
        Some("email") => "user@example.com".to_owned(),
        Some("uuid") => "00000000-0000-0000-0000-000000000000".to_owned(),
        Some("uri" | "url") => "https://example.com/".to_owned(),
        Some("byte") => "AAAA".to_owned(),
        _ => "string".to_owned(),
    }
}
