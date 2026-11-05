//! OpenAPI descriptions read, and requests built from them.

use std::collections::BTreeMap;

use serde_json::json;
use xmlcraft_openapi::{In, RequestInput, Version, parse};

const PETS: &str = r##"{
  "openapi": "3.0.3",
  "info": {"title": "Pets", "version": "1"},
  "servers": [{"url": "https://{env}.example.com/v1", "variables": {"env": {"default": "api"}}}],
  "paths": {
    "/pets": {
      "get": {"operationId": "listPets", "summary": "List", "parameters": [
        {"name": "limit", "in": "query", "schema": {"type": "integer", "minimum": 5}},
        {"name": "X-Trace", "in": "header", "required": true, "schema": {"type": "string"}}]},
      "post": {"operationId": "addPet", "requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Pet"}}}}}
    },
    "/pets/{petId}": {
      "parameters": [{"name": "petId", "in": "path", "required": true, "schema": {"type": "string"}}],
      "get": {"operationId": "getPet", "deprecated": true, "tags": ["pets"]},
      "delete": {"parameters": [{"$ref": "#/components/parameters/Reason"}]}
    }
  },
  "components": {
    "parameters": {"Reason": {"name": "reason", "in": "query", "schema": {"type": "string", "enum": ["sold", "died"]}}},
    "schemas": {
      "Pet": {"type": "object", "required": ["name"], "properties": {"name": {"type": "string"}, "age": {"type": "integer", "minimum": 1}, "tag": {"type": "string", "format": "email"}, "owner": {"$ref": "#/components/schemas/Owner"}}},
      "Owner": {"type": "object", "properties": {"pets": {"type": "array", "items": {"$ref": "#/components/schemas/Pet"}}, "kind": {"enum": ["person", "shop"]}}}
    }
  }
}"##;

#[test]
fn operations_servers_and_parameters_are_read_with_refs_followed() {
    let api = parse(PETS, false).expect("reads");
    assert_eq!((api.version, api.title.as_deref(), api.operations.len()), (Version::OpenApi30, Some("Pets"), 4));
    assert_eq!(api.servers[0].resolved(), "https://api.example.com/v1");
    let list = api.operation("listPets").expect("an operation");
    assert_eq!((list.method.as_str(), list.path.as_str(), list.summary.as_deref()), ("GET", "/pets", Some("List")));
    assert_eq!(
        list.parameters.iter().map(|p| (p.name.as_str(), p.location, p.required)).collect::<Vec<_>>(),
        [("limit", In::Query, false), ("X-Trace", In::Header, true)]
    );
    let get = api.operation("getPet").expect("an operation");
    assert!(get.deprecated && get.tags == ["pets"]);
    assert_eq!(get.parameters[0].name, "petId", "a parameter of the path is every operation's");
    let delete = api.operation("delete /pets/{petId}").expect("named by method and path");
    assert_eq!(delete.parameters.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["petId", "reason"], "{:?}", delete.parameters);
    assert!(api.operation("DELETE /PETS/{PETID}").is_some(), "in any case");
    assert!(api.warnings.is_empty(), "{:?}", api.warnings);
}

#[test]
fn examples_come_from_schemas_with_refs_enums_formats_and_recursion_cut() {
    let api = parse(PETS, false).expect("reads");
    let add = api.operation("addPet").expect("an operation");
    let input = api.sample_input(add);
    assert_eq!(input.content_type.as_deref(), Some("application/json"));
    let body: serde_json::Value = serde_json::from_str(input.body.as_deref().expect("a body")).expect("json");
    assert_eq!(body["name"], json!("string"));
    assert_eq!(body["age"], json!(1));
    assert_eq!(body["tag"], json!("user@example.com"));
    assert_eq!(body["owner"]["kind"], json!("person"), "the first of an enum");
    assert!(body["owner"]["pets"].is_array(), "{body}");
    // An example, a default, a const and allOf are used.
    let root = json!({});
    assert_eq!(xmlcraft_openapi::example_of(&root, &json!({"type": "string", "example": "x"}), 0), json!("x"));
    assert_eq!(xmlcraft_openapi::example_of(&root, &json!({"type": "integer", "default": 7}), 0), json!(7));
    assert_eq!(
        xmlcraft_openapi::example_of(
            &root,
            &json!({"allOf": [{"type": "object", "properties": {"a": {"type": "boolean"}}}, {"type": "object", "properties": {"b": {"type": ["null", "number"]}}}]}),
            0
        ),
        json!({"a": true, "b": 0.0})
    );
    assert_eq!(
        xmlcraft_openapi::example_of(&root, &json!({"oneOf": [{"type": "string", "format": "date"}, {"type": "integer"}]}), 0),
        json!("2026-01-01")
    );
    // A schema that refers to itself ends.
    let recursive = json!({"components": {"schemas": {"Node": {"type": "object", "properties": {"next": {"$ref": "#/components/schemas/Node"}}}}}});
    let example = xmlcraft_openapi::example_of(&recursive, &json!({"$ref": "#/components/schemas/Node"}), 0);
    assert!(example.is_object());
    assert!(xmlcraft_openapi::example_of(&recursive, &json!({"$ref": "#/nowhere"}), 0).is_null());
}

