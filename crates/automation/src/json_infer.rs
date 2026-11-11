//! `json_infer`: a JSON Schema generated from JSON, YAML or BSON documents.
//!
//! The documents are open documents, files (read through the root confinement) or text, with their language. Nothing is written: the
//! schema is returned as text, and the caller decides where it goes.

use serde_json::{Value, json};
use xmlcraft_engine::json_infer::{Document, EnumerationRule, MAX_DOCUMENTS, MAX_TOTAL_BYTES, Options, infer};

use crate::documents::{fail, json_result, language_source_properties, source};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut properties = language_source_properties();
    if let Some(map) = properties.as_object_mut() {
        map.insert(
            "sources".into(),
            json!({
                "type": "array",
                "minItems": 1,
                "maxItems": MAX_DOCUMENTS,
                "items": { "type": "object", "properties": language_source_properties(), "additionalProperties": false },
                "description": "Several documents to learn from, each with an `id`, a `path` or `text`, and its `language` (instead of those at the top level)"
            }),
        );
        map.insert(
            "enumerations".into(),
            json!({
                "description": "false for none; true for the default rule; or the rule: at most `max_values` different strings, each seen at least `min_occurrences` times (default 10 and 2)",
                "oneOf": [
                    { "type": "boolean" },
                    {
                        "type": "object",
                        "properties": { "max_values": { "type": "integer", "minimum": 1 }, "min_occurrences": { "type": "integer", "minimum": 1 } },
                        "additionalProperties": false
                    }
                ]
            }),
        );
    }
    vec![ToolDef {
        name: "json_infer",
        title: "Generate a JSON Schema from JSON, YAML or BSON documents",
        description: "Learn a JSON Schema (draft 2020-12) from one or more documents: object names (required when every object has them), array items, \
                      string, integer, number, boolean and null types, and enumerations for strings that repeat. Give `sources` (or one `id`, `path` or \
                      `text`, with `language`). A `.bson` file is read as the Extended JSON text of its documents. Returns `schema` as text; \
                      nothing is written to disk.",
        input_schema: json!({ "type": "object", "properties": properties, "additionalProperties": false }),
        read_only: true,
        destructive: false,
        handler: json_infer,
    }]
}

fn options_of(args: &Value) -> Result<Options> {
    let mut options = Options::default();
    match args.get("enumerations") {
        None | Some(Value::Bool(true)) => {}
        Some(Value::Bool(false)) => options.enumerations = None,
        Some(rule @ Value::Object(_)) => {
            let default = EnumerationRule::default();
            let max_values = match rule.get("max_values") {
                None => default.max_values,
                Some(v) => v
                    .as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .filter(|n| *n >= 1)
                    .ok_or_else(|| ToolError::InvalidArgs("max_values is a whole number from 1".into()))?,
            };
            let min_occurrences = match rule.get("min_occurrences") {
                None => default.min_occurrences,
                Some(v) => v.as_u64().filter(|n| *n >= 1).ok_or_else(|| ToolError::InvalidArgs("min_occurrences is a whole number from 1".into()))?,
            };
            options.enumerations = Some(EnumerationRule { max_values, min_occurrences });
        }
        Some(_) => return Err(ToolError::InvalidArgs("enumerations is a boolean or an object with max_values and min_occurrences".into())),
    }
    Ok(options)
}

fn json_infer(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let options = options_of(args)?;
    let mut documents: Vec<(String, xmlcraft_engine::language::Language)> = Vec::new();
    let mut lossy = false;
    match args.get("sources") {
        Some(Value::Array(list)) => {
            if args.get("id").is_some() || args.get("path").is_some() || args.get("text").is_some() {
                return Err(ToolError::InvalidArgs("give `sources`, or one of id, path and text, not both".into()));
            }
            if list.is_empty() || list.len() > MAX_DOCUMENTS {
                return Err(ToolError::InvalidArgs(format!("sources has from 1 to {MAX_DOCUMENTS} documents")));
            }
            // The sizes are added up as the documents are read, and reading stops at the cap rather than after the last one.
            let mut total = 0usize;
            for (index, item) in list.iter().enumerate() {
                let s = source(a, item)?;
                total = total.saturating_add(s.text.len());
                if total > MAX_TOTAL_BYTES {
                    return fail(format!(
                        "the documents have more than {MAX_TOTAL_BYTES} bytes together (reading stopped at document {})",
                        index + 1
                    ));
                }
                lossy |= s.lossy;
                documents.push((s.text, s.language));
            }
        }
        Some(_) => return Err(ToolError::InvalidArgs("sources is a list of documents".into())),
        None => {
            let s = source(a, args)?;
            lossy |= s.lossy;
            documents.push((s.text, s.language));
        }
    }
    let refs: Vec<Document<'_>> = documents.iter().map(|(text, language)| Document { text, language: *language }).collect();
    let inferred = match infer(&refs, &options) {
        Ok(inferred) => inferred,
        Err(e) => return fail(e),
    };
    json_result(json!({ "schema": inferred.schema, "documents": inferred.documents, "lossy": lossy }))
}
