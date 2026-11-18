//! `openapi_describe`, `openapi_request` and `openapi_call`, through the tool table.

use std::io::{Read, Write};
use std::net::TcpListener;

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, Network, ToolError};

fn call(a: &mut Automation, tool: &str, args: Value) -> Result<Value, ToolError> {
    a.call(tool, &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

const API: &str = r##"openapi: 3.0.0
info: {title: Pets, version: '1'}
servers: [{url: 'http://example.test/v1'}]
paths:
  /pets/{id}:
    get:
      operationId: getPet
      parameters:
        - {name: id, in: path, required: true, schema: {type: integer, minimum: 3}}
        - {name: verbose, in: query, schema: {type: boolean}}
  /pets:
    post:
      operationId: addPet
      requestBody:
        required: true
        content:
          application/json:
            schema: {type: object, properties: {name: {type: string}}}
"##;

#[test]
fn the_operations_of_a_yaml_description_are_listed_and_a_request_is_built_with_examples_and_overrides() {
    let mut a = Automation::new();
    let r = call(&mut a, "openapi_describe", json!({ "text": API })).expect("runs");
    assert_eq!(
        (r["version"].as_str(), r["operations"].as_array().map(Vec::len), r["servers"][0]["resolved"].as_str()),
        (Some("3.0"), Some(2), Some("http://example.test/v1")),
        "{r}"
    );
    let get = r["operations"].as_array().and_then(|o| o.iter().find(|o| o["id"] == "getPet")).expect("getPet");
    assert_eq!(get["parameters"][0], json!({ "name": "id", "in": "path", "required": true, "description": null }));
    let r = call(&mut a, "openapi_request", json!({ "text": API, "operation": "getPet" })).expect("runs");
    assert_eq!(r["request"]["url"].as_str(), Some("http://example.test/v1/pets/3"), "the example of a schema with a minimum: {r}");
    let r = call(&mut a, "openapi_request", json!({ "text": API, "operation": "getPet", "path_parameters": { "id": "42" }, "query": { "verbose": "true" }, "server": "http://other.test" })).expect("runs");
    assert_eq!(r["request"]["url"].as_str(), Some("http://other.test/pets/42?verbose=true"));
    let r = call(&mut a, "openapi_request", json!({ "text": API, "operation": "addPet" })).expect("runs");
    assert_eq!((r["request"]["method"].as_str(), r["request"]["body"].as_str().map(|b| b.contains("\"name\""))), (Some("POST"), Some(true)), "{r}");
    assert_eq!(r["request"]["headers"][0]["value"].as_str(), Some("application/json"));
    let e = call(&mut a, "openapi_request", json!({ "text": API, "operation": "nope" })).expect_err("unknown");
    assert!(e.to_string().contains("no operation nope") && e.to_string().contains("getPet"), "{e}");
    assert!(matches!(call(&mut a, "openapi_request", json!({ "text": API, "operation": "getPet", "query": "x" })), Err(ToolError::InvalidArgs(_))));
    assert!(call(&mut a, "openapi_describe", json!({ "text": "{\"a\": 1}" })).is_err());
}

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
    (format!("http://{address}"), handle)
}

#[test]
fn a_call_is_sent_to_a_server_and_the_json_answer_is_read_and_nothing_is_sent_offline_or_to_this_computer_unless_allowed() {
    let body = "{\"id\": 3, \"name\": \"Rex\"}";
    let (server, handle) =
        serve(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()));
    let mut online = Automation::new().with_network(Network::standard());
    let r = call(&mut online, "openapi_call", json!({ "text": API, "operation": "getPet", "server": server, "allow_private_hosts": true }))
        .expect("runs");
    assert_eq!((r["response"]["status"].as_u64(), r["response"]["json"]["name"].as_str()), (Some(200), Some("Rex")), "{r}");
    let received = handle.join().expect("server");
    assert!(received.starts_with("GET /pets/3 HTTP/1.1"), "{received}");
    let e = call(&mut online, "openapi_call", json!({ "text": API, "operation": "getPet", "server": "http://127.0.0.1:1" })).expect_err("private");
    assert!(e.to_string().contains("allow private hosts"), "{e}");
    let mut offline = Automation::new();
    let e = call(&mut offline, "openapi_call", json!({ "text": API, "operation": "getPet" })).expect_err("offline");
    assert!(e.to_string().contains("offline"), "{e}");
    assert!(matches!(
        call(&mut online, "openapi_call", json!({ "text": API, "operation": "getPet", "timeout_seconds": 0 })),
        Err(ToolError::InvalidArgs(_))
    ));
}

