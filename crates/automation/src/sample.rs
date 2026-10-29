//! `xml_sample`: an XML document generated from an XML Schema or a DTD.
//!
//! The grammar is an open document, a file (read through the root confinement) or text. The schema documents it includes and imports are
//! found among the open documents, the schemas that ship with the app and files; nothing is downloaded. Nothing is written: the document
//! is returned.

use serde_json::{Value, json};
use xmlcraft_engine::generate::{GrammarKind, sample_document};
use xmlcraft_sample::{ChoicePolicy, Fill, Options};

use crate::documents::{json_result, source, source_properties};
use crate::tools::ToolDef;
use crate::validate::schema_resolver;
use crate::{Automation, Content, Result, ToolError};

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut properties = source_properties();
    if let Some(map) = properties.as_object_mut() {
        map.insert("kind".into(), json!({ "type": "string", "enum": ["xsd", "dtd"], "description": "What the document is (default: an XML Schema when its root element is `schema`, else a DTD)" }));
        map.insert(
            "root".into(),
            json!({ "type": "string", "description": "The element to start with: its name, or `{namespace}name` (default: the first that can be made)" }),
        );
        map.insert("optional".into(), json!({ "type": "boolean", "description": "Make the optional elements and attributes (default true)" }));
        map.insert("repeat".into(), json!({ "type": "integer", "minimum": 0, "maximum": 100, "description": "How often an element that may occur more than once occurs, within its bounds (default 1)" }));
        map.insert("choice".into(), json!({ "type": "string", "enum": ["first", "rotate"], "description": "Which branch of a choice: the first, or each in turn (default first)" }));
        map.insert("fill".into(), json!({ "type": "string", "enum": ["typed", "named"], "description": "typed: `string`, `1`, `2000-01-01`; named: a string is the name of its element or attribute (default typed)" }));
        map.insert("max_depth".into(), json!({ "type": "integer", "minimum": 0, "maximum": 50, "description": "How deep elements nest before only what is required is made (default 6)" }));
        map.insert("schema_location".into(), json!({ "type": "string", "description": "Where the schema is: said in the document with xsi:schemaLocation or xsi:noNamespaceSchemaLocation" }));
    }
    vec![ToolDef {
        name: "xml_sample",
        title: "Generate a sample XML document from an XSD or a DTD",
        description: "Make an XML document that is valid against an XML Schema or a DTD, given as an open document `id`, a file `path` or `text`. The content \
                      models are followed (a choice takes a branch, an element repeats `repeat` times within its bounds); values come from fixed values, defaults, \
                      enumerations, patterns and the kind of each type, each checked by the type itself; recursion stops at `max_depth` with only what is required. \
                      Returns `text`, the `warnings` (a value or an element that could not be made as asked), the `root` used and the `roots` that could be. \
                      Nothing is written to disk.",
        input_schema: json!({ "type": "object", "properties": properties, "additionalProperties": false }),
        read_only: true,
        destructive: false,
        handler: xml_sample,
    }]
}

fn options_of(args: &Value) -> Result<Options> {
    let mut options = Options::default();
    if let Some(optional) = args.get("optional").and_then(Value::as_bool) {
        options.optional = optional;
    }
    if let Some(repeat) = args.get("repeat") {
        options.repeat = repeat
            .as_u64()
            .filter(|n| *n <= 100)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| ToolError::InvalidArgs("repeat is a whole number from 0 to 100".into()))?;
    }
    options.choice = match args.get("choice").and_then(Value::as_str) {
        None | Some("first") => ChoicePolicy::First,
        Some("rotate") => ChoicePolicy::Rotate,
        Some(other) => return Err(ToolError::InvalidArgs(format!("choice is first or rotate, not {other:?}"))),
    };
    options.fill = match args.get("fill").and_then(Value::as_str) {
        None | Some("typed") => Fill::Typed,
        Some("named") => Fill::Named,
        Some(other) => return Err(ToolError::InvalidArgs(format!("fill is typed or named, not {other:?}"))),
    };
    if let Some(depth) = args.get("max_depth") {
        options.max_depth = depth
            .as_u64()
            .filter(|n| *n <= 50)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| ToolError::InvalidArgs("max_depth is a whole number from 0 to 50".into()))?;
    }
    options.schema_location = args.get("schema_location").and_then(Value::as_str).map(str::to_owned);
    Ok(options)
}

fn xml_sample(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let options = options_of(args)?;
    let grammar = source(a, args)?;
    let kind = match args.get("kind").and_then(Value::as_str) {
        Some("xsd") => Some(GrammarKind::Xsd),
        Some("dtd") => Some(GrammarKind::Dtd),
        Some(_) | None => None,
    };
    let location = grammar.path.as_deref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let resolver = schema_resolver(a);
    // The external parts of a DTD are found the same way (the confinement lets through the files).
    let mut subsets = xmlcraft_dtd::ResolvingSubsetLoader::new(&resolver);
    let made = sample_document(&grammar.text, &location, kind, args.get("root").and_then(Value::as_str), &options, &resolver, &mut subsets)
        .map_err(ToolError::Failed)?;
    json_result(json!({
        "kind": made.kind.name(),
        "text": made.text,
        "warnings": made.warnings,
        "root": made.root,
        "roots": made.roots,
    }))
}
