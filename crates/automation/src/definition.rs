//! `xsd_definition`: the definition an XML Schema name refers to.

use serde_json::{Value, json};
use xmlcraft_text::Buffer;

use crate::documents::{Source, position_arg, source, source_and_position_properties};
use crate::tools::ToolDef;
use crate::validate::schema_resolver;
use crate::{Automation, Content, Result};

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![ToolDef {
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
    }]
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
