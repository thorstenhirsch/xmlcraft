//! `json_sample`: JSON, YAML or BSON made from a JSON Schema.
//!
//! The schema is an open document, a file (read through the root confinement) or text. The schema documents it refers to are found among
//! the open documents, the bundled schemas and files; nothing is downloaded. Nothing is written: the sample is returned.

use serde_json::{Value, json};
use xmlcraft_convert::Format;
use xmlcraft_engine::json_sample::{MAX_DEPTH, Options, sample};
use xmlcraft_engine::json_validation::load_schema_from_text;

use crate::documents::{json_result, source, source_properties};
use crate::tools::ToolDef;
use crate::validate::schema_resolver;
use crate::{Automation, Content, Result, ToolError};

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut properties = source_properties();
    if let Some(map) = properties.as_object_mut() {
        map.insert(
            "output".into(),
            json!({ "type": "string", "enum": ["json", "yaml", "bson"], "description": "What the sample is written in (default json; bson is Extended JSON text)" }),
        );
        map.insert(
            "optional".into(),
            json!({ "type": "boolean", "description": "Make the properties the schema does not require as well (default true)" }),
        );
        map.insert(
            "max_depth".into(),
            json!({ "type": "integer", "minimum": 0, "maximum": MAX_DEPTH, "description": "How deep objects and arrays nest before they are empty (default 6)" }),
        );
    }
    vec![ToolDef {
        name: "json_sample",
        title: "Generate a sample JSON, YAML or BSON value from a JSON Schema",
        description: "Make a value that shows the shape a JSON Schema describes, given as an open document `id`, a file `path` or `text`. The names the schema \
                      declares are followed (the required ones, and the others with `optional`), the first value it offers (`enum`, `const`, a default or an example) \
                      is used, and each place is made from its kind (an object from `properties`, one item from `items`, a string, a number, a boolean or null). \
                      Recursion stops at `max_depth`. Returns `text`, the `output` format and the `warnings` (what the format could not keep). Nothing is written \
                      to disk. The sample shows the shape; it is not promised to validate.",
        input_schema: json!({ "type": "object", "properties": properties, "additionalProperties": false }),
        read_only: true,
        destructive: false,
        handler: json_sample,
    }]
}

fn options_of(args: &Value) -> Result<Options> {
    let mut options = Options {
        format: match args.get("output").and_then(Value::as_str) {
            None | Some("json") => Format::Json,
            Some("yaml") => Format::Yaml,
            Some("bson") => Format::Bson,
            Some(other) => return Err(ToolError::InvalidArgs(format!("output is json, yaml or bson, not {other:?}"))),
        },
        ..Options::default()
    };
    if let Some(optional) = args.get("optional").and_then(Value::as_bool) {
        options.optional = optional;
    }
    if let Some(depth) = args.get("max_depth") {
        options.max_depth = depth
            .as_u64()
            .filter(|n| *n <= MAX_DEPTH as u64)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| ToolError::InvalidArgs(format!("max_depth is a whole number from 0 to {MAX_DEPTH}")))?;
    }
    Ok(options)
}

fn json_sample(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let options = options_of(args)?;
    let schema = source(a, args)?;
    let location = schema.path.as_deref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let resolver = schema_resolver(a);
    let set = load_schema_from_text(&schema.text, &location, &resolver).map_err(ToolError::Failed)?;
    let made = sample(&set, &options).map_err(ToolError::Failed)?;
    json_result(json!({
        "text": made.text,
        "output": options.format.name(),
        "warnings": made.warnings,
    }))
}
