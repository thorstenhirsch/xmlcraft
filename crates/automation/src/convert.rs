//! `data_convert`: a document converted between XML, JSON, YAML and BSON (as Extended JSON).
//!
//! The source is an open document, a file (through the root confinement) or text; nothing is written. The text of the result is returned
//! for the caller to open (`doc_new`) or save (`doc_save`).

use serde_json::{Value, json};
use xmlcraft_convert::{ConvertOptions, Format, convert};
use xmlcraft_engine::language::Language;

use crate::documents::{fail, json_result, source, source_properties};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut properties = source_properties();
    if let Some(map) = properties.as_object_mut() {
        let formats = json!(["xml", "json", "yaml", "bson"]);
        map.insert(
            "from".into(),
            json!({ "type": "string", "enum": formats, "description": "What the source is written in. Optional for an open document or a file (its name tells), required for `text`" }),
        );
        map.insert("to".into(), json!({ "type": "string", "enum": formats, "description": "What to convert to" }));
        map.insert("indent".into(), json!({ "type": "integer", "minimum": 0, "maximum": 16, "description": "Spaces of indentation (default 2; 0 writes compact JSON; YAML and XML are always indented)" }));
        map.insert(
            "standard_mapping".into(),
            json!({ "type": "boolean", "description": "Use the XPath 3.1 `fn:json-to-xml` mapping between XML and JSON (lossless for JSON) instead of the natural one (elements as members, attributes as `@name`, text as `#text`)" }),
        );
        map.insert(
            "attribute_prefix".into(),
            json!({ "type": "string", "description": "The natural mapping: what an attribute's member name starts with (default @)" }),
        );
        map.insert("text_key".into(), json!({ "type": "string", "description": "The natural mapping: the member that holds the text of an element that has attributes or children (default #text)" }));
        map.insert("guess_types".into(), json!({ "type": "boolean", "description": "XML to data: text that is a number, true, false or null becomes that (default false: strings)" }));
        map.insert(
            "root".into(),
            json!({ "type": "string", "description": "Data to XML: the root element when the data is not an object with one member (default root)" }),
        );
        map.insert(
            "item".into(),
            json!({ "type": "string", "description": "Data to XML: the element of array items that need a name (default item)" }),
        );
        map.insert("declaration".into(), json!({ "type": "boolean", "description": "Data to XML: write the XML declaration (default true)" }));
    }
    vec![ToolDef {
        name: "data_convert",
        title: "Convert between XML, JSON, YAML and BSON",
        description: "Convert a document to another of the formats XML, JSON, YAML and BSON (the Extended JSON text). Give an open document (`id`), a file \
                      (`path`) or `text` (with `from`), and `to`. Returns `text` and `warnings`: what the target could not hold exactly (NaN in JSON, the \
                      types of BSON in other formats, YAML anchors that were expanded). Between data formats the value tree is kept; XML becomes data by \
                      the natural mapping or, with `standard_mapping`, by the XPath 3.1 one. Nothing is written to disk: open the text with doc_new or \
                      save it with doc_save.",
        input_schema: json!({ "type": "object", "properties": properties, "required": ["to"], "additionalProperties": false }),
        read_only: true,
        destructive: false,
        handler: data_convert,
    }]
}

fn format_arg(args: &Value, key: &str) -> Result<Option<Format>> {
    match args.get(key) {
        None => Ok(None),
        Some(Value::String(name)) => {
            Format::parse(name).map(Some).ok_or_else(|| ToolError::InvalidArgs(format!("{key} is xml, json, yaml or bson, not {name:?}")))
        }
        Some(_) => Err(ToolError::InvalidArgs(format!("{key} is xml, json, yaml or bson"))),
    }
}

fn format_of(language: Language) -> Format {
    match language {
        Language::Xml => Format::Xml,
        Language::Json(_) => Format::Json,
    }
}

fn text_option(args: &Value, key: &str) -> Result<Option<String>> {
    match args.get(key) {
        None => Ok(None),
        Some(Value::String(s)) if !s.is_empty() => Ok(Some(s.clone())),
        Some(_) => Err(ToolError::InvalidArgs(format!("{key} is a non-empty string"))),
    }
}

fn bool_option(args: &Value, key: &str) -> Result<Option<bool>> {
    match args.get(key) {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(ToolError::InvalidArgs(format!("{key} is true or false"))),
    }
}

fn options_of(args: &Value) -> Result<ConvertOptions> {
    let mut options = ConvertOptions::default();
    if let Some(indent) = args.get("indent") {
        let n = indent.as_u64().and_then(|n| usize::try_from(n).ok()).filter(|n| *n <= 16);
        options.indent = Some(n.ok_or_else(|| ToolError::InvalidArgs("indent is a whole number from 0 to 16".into()))?).filter(|n| *n > 0);
    }
    options.standard_mapping = bool_option(args, "standard_mapping")?.unwrap_or(false);
    if let Some(prefix) = args.get("attribute_prefix") {
        let prefix = prefix.as_str().ok_or_else(|| ToolError::InvalidArgs("attribute_prefix is a string".into()))?;
        options.xml_to_data.attribute_prefix = prefix.to_owned();
        options.data_to_xml.attribute_prefix = prefix.to_owned();
    }
    if let Some(key) = text_option(args, "text_key")? {
        options.xml_to_data.text_key = key.clone();
        options.data_to_xml.text_key = key;
    }
    if let Some(guess) = bool_option(args, "guess_types")? {
        options.xml_to_data.guess_types = guess;
    }
    if let Some(root) = text_option(args, "root")? {
        options.data_to_xml.root = root;
    }
    if let Some(item) = text_option(args, "item")? {
        options.data_to_xml.item = item;
    }
    if let Some(declaration) = bool_option(args, "declaration")? {
        options.data_to_xml.declaration = declaration;
    }
    Ok(options)
}

fn data_convert(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let Some(to) = format_arg(args, "to")? else { return Err(ToolError::InvalidArgs("`to` is required".into())) };
    let options = options_of(args)?;
    let s = source(a, args)?;
    let from = match (format_arg(args, "from")?, args.get("text").is_some()) {
        (Some(from), _) => from,
        (None, false) => format_of(s.language),
        (None, true) => return Err(ToolError::InvalidArgs("`from` is required with `text`".into())),
    };
    match convert(&s.text, from, to, &options) {
        Ok(done) => json_result(json!({ "text": done.text, "warnings": done.warnings, "from": from.name(), "to": to.name(), "lossy": s.lossy })),
        Err(e) => fail(format!("the {} text cannot be read or written as {}: {e}", from.name(), to.name())),
    }
}
