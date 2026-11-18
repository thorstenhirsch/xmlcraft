//! `xquery_eval`: an XQuery 3.1 query run with an optional document as the context item.
//!
//! The document is an open document (`id`), a file (`path`, through the root confinement) or `text`; without one the query runs
//! with no context item. External variables come from `variables`, documents `fn:doc` may open from `documents` (nothing else is
//! read). Nothing is changed.

use serde_json::{Value, json};
use xmlcraft_xquery::{Options, ParamValue, Serialization, Variable, evaluate};

use crate::documents::{fail, json_result, source, source_properties};
use crate::tools::ToolDef;
use crate::xpath::{bounded, error_text, items_json};
use crate::{Automation, Content, Result, ToolError};

/// The most items one answer lists.
const MAX_ITEMS: usize = 1000;
/// The most interpreter steps a run may take.
const MAX_STEPS: u64 = 1_000_000_000;
/// The most bytes of serialized output returned.
const MAX_OUTPUT: usize = 8 * 1024 * 1024;

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut properties = source_properties();
    if let Some(map) = properties.as_object_mut() {
        map.insert("query".into(), json!({ "type": "string", "description": "The XQuery 3.1 query (a main module: an optional prolog and a body)" }));
        map.insert(
            "namespaces".into(),
            json!({ "type": "object", "additionalProperties": { "type": "string" }, "description": "Prefixes the query uses besides its own declarations, with their namespaces (xml, xs, xsi, fn, local, math, map, array and err are known)" }),
        );
        map.insert(
            "variables".into(),
            json!({ "type": "object", "additionalProperties": { "type": ["string", "number", "boolean"] }, "description": "Values for the query's external variables (`declare variable $name external;`), by name without `$`: a string is xs:string, a number xs:integer or xs:double, a boolean xs:boolean" }),
        );
        map.insert(
            "documents".into(),
            json!({ "type": "object", "additionalProperties": { "type": "string" }, "description": "Documents fn:doc may open: URI to XML text. Nothing else is read (no files, no network)" }),
        );
        map.insert(
            "serialize".into(),
            json!({ "type": "string", "enum": ["xml", "text", "json", "adaptive"], "description": "Also return the result serialized (`serialized`): the XML, text, JSON or adaptive output method" }),
        );
        map.insert("max_items".into(), json!({ "type": "integer", "minimum": 1, "maximum": MAX_ITEMS, "description": "The most items listed (default 100; the answer says how many there are)" }));
        map.insert(
            "max_steps".into(),
            json!({ "type": "integer", "minimum": 1, "maximum": MAX_STEPS, "description": "Stop the run after this many interpreter steps (default 50 000 000; XMLC0001)" }),
        );
    }
    vec![ToolDef {
        name: "xquery_eval",
        title: "Run an XQuery",
        description: "Run an XQuery 3.1 query, with the document (if one is given) as the context item. Returns `count` and `items` as xpath_eval \
                      lists them (a node of the document has its `path`, `line` and `column`; a node the query built has neither and its `text` is \
                      its serialization) and, with `serialize`, `serialized`. Supported: the prolog (namespaces, variables, functions, options), \
                      FLWOR with group by, order by, count and windows, switch, typeswitch, try/catch, direct and computed constructors. Not \
                      supported (refused with their codes): schema import and validate, module import, XQuery Update, Full-Text. An error has \
                      the W3C code, says whether the query did not compile or failed running, and where in the query.",
        input_schema: json!({ "type": "object", "properties": properties, "required": ["query"], "additionalProperties": false }),
        read_only: true,
        destructive: false,
        handler: xquery_eval,
    }]
}

fn strings(args: &Value, name: &str) -> Result<Vec<(String, String)>> {
    match args.get(name) {
        None => Ok(Vec::new()),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(k, v)| {
                v.as_str().map(|s| (k.clone(), s.to_owned())).ok_or_else(|| ToolError::InvalidArgs(format!("{name}: the value of {k} is a string")))
            })
            .collect(),
        Some(_) => Err(ToolError::InvalidArgs(format!("{name} is an object"))),
    }
}

fn xquery_eval(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let query = args.get("query").and_then(Value::as_str).ok_or_else(|| ToolError::InvalidArgs("`query` is required".into()))?;
    let has_source = args.get("id").is_some() || args.get("path").is_some() || args.get("text").is_some();
    let text = if has_source {
        let s = source(a, args)?;
        if !s.language.is_xml() && args.get("text").is_none() {
            return fail("xquery_eval reads XML documents (a JSON or YAML document is not XML)");
        }
        Some(s.text)
    } else {
        None
    };
    let mut options = Options { namespaces: strings(args, "namespaces")?, documents: strings(args, "documents")?, ..Options::default() };
    match args.get("variables") {
        None => {}
        Some(Value::Object(map)) => {
            for (name, value) in map {
                let value = match value {
                    Value::String(s) => ParamValue::String(s.clone()),
                    Value::Bool(b) => ParamValue::Boolean(*b),
                    Value::Number(n) => ParamValue::Number(n.to_string()),
                    _ => return Err(ToolError::InvalidArgs(format!("variables: ${name} is a string, a number or a boolean"))),
                };
                options.variables.push(Variable { name: name.clone(), value });
            }
        }
        Some(_) => return Err(ToolError::InvalidArgs("variables is an object of names and values".into())),
    }
    options.serialize = match args.get("serialize") {
        None => None,
        Some(Value::String(s)) => {
            Some(Serialization::parse(s).ok_or_else(|| ToolError::InvalidArgs(format!("serialize is xml, text, json or adaptive, not {s:?}")))?)
        }
        Some(_) => return Err(ToolError::InvalidArgs("serialize is a string".into())),
    };
    options.max_items = bounded(args, "max_items", 100, MAX_ITEMS)?;
    options.max_output = MAX_OUTPUT;
    if let Some(v) = args.get("max_steps") {
        options.max_steps = v
            .as_u64()
            .filter(|n| (1..=MAX_STEPS).contains(n))
            .ok_or_else(|| ToolError::InvalidArgs(format!("max_steps is a whole number from 1 to {MAX_STEPS}")))?;
    }
    match evaluate(query, text.as_deref(), &options) {
        Ok(done) => {
            let items = items_json(&done.result, text.as_deref().unwrap_or(""));
            let mut answer =
                json!({ "count": done.result.count, "truncated": done.result.count > items.len(), "items": items, "engine": "XQuery 3.1" });
            if let Some(s) = done.serialized {
                answer["serialized"] = json!(s);
            }
            json_result(answer)
        }
        Err(e) => fail(error_text(&e).replace("of the expression", "of the query")),
    }
}
