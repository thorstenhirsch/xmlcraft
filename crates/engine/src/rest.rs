//! What the OpenAPI window works from: the operations and servers of an OpenAPI description (2.0, 3.0, 3.1 or 3.2, in JSON or YAML)
//! and the HTTP request made for one, with examples for what is needed. Sending is the window's own (it needs the network).

use xmlcraft_openapi::{Operation, parse};

use crate::soap::MAX_OPERATIONS;

/// One operation as the window lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestOperation {
    /// The `operationId`, or `method path`: what the request is made by.
    pub id: String,
    pub method: String,
    pub path: String,
    pub summary: Option<String>,
    pub deprecated: bool,
}

impl RestOperation {
    /// The text the list shows: `GET /pets/{id}`, with the summary after it.
    pub fn label(&self) -> String {
        let mut label = format!("{} {}", self.method, self.path);
        if let Some(summary) = self.summary.as_deref().filter(|s| !s.trim().is_empty()) {
            let short: String = summary.chars().take(60).collect();
            label.push_str(" — ");
            label.push_str(&short);
        }
        if self.deprecated {
            label.push_str(" (deprecated)");
        }
        label
    }
}

fn entry(o: &Operation) -> RestOperation {
    RestOperation { id: o.id.clone(), method: o.method.clone(), path: o.path.clone(), summary: o.summary.clone(), deprecated: o.deprecated }
}

/// The operations, the servers (each with its variables resolved) and what was left out of the OpenAPI description `text` (`yaml`: it
/// is YAML).
pub fn describe(text: &str, yaml: bool) -> Result<(Vec<RestOperation>, Vec<String>, Vec<String>), String> {
    let api = parse(text, yaml).map_err(|e| e.0)?;
    let operations = api.operations.iter().take(MAX_OPERATIONS).map(entry).collect();
    let servers = api.servers.iter().map(|s| s.resolved()).collect();
    Ok((operations, servers, api.warnings))
}

/// The request made for an operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Make the request of `operation` with example values, to the server `server` (the first the description names when none is given).
pub fn prepare(text: &str, yaml: bool, operation: &str, server: Option<&str>) -> Result<RestRequest, String> {
    let api = parse(text, yaml).map_err(|e| e.0)?;
    let op = api.operation(operation).ok_or_else(|| format!("the description has no operation {operation}"))?;
    let input = api.sample_input(op);
    let built = api.build_request(op, &input, server)?;
    Ok(RestRequest { method: built.method, url: built.url, headers: built.headers, body: String::from_utf8_lossy(&built.body).into_owned() })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    const API: &str = r#"{
      "openapi": "3.0.3", "info": {"title": "Pets", "version": "1"},
      "servers": [{"url": "https://{host}/v1", "variables": {"host": {"default": "api.example.com"}}}, {"url": "http://localhost:8080"}],
      "paths": {
        "/pets/{id}": {"get": {"operationId": "getPet", "summary": "One pet", "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "integer"}}]}},
        "/pets": {"post": {"summary": "Add", "deprecated": true, "requestBody": {"required": true, "content": {"application/json": {"schema": {"type": "object", "properties": {"name": {"type": "string"}}}}}}}}
      }
    }"#;

    #[test]
    fn operations_and_servers_are_listed_with_their_variables_resolved() {
        let (ops, servers, warnings) = describe(API, false).expect("reads");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(servers, ["https://api.example.com/v1", "http://localhost:8080"]);
        let labels: Vec<String> = ops.iter().map(RestOperation::label).collect();
        assert!(labels.contains(&"GET /pets/{id} — One pet".to_owned()), "{labels:?}");
        assert!(labels.contains(&"POST /pets — Add (deprecated)".to_owned()), "{labels:?}");
        assert!(describe("not an api", false).is_err());
    }

    #[test]
    fn a_request_has_the_path_filled_in_a_body_from_the_schema_and_the_chosen_server() {
        let get = prepare(API, false, "getPet", None).expect("builds");
        assert_eq!((get.method.as_str(), get.url.as_str()), ("GET", "https://api.example.com/v1/pets/0"));
        let local = prepare(API, false, "getPet", Some("http://localhost:8080")).expect("builds");
        assert_eq!(local.url, "http://localhost:8080/pets/0");
        let post = prepare(API, false, "post /pets", None).expect("builds");
        assert!(post.body.contains("\"name\""), "{}", post.body);
        assert!(post.headers.iter().any(|(n, v)| n == "Content-Type" && v == "application/json"), "{:?}", post.headers);
        assert!(prepare(API, false, "nothing", None).expect_err("no such operation").contains("no operation"));
    }
}
