//! What the OpenAPI window works from: the operations and servers of an OpenAPI description (2.0, 3.0, 3.1 or 3.2, in JSON or YAML)
//! and the HTTP request made for one, with examples for what is needed. Sending is the window's own (it needs the network).

use xmlcraft_json_syntax::{Data, Dialect, Options, Value};
use xmlcraft_openapi::{Operation, parse};

use crate::http_auth::KeyPlace;
use crate::soap::MAX_OPERATIONS;

/// The most security schemes that are listed.
const MAX_SCHEMES: usize = 50;

/// What a security scheme of a description asks for, and whether the HTTP window can fill it in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemeKind {
    Basic,
    Bearer,
    ApiKey {
        place: KeyPlace,
        name: String,
    },
    /// A scheme that is not supported yet, and what it is.
    Unsupported(String),
}

/// One security scheme of a description.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecurityScheme {
    pub name: String,
    pub kind: SchemeKind,
}

/// The security schemes of a description (`components.securitySchemes`, or `securityDefinitions` in 2.0) and the one its root `security`
/// asks for first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Security {
    pub schemes: Vec<SecurityScheme>,
    pub default: Option<String>,
}

impl Security {
    /// The scheme to start from: the one the root `security` names first, else the only one there is.
    pub fn preferred(&self) -> Option<&SecurityScheme> {
        let named = self.default.as_ref().and_then(|d| self.schemes.iter().find(|s| &s.name == d));
        named.or_else(|| if self.schemes.len() == 1 { self.schemes.first() } else { None })
    }
}

fn text_of<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    match &value.get(key)?.data {
        Data::String(s) => Some(s),
        _ => None,
    }
}

/// The security schemes of the description `text` (`yaml`: it is YAML); none when it has none or cannot be read.
pub fn security(text: &str, yaml: bool) -> Security {
    let root = if yaml {
        xmlcraft_yaml_syntax::parse(text, &xmlcraft_yaml_syntax::Options::default()).documents.into_iter().next()
    } else {
        xmlcraft_json_syntax::parse(text, &Options { dialect: Dialect::Json5, ..Options::default() }).root
    };
    let Some(root) = root else { return Security::default() };
    let table = root.get("components").and_then(|c| c.get("securitySchemes")).or_else(|| root.get("securityDefinitions"));
    let Some(Data::Object(members)) = table.map(|t| &t.data) else { return Security::default() };
    let mut schemes = Vec::new();
    for member in members.iter().take(MAX_SCHEMES) {
        let scheme = &member.value;
        let kind = match text_of(scheme, "type") {
            // (Swagger 2.0 calls HTTP Basic a type of its own.)
            Some("basic") => SchemeKind::Basic,
            Some("http") => match text_of(scheme, "scheme").map(str::to_ascii_lowercase).as_deref() {
                Some("basic") => SchemeKind::Basic,
                Some("bearer") => SchemeKind::Bearer,
                Some(other) => SchemeKind::Unsupported(format!("HTTP {other} authentication")),
                None => SchemeKind::Unsupported("HTTP authentication with no scheme".to_owned()),
            },
            Some("apiKey") => match (text_of(scheme, "in"), text_of(scheme, "name")) {
                (Some("header"), Some(name)) => SchemeKind::ApiKey { place: KeyPlace::Header, name: name.to_owned() },
                (Some("query"), Some(name)) => SchemeKind::ApiKey { place: KeyPlace::Query, name: name.to_owned() },
                (Some("cookie"), _) => SchemeKind::Unsupported("an API key in a cookie".to_owned()),
                _ => SchemeKind::Unsupported("an API key with no place or name".to_owned()),
            },
            Some("oauth2") => SchemeKind::Unsupported("OAuth 2.0".to_owned()),
            Some("openIdConnect") => SchemeKind::Unsupported("OpenID Connect".to_owned()),
            Some("mutualTLS") => SchemeKind::Unsupported("mutual TLS".to_owned()),
            Some(other) => SchemeKind::Unsupported(other.to_owned()),
            None => SchemeKind::Unsupported("a scheme with no type".to_owned()),
        };
        schemes.push(SecurityScheme { name: member.key.clone(), kind });
    }
    // The first alternative of the root `security` that the window can fill in; else the first that names a scheme of the description.
    let default = match root.get("security").map(|s| &s.data) {
        Some(Data::Array(requirements)) => {
            let names: Vec<&str> = requirements
                .iter()
                .filter_map(|r| match &r.data {
                    Data::Object(names) => Some(names.iter().map(|m| m.key.as_str())),
                    _ => None,
                })
                .flatten()
                .filter(|k| schemes.iter().any(|s| s.name == *k))
                .collect();
            let supported = |name: &&str| schemes.iter().any(|s| s.name == **name && !matches!(s.kind, SchemeKind::Unsupported(_)));
            names.iter().find(|n| supported(n)).or_else(|| names.first()).map(|n| (*n).to_owned())
        }
        _ => None,
    };
    Security { schemes, default }
}

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

