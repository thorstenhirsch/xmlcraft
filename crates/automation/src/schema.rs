//! A small JSON Schema checker for tool arguments.
//!
//! Tool inputs are described with JSON Schema so agents can discover them (`tools/list`). This checks arguments against exactly the
//! subset the tool table uses: `type`, `enum`, `properties`, `required`, `additionalProperties: false`, `items`, and the bounds
//! `minimum`, `maximum`, `minLength`, `maxLength`, `minItems`, `maxItems`. Validating documents against user schemas is a different
//! job (the `validate` and `json` crates).

use serde_json::Value;

/// The JSON Schema type names this checker understands.
const KNOWN_TYPES: &[&str] = &["null", "boolean", "string", "array", "object", "number", "integer"];

/// How deep nested schemas and values are followed; deeper input is refused rather than recursed into.
const MAX_DEPTH: usize = 32;

/// Check `value` against `schema`; the error names the path of the first problem and says what was expected.
pub fn validate(schema: &Value, value: &Value) -> Result<(), String> {
    check(schema, value, "arguments", 0)
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(n) if n.is_i64() || n.is_u64() => "an integer",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn is_type(v: &Value, ty: &str) -> bool {
    match ty {
        "null" => v.is_null(),
        "boolean" => v.is_boolean(),
        "string" => v.is_string(),
        "array" => v.is_array(),
        "object" => v.is_object(),
        "number" => v.is_number(),
        "integer" => v.is_i64() || v.is_u64() || v.as_f64().is_some_and(|f| f.is_finite() && f.fract() == 0.0 && f.abs() < 9.0e15),
        _ => false,
    }
}

fn article(ty: &str) -> String {
    match ty {
        "integer" | "array" | "object" => format!("an {ty}"),
        "null" => "null".to_owned(),
        other => format!("a {other}"),
    }
}

fn check(schema: &Value, value: &Value, path: &str, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("{path}: nested more than {MAX_DEPTH} levels deep"));
    }
    let Some(schema) = schema.as_object() else { return Ok(()) };
    if let Some(ty) = schema.get("type") {
        let mut allowed: Vec<&str> = match ty {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        // A type name this checker does not know constrains nothing.
        allowed.retain(|t| KNOWN_TYPES.contains(t));
        if !allowed.is_empty() && !allowed.iter().any(|t| is_type(value, t)) {
            let want: Vec<String> = allowed.iter().map(|t| article(t)).collect();
            return Err(format!("{path}: expected {}, got {}", want.join(" or "), type_name(value)));
        }
    }
    if let Some(Value::Array(options)) = schema.get("enum")
        && !options.contains(value)
    {
        let list: Vec<String> = options.iter().map(Value::to_string).collect();
        return Err(format!("{path}: expected one of {}, got {value}", list.join(", ")));
    }
    if let Some(n) = value.as_f64() {
        if let Some(min) = schema.get("minimum").and_then(Value::as_f64)
            && n < min
        {
            return Err(format!("{path}: must be at least {min}, got {n}"));
        }
        if let Some(max) = schema.get("maximum").and_then(Value::as_f64)
            && n > max
        {
            return Err(format!("{path}: must be at most {max}, got {n}"));
        }
    }
    if let Some(s) = value.as_str() {
        let len = s.chars().count();
        if let Some(min) = schema.get("minLength").and_then(Value::as_u64)
            && (len as u64) < min
        {
            return Err(format!("{path}: must have at least {min} characters"));
        }
        if let Some(max) = schema.get("maxLength").and_then(Value::as_u64)
            && (len as u64) > max
        {
            return Err(format!("{path}: must have at most {max} characters, has {len}"));
        }
    }
    if let Some(items) = value.as_array() {
        if let Some(min) = schema.get("minItems").and_then(Value::as_u64)
            && (items.len() as u64) < min
        {
            return Err(format!("{path}: must have at least {min} items"));
        }
        if let Some(max) = schema.get("maxItems").and_then(Value::as_u64)
            && (items.len() as u64) > max
        {
            return Err(format!("{path}: must have at most {max} items, has {}", items.len()));
        }
        if let Some(item_schema) = schema.get("items") {
            for (i, item) in items.iter().enumerate() {
                check(item_schema, item, &format!("{path}[{i}]"), depth + 1)?;
            }
        }
    }
    if let Some(object) = value.as_object() {
        let props = schema.get("properties").and_then(Value::as_object);
        if let Some(Value::Array(required)) = schema.get("required") {
            for name in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(name) {
                    return Err(format!("{path}: missing required {name:?}"));
                }
            }
        }
        if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
            let known: Vec<&str> = props.map(|p| p.keys().map(String::as_str).collect()).unwrap_or_default();
            if let Some(extra) = object.keys().find(|k| !known.contains(&k.as_str())) {
                let hint = if known.is_empty() { "takes no arguments".to_owned() } else { format!("allowed: {}", known.join(", ")) };
                return Err(format!("{path}: unknown argument {extra:?} ({hint})"));
            }
        }
        if let Some(props) = props {
            for (name, sub) in props {
                if let Some(v) = object.get(name) {
                    check(sub, v, &format!("{path}.{name}"), depth + 1)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "minLength": 1, "maxLength": 20},
                "dpi": {"type": "integer", "minimum": 1, "maximum": 600},
                "mode": {"type": "string", "enum": ["a", "b"]},
                "tags": {"type": "array", "items": {"type": "string"}, "maxItems": 2},
                "ratio": {"type": "number"},
                "any": {"type": ["string", "null"]}
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    #[test]
    fn valid_arguments_pass() {
        assert_eq!(validate(&schema(), &json!({"path": "a.xml", "dpi": 96, "mode": "a", "tags": ["x"], "ratio": 0.5, "any": null})), Ok(()));
        assert_eq!(validate(&schema(), &json!({"path": "a"})), Ok(()));
    }

    #[test]
    fn each_problem_names_its_path_and_what_was_expected() {
        let cases = [
            (json!({}), "missing required \"path\""),
            (json!({"path": 3}), "arguments.path: expected a string, got an integer"),
            (json!({"path": ""}), "must have at least 1 characters"),
            (json!({"path": "x".repeat(21)}), "must have at most 20 characters, has 21"),
            (json!({"path": "a", "dpi": 0}), "arguments.dpi: must be at least 1"),
            (json!({"path": "a", "dpi": 601}), "must be at most 600"),
            (json!({"path": "a", "dpi": 1.5}), "expected an integer, got a number"),
            (json!({"path": "a", "mode": "c"}), "expected one of \"a\", \"b\", got \"c\""),
            (json!({"path": "a", "tags": ["x", "y", "z"]}), "must have at most 2 items"),
            (json!({"path": "a", "tags": [1]}), "arguments.tags[0]: expected a string"),
            (json!({"path": "a", "any": 5}), "expected a string or null"),
            (json!({"path": "a", "extra": 1}), "unknown argument \"extra\" (allowed:"),
            (json!([1]), "expected an object, got an array"),
            (json!(null), "expected an object, got null"),
        ];
        for (args, needle) in cases {
            let err = validate(&schema(), &args).expect_err(&args.to_string());
            assert!(err.contains(needle), "{args}: {err}");
        }
    }

    #[test]
    fn an_integral_float_is_an_integer_and_a_huge_one_is_not() {
        let s = json!({"type": "integer"});
        assert_eq!(validate(&s, &json!(3.0)), Ok(()));
        assert!(validate(&s, &json!(3.5)).is_err());
        assert!(validate(&s, &json!(1.0e30)).is_err());
        assert!(validate(&s, &json!("3")).is_err());
    }

    #[test]
    fn a_tool_with_no_arguments_says_so() {
        let none = json!({"type": "object", "properties": {}, "additionalProperties": false});
        assert_eq!(validate(&none, &json!({})), Ok(()));
        assert!(validate(&none, &json!({"x": 1})).expect_err("refused").contains("takes no arguments"));
    }

    #[test]
    fn hostile_nesting_is_refused_not_recursed_into() {
        let mut schema = json!({"type": "string"});
        let mut value = json!("x");
        for _ in 0..100 {
            schema = json!({"type": "array", "items": schema});
            value = json!([value]);
        }
        assert!(validate(&schema, &value).expect_err("too deep").contains("levels deep"));
    }

    #[test]
    fn unknown_schema_keywords_and_non_object_schemas_are_ignored() {
        assert_eq!(validate(&json!(true), &json!(1)), Ok(()));
        assert_eq!(validate(&json!({"format": "uri", "pattern": "x"}), &json!("anything")), Ok(()));
        assert_eq!(validate(&json!({"type": "madeup"}), &json!(1)), Ok(()), "an unknown type name constrains nothing");
    }
}
