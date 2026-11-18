//! `xsd_definition`: the definition an XML Schema name refers to.

use serde_json::{Value, json};
use xmlcraft_text::Buffer;

use crate::documents::{Source, language_source_properties, position_arg, source, source_and_position_properties};
use crate::tools::ToolDef;
use crate::validate::schema_resolver;
use crate::{Automation, Content, Result};

fn data_position_properties() -> Value {
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
            name: "json_definition",
            title: "Find where a $ref of a JSON or YAML document points",
            description: "Find the target of the `$ref` at a place of a JSON, JSONC, JSON5, BSON (Extended JSON) or YAML document (JSON Schema, OpenAPI): `#/definitions/x` or `other.json#/x`, the fragment a JSON Pointer (RFC 6901: `~0`, `~1` and percent-encoding are decoded). A file part is read with the resolver the \
                      validation uses (open documents, the schemas that ship with the app, files through the root confinement; nothing is downloaded unless the session works online), relative to the document. Only one hop is followed. `$id` and `$anchor` references are not resolved. \
                      `line` and `column` (from 1) or a byte `offset` say the place (text needs `language`); the result has `found`, `reference`, the `file` (when it is another document), the byte range, the `line` and `column` (from 1) of the target's name \
                      (the member's key; the start of the item for an array element) and `selected`. A place that is no `$ref` is `found: false`; a reference that cannot be followed (the file is not available, the pointer names nothing) is an error that says why.",
            input_schema: json!({ "type": "object", "properties": data_position_properties(), "additionalProperties": false }),
            read_only: true,
            destructive: false,
            handler: json_definition,
        },
        ToolDef {
            name: "json_pointer_at",
            title: "The JSON Pointer of a place in a JSON, YAML or BSON document",
            description: "The JSON Pointer (RFC 6901) of the value at a place of a JSON, JSONC, JSON5, JSON Lines (relative to the line), BSON (Extended JSON) or YAML (relative to its document) text: what Copy JSON Pointer in the Outline puts on the clipboard. `line` and `column` (from 1) or a byte `offset`; text needs `language`. The root is the empty pointer.",
            input_schema: json!({ "type": "object", "properties": data_position_properties(), "additionalProperties": false }),
            read_only: true,
            destructive: false,
            handler: json_pointer_at,
        },
        ToolDef {
            name: "xsd_definition",
            title: "Find the definition of an XML Schema name",
            description: "Find where a name in an XML Schema document is defined: the type a `type` or `base` names, the global element a `ref` or \
                      `substitutionGroup` names, and the group or attribute group a `ref` names. The schema is read with the resolver the \
                      validation uses (open documents as they are edited, the schemas that ship with the app, catalogs and files through the \
                      root confinement; nothing is downloaded unless the session works online). `line` and `column` (from 1) or a byte `offset` \
                      say the place; the result has `found`, the `kind`, the expanded `name`, the `file` (when it is another document), \
                      the byte range and the `line` and `column` (from 1) where the definition's name starts and ends. A place that names nothing, or a name that is not defined, is `found: false`.",
            input_schema: json!({
                "type": "object",
                "properties": source_and_position_properties(),
                "additionalProperties": false,
            }),
            read_only: true,
            destructive: false,
            handler: xsd_definition,
        },
    ]
}

fn xsd_definition(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let source: Source = source(a, args)?;
    if source.language != xmlcraft_engine::language::Language::Xml {
        return crate::documents::fail("xsd_definition reads an XML Schema document, which is XML");
    }
    let buffer = Buffer::from_text(&source.text);
    let offset = position_arg(&source.text, &buffer, args)?;
    let location = source.path.as_deref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let resolver = schema_resolver(a);
    let found = xmlcraft_engine::xsd_definition::definition(&source.text, &location, offset, &resolver).map_err(crate::ToolError::Failed)?;
    let mut value = json!({
        "language": "xsd",
        "offset": offset,
        "found": found.is_some(),
        "file": Value::Null,
        "start": Value::Null,
        "end": Value::Null,
        "lossy": source.lossy,
    });
    if let Some(definition) = found {
        value["kind"] = json!(definition.kind.as_str());
        value["name"] = json!(definition.name);
        // The file is named when it is another document than the one asked about; the place is given as lines and columns either way.
        if !definition.location.is_empty() && definition.location != location {
            value["file"] = json!(definition.location);
        }
        value["line"] = json!(definition.line.saturating_add(1));
        value["column"] = json!(definition.column.saturating_add(1));
        value["end_line"] = json!(definition.end_line.saturating_add(1));
        value["end_column"] = json!(definition.end_column.saturating_add(1));
        value["start"] = json!(definition.start);
        value["end"] = json!(definition.end);
    }
    crate::documents::json_result(value)
}

/// The document and the place of a JSON, YAML or BSON tool: the language is the document's, or the one given for text.
fn data_source(a: &Automation, args: &Value, tool: &str) -> Result<(Source, usize)> {
    let source: Source = source(a, args)?;
    if !source.language.is_data() {
        return crate::documents::fail(format!(
            "{tool} reads JSON, JSONC, JSON5, BSON or YAML: give language json, jsonc, json5, jsonl, bson or yaml for text"
        ));
    }
    let buffer = Buffer::from_text(&source.text);
    let offset = position_arg(&source.text, &buffer, args)?;
    Ok((source, offset))
}

fn json_definition(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let (source, offset) = data_source(a, args, "json_definition")?;
    let location = source.path.as_deref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let resolver = schema_resolver(a);
    let found = xmlcraft_engine::json_definition::definition(source.language, &source.text, &location, offset, &resolver)
        .map_err(crate::ToolError::Failed)?;
    let mut value = json!({
        "language": source.language.name(),
        "offset": offset,
        "found": found.is_some(),
        "file": Value::Null,
        "start": Value::Null,
        "end": Value::Null,
        "lossy": source.lossy,
    });
    if let Some(definition) = found {
        value["reference"] = json!(definition.name);
        value["selected"] = json!(definition.selected);
        if !definition.location.is_empty() && definition.location != location {
            value["file"] = json!(definition.location);
        }
        value["line"] = json!(definition.line.saturating_add(1));
        value["column"] = json!(definition.column.saturating_add(1));
        value["end_line"] = json!(definition.end_line.saturating_add(1));
        value["end_column"] = json!(definition.end_column.saturating_add(1));
        value["start"] = json!(definition.start);
        value["end"] = json!(definition.end);
    }
    crate::documents::json_result(value)
}

fn json_pointer_at(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let (source, offset) = data_source(a, args, "json_pointer_at")?;
    let pointer = xmlcraft_engine::json_definition::json_pointer_at(&source.text, source.language, offset);
    crate::documents::json_result(json!({ "language": source.language.name(), "offset": offset, "pointer": pointer }))
}
