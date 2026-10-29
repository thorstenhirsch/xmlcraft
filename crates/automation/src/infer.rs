//! `xml_infer`: an XML Schema or a DTD generated from one or more documents.
//!
//! The documents are open documents, files (read through the root confinement) or text. Nothing is written: the files of the result are
//! returned, and the caller decides where they go.

use serde_json::{Value, json};
use xmlcraft_infer::{EnumerationRule, Layout, Options, Output, infer};

use crate::documents::{fail, json_result, source, source_properties};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

/// The most documents one call reads.
const MAX_SOURCES: usize = 1000;

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut properties = source_properties();
    if let Some(map) = properties.as_object_mut() {
        map.insert(
            "sources".into(),
            json!({
                "type": "array",
                "minItems": 1,
                "maxItems": MAX_SOURCES,
                "items": { "type": "object", "properties": source_properties(), "additionalProperties": false },
                "description": "Several documents to learn from, each given as an `id`, a `path` or `text` (instead of those three at the top level)"
            }),
        );
        map.insert("output".into(), json!({ "type": "string", "enum": ["xsd", "dtd"], "description": "The kind of schema (default xsd)" }));
        map.insert(
            "layout".into(),
            json!({ "type": "string", "enum": ["russian-doll", "salami-slice", "venetian-blind"], "description": "How an XML Schema is arranged (default russian-doll)" }),
        );
        map.insert("types".into(), json!({ "type": "boolean", "description": "Infer boolean, integer, decimal, double, date, dateTime and time instead of string (default true)" }));
        map.insert(
            "enumerations".into(),
            json!({
                "description": "false for none; true for the default rule; or the rule: at most `max_values` different values, each seen at least `min_occurrences` times (default 10 and 2)",
                "oneOf": [
                    { "type": "boolean" },
                    {
                        "type": "object",
                        "properties": { "max_values": { "type": "integer", "minimum": 2 }, "min_occurrences": { "type": "integer", "minimum": 1 } },
                        "additionalProperties": false
                    }
                ]
            }),
        );
        map.insert("unbounded".into(), json!({ "type": "boolean", "description": "An element seen more than once may occur any number of times (default true); false: at most as often as seen" }));
    }
    vec![ToolDef {
        name: "xml_infer",
        title: "Generate an XSD or a DTD from documents",
        description: "Learn a schema from one or more well-formed documents: the content model of each element from the order and number of its children, \
                      attributes required or optional, simple types (boolean, integer, decimal, double, date, dateTime, time, else string) and enumerations. \
                      A schema inferred from some documents validates each of them. Give `sources` (or one `id`, `path` or `text`). For an XML Schema there \
                      is one file per namespace, in the `layout` asked for; for a DTD one file. Returns `files`: each with its `name`, `namespace` and `text`. \
                      Nothing is written to disk.",
        input_schema: json!({ "type": "object", "properties": properties, "additionalProperties": false }),
        read_only: true,
        destructive: false,
        handler: xml_infer,
    }]
}

fn options_of(args: &Value) -> Result<Options> {
    let mut options = Options::default();
    let layout = match args.get("layout").and_then(Value::as_str) {
        None | Some("russian-doll") => Layout::RussianDoll,
        Some("salami-slice") => Layout::SalamiSlice,
        Some("venetian-blind") => Layout::VenetianBlind,
        Some(other) => return Err(ToolError::InvalidArgs(format!("layout is russian-doll, salami-slice or venetian-blind, not {other:?}"))),
    };
    options.output = match args.get("output").and_then(Value::as_str) {
        None | Some("xsd") => Output::Xsd { layout },
        Some("dtd") => Output::Dtd,
        Some(other) => return Err(ToolError::InvalidArgs(format!("output is xsd or dtd, not {other:?}"))),
    };
    if let Some(types) = args.get("types").and_then(Value::as_bool) {
        options.detect_types = types;
    }
    if let Some(unbounded) = args.get("unbounded").and_then(Value::as_bool) {
        options.unbounded = unbounded;
    }
    match args.get("enumerations") {
        None | Some(Value::Bool(true)) => {}
        Some(Value::Bool(false)) => options.enumerations = None,
        Some(rule @ Value::Object(_)) => {
            let default = EnumerationRule::default();
            let number = |key: &str, fallback: usize| -> Result<usize> {
                match rule.get(key) {
                    None => Ok(fallback),
                    Some(v) => v
                        .as_u64()
                        .and_then(|n| usize::try_from(n).ok())
                        .filter(|n| *n >= 1)
                        .ok_or_else(|| ToolError::InvalidArgs(format!("{key} is a whole number from 1"))),
                }
            };
            options.enumerations = Some(EnumerationRule {
                max_values: number("max_values", default.max_values)?,
                min_occurrences: number("min_occurrences", default.min_occurrences)?,
            });
        }
        Some(_) => return Err(ToolError::InvalidArgs("enumerations is a boolean or an object with max_values and min_occurrences".into())),
    }
    Ok(options)
}

fn xml_infer(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let options = options_of(args)?;
    let mut texts: Vec<String> = Vec::new();
    let mut lossy = false;
    match args.get("sources") {
        Some(Value::Array(list)) => {
            if args.get("id").is_some() || args.get("path").is_some() || args.get("text").is_some() {
                return Err(ToolError::InvalidArgs("give `sources`, or one of id, path and text, not both".into()));
            }
            if list.is_empty() || list.len() > MAX_SOURCES {
                return Err(ToolError::InvalidArgs(format!("sources has from 1 to {MAX_SOURCES} documents")));
            }
            for item in list {
                let s = source(a, item)?;
                lossy |= s.lossy;
                texts.push(s.text);
            }
        }
        Some(_) => return Err(ToolError::InvalidArgs("sources is a list of documents".into())),
        None => {
            let s = source(a, args)?;
            lossy |= s.lossy;
            texts.push(s.text);
        }
    }
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let generated = match infer(&refs, &options) {
        Ok(generated) => generated,
        Err(e) => return fail(e.to_string()),
    };
    let files: Vec<Value> = generated.files.iter().map(|f| json!({ "name": f.name, "namespace": f.namespace, "text": f.text })).collect();
    json_result(json!({ "files": files, "documents": refs.len(), "lossy": lossy }))
}
