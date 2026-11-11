//! `bundled_schemas`: the XML Schemas and DTDs that ship with XmlCraft, to choose one and assign it with `xml_assign_schema`.

use serde_json::{Value, json};
use xmlcraft_engine::bundled::{self, BundledFormat};

use crate::documents::json_result;
use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "bundled_schemas",
        title: "List the XML Schemas and DTDs that ship with XmlCraft",
        description: "The XML Schemas and DTDs in the bundle, each with the path it is in, its set (`w3c`, `ubl`), its `bundled:/…` uri, the public `location` a document names it \
                      by (the publisher's address, which the bundled catalog maps to the file, so the document validates offline; null for a file known only by its namespace, \
                      which cannot be assigned), for a DTD its `public_id`, and for an XML Schema its target namespace. `query` filters by path, file name or set (without case); \
                      `format` keeps only `xsd` or `dtd`. Nothing is assigned: pass the location (or the uri, which is written as the location) to xml_assign_schema.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Only schemas whose path, file name or set contains this" },
                "format": { "type": "string", "enum": ["xsd", "dtd"], "description": "Only XML Schemas or only DTDs" },
                "max": { "type": "integer", "minimum": 1, "maximum": 200, "description": "The most schemas listed (default 200)" },
            },
            "additionalProperties": false,
        }),
        read_only: true,
        destructive: false,
        handler: bundled_schemas,
    }]
}

fn bundled_schemas(_: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let query = args.get("query").and_then(Value::as_str).unwrap_or_default();
    let max = match args.get("max") {
        None => 200,
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| (1..=200).contains(n))
            .ok_or_else(|| ToolError::InvalidArgs("max is a whole number from 1 to 200".into()))?,
    };
    let format = match args.get("format").and_then(Value::as_str) {
        None => None,
        Some("xsd") => Some(BundledFormat::Xsd),
        Some("dtd") => Some(BundledFormat::Dtd),
        Some(other) => return Err(ToolError::InvalidArgs(format!("format is xsd or dtd, not {other:?}"))),
    };
    let found: Vec<bundled::BundledSchema> =
        bundled::matching(query).into_iter().filter(|schema| format.is_none_or(|format| schema.format == format)).collect();
    let matched = found.len();
    let schemas: Vec<Value> = found
        .into_iter()
        .take(max)
        .map(|schema| {
            let namespace = (schema.format == BundledFormat::Xsd).then(|| bundled::namespace_of(&schema.uri)).flatten();
            json!({
                "path": schema.path,
                "uri": schema.uri,
                "name": schema.name,
                "set": schema.set,
                "format": schema.format.as_str(),
                "location": schema.location,
                "public_id": schema.public_id,
                "namespace": namespace,
            })
        })
        .collect();
    json_result(json!({ "total": bundled::list().len(), "matched": matched, "count": schemas.len(), "schemas": schemas }))
}
