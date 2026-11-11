//! `json_sample`: a sample JSON, YAML or BSON value made from a JSON Schema.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, tool: &str, args: Value) -> Result<Value, ToolError> {
    a.call(tool, &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

fn sample(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    call(a, "json_sample", args)
}

const SCHEMA: &str = r#"{"type":"object","required":["name"],"properties":{"name":{"type":"string"},"age":{"type":"integer"},"color":{"type":"string","enum":["red","blue"]}}}"#;

#[test]
fn a_schema_gives_a_sample_in_each_output_format() {
    let mut a = Automation::new();
    let result = sample(&mut a, json!({ "text": SCHEMA })).expect("runs");
    assert_eq!(result["output"], "json");
    let value: Value = serde_json::from_str(result["text"].as_str().expect("text")).expect("JSON");
    assert_eq!(value, json!({ "name": "string", "age": 0, "color": "red" }));

    let yaml = sample(&mut a, json!({ "text": SCHEMA, "output": "yaml" })).expect("runs");
    assert!(yaml["text"].as_str().is_some_and(|t| t.contains("name: string")), "{yaml}");
    let bson = sample(&mut a, json!({ "text": SCHEMA, "output": "bson" })).expect("runs");
    assert!(bson["text"].as_str().is_some_and(|t| t.contains("\"name\"")), "{bson}");
}

#[test]
fn the_required_names_the_optional_ones_and_the_depth_are_the_options() {
    let mut a = Automation::new();
    let result = sample(&mut a, json!({ "text": SCHEMA, "optional": false })).expect("runs");
    let value: Value = serde_json::from_str(result["text"].as_str().expect("text")).expect("JSON");
    assert_eq!(value, json!({ "name": "string" }));
    let result = sample(&mut a, json!({ "text": SCHEMA, "max_depth": 0 })).expect("runs");
    let value: Value = serde_json::from_str(result["text"].as_str().expect("text")).expect("JSON");
    assert_eq!(value, json!({}));
}

#[test]
fn a_broken_schema_and_the_arguments_are_refused() {
    let mut a = Automation::new();
    let err = sample(&mut a, json!({ "text": "{" })).expect_err("not a schema");
    assert!(matches!(&err, ToolError::Failed(m) if m.contains("not valid JSON")), "{err:?}");
    for args in [
        json!({ "text": SCHEMA, "output": "xml" }),
        json!({ "text": SCHEMA, "max_depth": 99 }),
        json!({ "text": SCHEMA, "optional": "yes" }),
        json!({}),
        json!({ "text": SCHEMA, "path": "schema.json" }),
    ] {
        let err = sample(&mut a, args.clone()).expect_err(&args.to_string());
        assert!(matches!(err, ToolError::InvalidArgs(_) | ToolError::Failed(_)), "{args}: {err:?}");
    }
}

#[test]
fn a_schema_that_refers_to_itself_through_many_names_ends_with_an_error_or_a_bounded_sample() {
    let names: Vec<String> = ('a'..='l').map(|n| format!("\"{n}\":{{\"$ref\":\"#\"}}")).collect();
    let schema = format!("{{\"properties\":{{{}}}}}", names.join(","));
    let mut a = Automation::new();
    let started = std::time::Instant::now();
    let err = sample(&mut a, json!({ "text": schema, "max_depth": 32 })).expect_err("too large");
    assert!(matches!(&err, ToolError::Failed(m) if m.contains("too large")), "{err:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    let small = sample(&mut a, json!({ "text": schema, "max_depth": 1 })).expect("a bounded sample");
    let value: Value = serde_json::from_str(small["text"].as_str().expect("text")).expect("JSON");
    assert_eq!(value.as_object().map(serde_json::Map::len), Some(12));
}
