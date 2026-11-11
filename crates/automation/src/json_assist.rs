//! `json_complete` and `json_hover`: what the JSON Schema a JSON, JSONC, JSON5, JSON Lines, YAML or BSON document names says may be written
//! at a place (names and values) and what it says of the name or value there. A BSON file is read as the Extended JSON text it is
//! shown as. The schema is found the way validation finds it (the
//! `$schema` member, or the `# yaml-language-server: $schema=` modeline) and read through the same resolver, so a relative path is a
//! file inside the root confinement.

use serde_json::{Value, json};
use xmlcraft_engine::analysis::MAX_ANALYSED_BYTES;
use xmlcraft_engine::json_assist::{CompletionKind, complete, hover};
use xmlcraft_engine::language::Language;
use xmlcraft_text::Buffer;

use crate::documents::{fail, json_result, language_source_properties, position_arg, position_json, source};
use crate::tools::ToolDef;
use crate::validate::schema_resolver;
use crate::{Automation, Content, Result};

fn properties() -> Value {
    let mut properties = language_source_properties();
    if let Some(map) = properties.as_object_mut() {
        map.insert("line".into(), json!({ "type": "integer", "minimum": 1 }));
        map.insert("column".into(), json!({ "type": "integer", "minimum": 1 }));
        map.insert(
            "offset".into(),
            json!({ "type": "integer", "minimum": 0, "description": "A byte offset of the text, instead of line and column" }),
        );
    }
    properties
}

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "json_complete",
            title: "What the JSON Schema allows here",
            description: "What may be written at a position of a JSON, JSONC, JSON5, JSON Lines, YAML or BSON document (an open document `id`, a file `path`, or `text` with `language`; the \
                          position as `line` and `column` from 1, or a byte `offset`), from the JSON Schema the document names (a root `$schema` member, or in YAML the \
                          `# yaml-language-server: $schema=` comment; a relative path is a file; a BSON file is read as its Extended JSON text, the position in that text). At a name: the properties of the object the schema declares, those it requires first, \
                          without the names the object already has; at a value: the values of `enum` and `const`, `true`/`false` for a boolean, `null`, the default and the examples. \
                          Each item has the `label`, the `insert` text, the `cursor` in it, the part of the document it `replace`s (`start`, `end`, what has been typed of a name or \
                          value), a `kind` (property or value), a `detail` (types, or why a value is offered) and the schema's `documentation`. Works on a document that is being typed (an \
                          unclosed bracket, a name with no value yet). `schema` is false when the document names none or it cannot be read.",
            input_schema: json!({ "type": "object", "properties": properties(), "additionalProperties": false }),
            read_only: true,
            destructive: false,
            handler: json_complete,
        },
        ToolDef {
            name: "json_hover",
            title: "What the JSON Schema says of a name or value",
            description: "The words of the JSON Schema a JSON, JSONC, JSON5, JSON Lines, YAML or BSON document names about the name or value at a position (see json_complete for how the document, \
                          the position and the schema are given): the title, the description, the types, the values of an enum, the default, and whether it is deprecated, with the \
                          `start` and `end` of the name or value they are about. Nothing is returned (`found` false) for a place the schema does not describe.",
            input_schema: json!({ "type": "object", "properties": properties(), "additionalProperties": false }),
            read_only: true,
            destructive: false,
            handler: json_hover,
        },
    ]
}

/// The document and the place, for both tools.
fn place(a: &Automation, args: &Value) -> Result<(crate::documents::Source, Language, usize, Buffer)> {
    let src = source(a, args)?;
    if src.text.len() > MAX_ANALYSED_BYTES {
        return fail("the text is too large to analyse");
    }
    let language = src.language;
    if matches!(language, Language::Xml | Language::Html) {
        return fail(
            "this reads JSON, JSONC, JSON5, JSON Lines, YAML and BSON: give language json, jsonc, json5, jsonl, yaml or bson for text, or a document with such a name",
        );
    }
    let buffer = Buffer::from_text(&src.text);
    let offset = position_arg(&src.text, &buffer, args)?;
    Ok((src, language, offset, buffer))
}

fn json_complete(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let (src, language, offset, buffer) = place(a, args)?;
    let location = src.path.as_deref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let resolver = schema_resolver(a);
    let items = complete(&src.text, language, offset, &location, &resolver);
    let listed: Vec<Value> = items
        .iter()
        .map(|c| {
            json!({
                "label": c.label, "insert": c.insert, "cursor": c.cursor, "start": c.replace.start, "end": c.replace.end,
                "kind": match c.kind { CompletionKind::Property => "property", CompletionKind::Value => "value" },
                "detail": c.detail, "documentation": c.documentation, "required": c.required, "deprecated": c.deprecated,
            })
        })
        .collect();
    let (line, column) = position_json(&buffer, offset);
    json_result(
        json!({ "line": line, "column": column, "offset": offset, "count": listed.len(), "items": listed, "schema": !listed.is_empty() || named(&src.text, language), "lossy": src.lossy }),
    )
}

/// Whether the document names a schema (so that an empty list means that nothing may be written, not that there is no schema).
fn named(text: &str, language: Language) -> bool {
    // (A BSON file's Extended JSON is not validated, so it is asked only for the schema it names.)
    xmlcraft_engine::json_validation::names_a_schema(text, language)
        || (language == Language::Bson && xmlcraft_engine::json_validation::schema_reference(text, language).is_some())
}

fn json_hover(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let (src, language, offset, buffer) = place(a, args)?;
    let location = src.path.as_deref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let resolver = schema_resolver(a);
    match hover(&src.text, language, offset, &location, &resolver) {
        Some(h) => {
            let (line, column) = position_json(&buffer, h.range.start.min(buffer.len()));
            let (end_line, end_column) = position_json(&buffer, h.range.end.min(buffer.len()));
            json_result(json!({
                "found": true, "text": h.text, "start": h.range.start, "end": h.range.end,
                "line": line, "column": column, "end_line": end_line, "end_column": end_column, "lossy": src.lossy,
            }))
        }
        None => json_result(json!({ "found": false, "lossy": src.lossy })),
    }
}
