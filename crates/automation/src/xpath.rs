//! `xpath_eval`: an XPath 3.1 expression evaluated over a document.
//!
//! The document is an open document (`id`), a file (`path`, through the root confinement) or `text`. Nothing is changed.

use serde_json::{Map, Value, json};
use xmlcraft_query::{Options, Phase, evaluate};
use xmlcraft_text::Buffer;

use crate::documents::{fail, json_result, position_json, source, source_properties};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

/// The most items one answer lists.
const MAX_ITEMS: usize = 1000;

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut properties = source_properties();
    if let Some(map) = properties.as_object_mut() {
        map.insert("expression".into(), json!({ "type": "string", "description": "The XPath expression" }));
        map.insert(
            "namespaces".into(),
            json!({ "type": "object", "additionalProperties": { "type": "string" }, "description": "Prefixes the expression uses, with their namespaces (xs, fn, map, array, math and xml are known)" }),
        );
        map.insert(
            "version".into(),
            json!({ "type": "string", "enum": ["1.0", "2.0", "3.0", "3.1"], "description": "The XPath version asked for (default 3.1). The engine is 3.1's: the older versions are subsets of it and run under 3.1's rules (1.0's compatibility mode is not implemented)" }),
        );
        map.insert("max_items".into(), json!({ "type": "integer", "minimum": 1, "maximum": MAX_ITEMS, "description": "The most items listed (default 100; the answer says how many there are)" }));
    }
    vec![ToolDef {
        name: "xpath_eval",
        title: "Evaluate an XPath expression",
        description: "Evaluate an XPath 3.1 expression with the document as the context. Returns `count` and `items`: for a node its `kind` (element, attribute, \
                      text, comment, processing-instruction, document), `name`, `path` (as fn:path writes it), `line` and `column` in the document, and its `text` as \
                      written; for a value its type (xs:string, xs:integer, …, map, array) and its XPath representation. An error has the W3C code (XPST0003 …), says \
                      whether the expression did not compile or failed running, and where in the expression.",
        input_schema: json!({ "type": "object", "properties": properties, "required": ["expression"], "additionalProperties": false }),
        read_only: true,
        destructive: false,
        handler: xpath_eval,
    }]
}

fn xpath_eval(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let expression = args.get("expression").and_then(Value::as_str).ok_or_else(|| ToolError::InvalidArgs("`expression` is required".into()))?;
    let s = source(a, args)?;
    if !s.language.is_xml() && args.get("text").is_none() {
        return fail("xpath_eval reads XML documents (a JSON or YAML document is not XML)");
    }
    let mut options = Options::default();
    // (The version is checked and given back; the engine is 3.1's whatever is asked.)
    if let Some(version) = args.get("version").and_then(Value::as_str)
        && !matches!(version, "1.0" | "2.0" | "3.0" | "3.1")
    {
        return Err(ToolError::InvalidArgs(format!("version is 1.0, 2.0, 3.0 or 3.1, not {version:?}")));
    }
    match args.get("namespaces") {
        None => {}
        Some(Value::Object(map)) => {
            for (prefix, uri) in map {
                let uri = uri.as_str().ok_or_else(|| ToolError::InvalidArgs(format!("the namespace of {prefix} is a string")))?;
                options.namespaces.push((prefix.clone(), uri.to_owned()));
            }
        }
        Some(_) => return Err(ToolError::InvalidArgs("namespaces is an object of prefixes and namespaces".into())),
    }
    let listed = match args.get("max_items") {
        None => 100,
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| (1..=MAX_ITEMS).contains(n))
            .ok_or_else(|| ToolError::InvalidArgs(format!("max_items is a whole number from 1 to {MAX_ITEMS}")))?,
    };
    options.max_items = listed;
    match evaluate(&s.text, expression, &options) {
        Ok(done) => {
            let buffer = Buffer::from_text(&s.text);
            let items: Vec<Value> = done
                .items
                .iter()
                .map(|i| {
                    let mut item = Map::new();
                    item.insert("kind".into(), json!(i.kind));
                    if !i.name.is_empty() {
                        item.insert("name".into(), json!(i.name));
                    }
                    if let Some(path) = &i.path {
                        item.insert("path".into(), json!(path));
                    }
                    if i.unmapped {
                        item.insert("unmapped".into(), json!(true));
                    }
                    if let Some(span) = &i.span {
                        let (line, column) = position_json(&buffer, span.start.min(buffer.len()));
                        item.insert("line".into(), json!(line));
                        item.insert("column".into(), json!(column));
                    }
                    item.insert("text".into(), json!(i.text));
                    if i.cut {
                        item.insert("cut".into(), json!(true));
                    }
                    Value::Object(item)
                })
                .collect();
            json_result(
                json!({ "count": done.count, "truncated": done.count > items.len(), "items": items, "version": args.get("version").cloned().unwrap_or(json!("3.1")), "engine": "XPath 3.1" }),
            )
        }
        Err(e) => {
            let phase = match e.phase {
                Phase::Document => "document",
                Phase::Compile => "compile",
                Phase::Run => "run",
            };
            let place = e.span.as_ref().map(|r| format!(" (at {}..{} of the expression)", r.start, r.end)).unwrap_or_default();
            fail(format!("{phase} error {e}{place}"))
        }
    }
}
