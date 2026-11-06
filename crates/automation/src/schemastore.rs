//! `schemastore_find`: the JSON Schemas SchemaStore has for a file, by its name. The catalog is given (`catalog_text`, or a file through
//! the root confinement) or, when neither is, fetched from schemastore.org through the session's resolver (which is offline unless the
//! program gave the session a network that downloads, and which keeps what it downloaded).

use serde_json::{Value, json};
use xmlcraft_engine::schemastore::{CATALOG_URL, matching, parse_catalog};
use xmlcraft_resolve::{Budget, Request};
use xmlcraft_text::encoding::{decode, sniff};

use crate::documents::{fail, json_result, text_arg};
use crate::tools::ToolDef;
use crate::validate::schema_resolver;
use crate::{Automation, Content, Result, ToolError};

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "schemastore_find",
        title: "Find the JSON Schema for a file in SchemaStore",
        description: "The schemas of SchemaStore (schemastore.org) that are for a file, by its `name` (a file name or a path: `package.json`, `.github/workflows/ci.yml`), the better fits first, each \
                      with its name, description, url and the patterns of file names it is for. The catalog is `catalog_text`, or the file `catalog` (inside the root), or, when neither is given, \
                      downloaded from schemastore.org through the session's network (refused while the session is offline). Nothing is assigned: pass the url to xml_assign_schema.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "The file name or path to find a schema for" },
                "catalog_text": { "type": "string", "description": "The text of a SchemaStore catalog, instead of downloading it" },
                "catalog": { "type": "string", "description": "A catalog file, instead of downloading it" },
                "max": { "type": "integer", "minimum": 1, "maximum": 100, "description": "The most schemas listed (default 20)" },
            },
            "required": ["name"],
            "additionalProperties": false,
        }),
        read_only: true,
        destructive: false,
        handler: schemastore_find,
    }]
}

fn schemastore_find(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let name = text_arg(args, "name").filter(|n| !n.is_empty()).ok_or_else(|| ToolError::InvalidArgs("`name` is required".into()))?;
    let max = match args.get("max") {
        None => 20,
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| (1..=100).contains(n))
            .ok_or_else(|| ToolError::InvalidArgs("max is a whole number from 1 to 100".into()))?,
    };
    let text = if let Some(text) = text_arg(args, "catalog_text") {
        text.to_owned()
    } else if let Some(path) = text_arg(args, "catalog") {
        let real = a.confinement.resolve_read(path)?;
        let loaded = xmlcraft_platform::files::read_file(&real).map_err(|e| ToolError::Failed(format!("{}: {e}", real.display())))?;
        String::from_utf8_lossy(&loaded.bytes).into_owned()
    } else {
        if !a.network().can_download() || a.network().is_offline() {
            return fail(
                "no catalog was given and the session is offline, so SchemaStore is not asked: pass `catalog_text` or `catalog`, or let a person allow the network",
            );
        }
        let resolver = schema_resolver(a);
        let found = resolver.resolve(&Request::uri(CATALOG_URL, ""), &mut Budget::default()).map_err(|f| ToolError::Failed(f.message))?;
        decode(&found.bytes, &sniff(&found.bytes)).text
    };
    let entries = parse_catalog(&text).map_err(ToolError::Failed)?;
    let found: Vec<Value> = matching(&entries, name)
        .into_iter()
        .take(max)
        .map(|e| json!({ "name": e.name, "description": e.description, "url": e.url, "file_match": e.patterns }))
        .collect();
    json_result(json!({ "name": name, "catalog_entries": entries.len(), "count": found.len(), "schemas": found }))
}