const SECURED: &str = r##"{"openapi": "3.0.0", "info": {"title": "S", "version": "1"}, "servers": [{"url": "http://example.test"}],
  "security": [{"k": []}],
  "components": {"securitySchemes": {"b": {"type": "http", "scheme": "basic"}, "k": {"type": "apiKey", "in": "header", "name": "X-Key"}, "o": {"type": "oauth2", "flows": {}}}},
  "paths": {"/p": {"get": {"operationId": "p"}}}}"##;

#[test]
fn the_security_schemes_of_a_description_are_listed_and_auth_names_what_it_adds_but_never_its_secrets() {
    let mut a = Automation::new();
    let r = call(&mut a, "openapi_describe", json!({ "text": SECURED })).expect("runs");
    assert_eq!(r["default_security_scheme"], json!("k"));
    assert_eq!(r["security_schemes"][0], json!({ "name": "b", "supported": true, "type": "basic" }));
    assert_eq!(r["security_schemes"][1], json!({ "name": "k", "supported": true, "type": "api_key", "in": "header", "key_name": "X-Key" }));
    assert_eq!(r["security_schemes"][2]["supported"], json!(false));
    let r = call(
        &mut a,
        "openapi_request",
        json!({ "text": SECURED, "operation": "p", "auth": { "type": "basic", "user": "alice", "password": "hunter2-pw" } }),
    )
    .expect("runs");
    assert_eq!(r["auth"]["adds_headers"], json!(["Authorization"]), "{r}");
    assert!(!r.to_string().contains("hunter2-pw") && !r.to_string().contains("YWxpY2U6aHVudGVyMi1wdw"), "no secret, nor its encoding: {r}");
    let q = call(
        &mut a,
        "openapi_request",
        json!({ "text": SECURED, "operation": "p", "auth": { "type": "api_key", "name": "key", "value": "s3cr3t-k", "in": "query" } }),
    )
    .expect("runs");
    assert_eq!((q["auth"]["adds_query"].clone(), q["request"]["url"].as_str()), (json!(true), Some("http://example.test/p")), "{q}");
    assert!(!q.to_string().contains("s3cr3t-k"), "{q}");
    for bad in [
        json!({ "type": "digest" }),
        json!({ "type": "basic", "user": "a:b" }),
        json!({ "type": "bearer", "token": "" }),
        json!({ "type": "bearer", "extra": 1 }),
        json!("basic"),
    ] {
        assert!(call(&mut a, "openapi_request", json!({ "text": SECURED, "operation": "p", "auth": bad })).is_err(), "{bad}");
    }
}

#[test]
fn a_call_sends_the_authorization_and_never_gives_a_secret_back_not_even_when_the_server_echoes_it_or_the_address_fails() {
    // The server echoes the request line and the Authorization header in its answer.
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let address = format!("http://{}", listener.local_addr().expect("address"));
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accepts");
        let mut received = Vec::new();
        let mut buffer = [0u8; 4096];
        while !String::from_utf8_lossy(&received).contains("\r\n\r\n") {
            let n = stream.read(&mut buffer).expect("reads");
            if n == 0 {
                break;
            }
            received.extend_from_slice(buffer.get(..n).unwrap_or_default());
        }
        let text = String::from_utf8_lossy(&received).into_owned();
        let echo =
            text.lines().filter(|l| l.starts_with("GET") || l.to_ascii_lowercase().starts_with("authorization")).collect::<Vec<_>>().join(" | ");
        let body = format!("{{\"echo\": \"{echo}\"}}");
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Echo: {echo}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .expect("writes");
        text
    });
    let mut online = Automation::new().with_network(Network::standard());
    let r = call(
        &mut online,
        "openapi_call",
        json!({ "text": SECURED, "operation": "p", "server": address, "allow_private_hosts": true, "auth": { "type": "bearer", "token": "tok-9f8e7d" } }),
    )
    .expect("runs");
    let sent = handle.join().expect("server");
    assert!(sent.contains("Authorization: Bearer tok-9f8e7d") || sent.contains("authorization: Bearer tok-9f8e7d"), "the server got it: {sent}");
    assert!(!r.to_string().contains("tok-9f8e7d"), "the secret is not returned, though the server echoed it: {r}");
    assert!(r.to_string().contains("***"), "{r}");
    assert_eq!(r["auth"]["type"], json!("bearer"));
    // A failing address with the key in the query does not show the key.
    let e = call(
        &mut online,
        "openapi_call",
        json!({ "text": SECURED, "operation": "p", "server": "http://127.0.0.1:1", "allow_private_hosts": true, "timeout_seconds": 5, "auth": { "type": "api_key", "name": "key", "value": "q-secret-77", "in": "query" } }),
    )
    .expect_err("nobody listens");
    assert!(!e.to_string().contains("q-secret-77"), "{e}");
}