/// An OpenAPI description as the window lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Described {
    pub operations: Vec<RestOperation>,
    /// Each server's address with its variables resolved.
    pub servers: Vec<String>,
    /// What was left out.
    pub warnings: Vec<String>,
    /// How the description says requests are authorized.
    pub security: Security,
}

/// The operations, the servers and what was left out of the OpenAPI description `text` (`yaml`: it is YAML).
pub fn describe(text: &str, yaml: bool) -> Result<Described, String> {
    let api = parse(text, yaml).map_err(|e| e.0)?;
    let operations = api.operations.iter().take(MAX_OPERATIONS).map(entry).collect();
    let servers = api.servers.iter().map(|s| s.resolved()).collect();
    Ok(Described { operations, servers, warnings: api.warnings, security: security(text, yaml) })
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
        let Described { operations: ops, servers, warnings, security } = describe(API, false).expect("reads");
        assert_eq!(security, Security::default());
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

    #[test]
    fn security_schemes_are_listed_with_what_the_window_can_fill_in_and_the_first_required_one_is_the_default() {
        let api = r#"{"openapi": "3.0.3", "info": {"title": "T", "version": "1"}, "paths": {},
          "security": [{"oauth": []}, {"key": []}],
          "components": {"securitySchemes": {
            "basic": {"type": "http", "scheme": "Basic"}, "bearer": {"type": "http", "scheme": "bearer", "bearerFormat": "JWT"},
            "key": {"type": "apiKey", "in": "header", "name": "X-Api-Key"}, "q": {"type": "apiKey", "in": "query", "name": "k"},
            "c": {"type": "apiKey", "in": "cookie", "name": "s"}, "oauth": {"type": "oauth2", "flows": {}},
            "digest": {"type": "http", "scheme": "digest"}, "oidc": {"type": "openIdConnect", "openIdConnectUrl": "https://x"}}}}"#;
        let found = security(api, false);
        let kinds: Vec<(&str, &SchemeKind)> = found.schemes.iter().map(|s| (s.name.as_str(), &s.kind)).collect();
        assert_eq!(kinds[0], ("basic", &SchemeKind::Basic));
        assert_eq!(kinds[1], ("bearer", &SchemeKind::Bearer));
        assert_eq!(kinds[2], ("key", &SchemeKind::ApiKey { place: KeyPlace::Header, name: "X-Api-Key".to_owned() }));
        assert_eq!(kinds[3], ("q", &SchemeKind::ApiKey { place: KeyPlace::Query, name: "k".to_owned() }));
        for (i, what) in [(4, "cookie"), (5, "OAuth 2.0"), (6, "digest"), (7, "OpenID Connect")] {
            assert!(matches!(kinds[i].1, SchemeKind::Unsupported(why) if why.contains(what)), "{:?}", kinds[i]);
        }
        assert_eq!(found.default.as_deref(), Some("key"), "the first alternative the window can fill in (OAuth 2.0 comes first, and cannot be)");
        assert_eq!(found.preferred().map(|s| s.name.as_str()), Some("key"));
        // Swagger 2.0 and YAML.
        let two = "swagger: '2.0'\ninfo: {title: T, version: '1'}\npaths: {}\nsecurityDefinitions:\n  b: {type: basic}\n  a: {type: apiKey, in: query, name: token}\n  o: {type: oauth2, flow: implicit}\n";
        let found = security(two, true);
        assert_eq!(found.schemes.len(), 3);
        assert_eq!(found.schemes[0].kind, SchemeKind::Basic);
        assert!(found.preferred().is_none(), "several, and none required first");
        let only = security(r#"{"components": {"securitySchemes": {"t": {"type": "http", "scheme": "bearer"}}}}"#, false);
        assert_eq!(only.preferred().map(|s| &s.kind), Some(&SchemeKind::Bearer), "the only one there is");
        assert_eq!(security("not json", false), Security::default());
        assert_eq!(security("{}", false), Security::default());
    }
}