#[test]
fn a_request_has_its_path_filled_in_and_encoded_its_query_headers_and_body() {
    let api = parse(PETS, false).expect("reads");
    let get = api.operation("getPet").expect("an operation");
    let mut input = RequestInput::default();
    input.path.insert("petId".into(), "a b/c?".into());
    let r = api.build_request(get, &input, None).expect("builds");
    assert_eq!((r.method.as_str(), r.url.as_str()), ("GET", "https://api.example.com/v1/pets/a%20b%2Fc%3F"));
    let list = api.operation("listPets").expect("an operation");
    let mut input = RequestInput::default();
    input.headers.insert("X-Trace".into(), "t1".into());
    input.query.insert("limit".into(), "10".into());
    input.query.insert("extra key".into(), "a&b".into());
    let r = api.build_request(list, &input, Some("http://localhost:8080/")).expect("builds");
    assert_eq!(r.url, "http://localhost:8080/pets?limit=10&extra%20key=a%26b");
    assert_eq!(r.headers, [("X-Trace".to_owned(), "t1".to_owned())]);
    let add = api.operation("addPet").expect("an operation");
    let sample = api.sample_input(add);
    let r = api.build_request(add, &sample, None).expect("builds");
    assert_eq!(r.headers, [("Content-Type".to_owned(), "application/json".to_owned())]);
    assert!(String::from_utf8(r.body).expect("utf-8").contains("\"name\""));
    // What is required and missing is said.
    for (op, why) in [("getPet", "petId"), ("listPets", "X-Trace"), ("addPet", "body")] {
        let e = api.build_request(api.operation(op).expect("an operation"), &RequestInput::default(), None).expect_err(op);
        assert!(e.contains(why), "{op}: {e}");
    }
    // The sample input of a path parameter is a value, so the sample request builds.
    let sample = api.sample_input(get);
    assert_eq!(sample.path, BTreeMap::from([("petId".to_owned(), "string".to_owned())]));
    assert!(api.build_request(get, &sample, None).is_ok());
}

#[test]
fn a_yaml_description_and_swagger_2_are_read_too() {
    let yaml = "openapi: 3.1.0\ninfo: {title: Y, version: '1'}\nservers: [{url: 'http://h/api'}]\npaths:\n  /a:\n    get:\n      operationId: a\n      parameters:\n        - {name: q, in: query, required: true, schema: {type: [string, 'null']}}\n";
    let api = parse(yaml, true).expect("reads");
    assert_eq!((api.version, api.operations.len()), (Version::OpenApi31, 1));
    let r = api.build_request(&api.operations[0], &api.sample_input(&api.operations[0]), None).expect("builds");
    assert_eq!(r.url, "http://h/api/a?q=string");
    let swagger = r##"{"swagger": "2.0", "host": "h.example.com", "basePath": "/v2", "schemes": ["http", "https"], "consumes": ["application/json"],
      "paths": {"/p/{id}": {"put": {"operationId": "put", "parameters": [
        {"name": "id", "in": "path", "required": true, "type": "integer"},
        {"name": "limit", "in": "query", "type": "integer", "default": 3},
        {"name": "body", "in": "body", "required": true, "schema": {"$ref": "#/definitions/P"}}]}}},
      "definitions": {"P": {"type": "object", "properties": {"n": {"type": "string"}}}}}"##;
    let api = parse(swagger, false).expect("reads");
    assert_eq!(api.version, Version::Swagger2);
    assert_eq!(api.servers[0].resolved(), "https://h.example.com/v2");
    let op = &api.operations[0];
    assert_eq!((op.parameters.len(), op.bodies.len(), op.bodies[0].content_type.as_str()), (2, 1, "application/json"));
    let sample = api.sample_input(op);
    assert_eq!(sample.path.get("id").map(String::as_str), Some("0"));
    let r = api.build_request(op, &sample, None).expect("builds");
    assert!(r.url.starts_with("https://h.example.com/v2/p/0"), "{}", r.url);
    assert!(String::from_utf8(r.body).expect("utf-8").contains("\"n\""));
}

#[test]
fn what_is_not_a_description_is_refused_and_a_ref_that_goes_nowhere_is_a_warning() {
    for (text, why) in [
        ("{}", "no `openapi` or `swagger`"),
        ("{\"openapi\": \"4.0\"}", "not known"),
        ("{\"swagger\": \"1.2\"}", "not known"),
        ("not json at all {", "cannot be read"),
        ("", "cannot be read"),
    ] {
        let e = parse(text, false).expect_err(text);
        assert!(e.0.contains(why), "{text:?}: {e}");
    }
    let broken = r##"{"openapi": "3.0.0", "paths": {"/a": {"get": {"parameters": [{"$ref": "#/components/parameters/Nope"}]}}}}"##;
    let api = parse(broken, false).expect("reads");
    assert_eq!(api.warnings.len(), 1, "{:?}", api.warnings);
    assert!(api.warnings[0].contains("Nope"));
    let e = api.build_request(&api.operations[0], &RequestInput::default(), None).expect_err("no server");
    assert!(e.contains("no server"), "{e}");
}

proptest::proptest! {
    #[test]
    fn any_text_is_a_description_or_an_error_and_a_request_of_it_is_built_or_refused(text in ".{0,300}") {
        for yaml in [false, true] {
            if let Ok(api) = parse(&text, yaml) {
                for op in &api.operations {
                    let _ = api.build_request(op, &api.sample_input(op), Some("http://x"));
                }
            }
        }
    }
}
