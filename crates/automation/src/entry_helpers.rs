//! `xml_entry_helpers`: what the DTD of a document allows at a place (the elements that may come, the attributes of the element the place
//! is in, the entities that can be referred to), and the text each would put in. The DTD is read the way validation reads it: its external
//! parts are files read through the root confinement; nothing is downloaded.

use serde_json::{Value, json};
use xmlcraft_complete::{choices_at, element_snippet};
use xmlcraft_dtd::doctype_parts;
use xmlcraft_platform::subset::FileSubsetLoader;
use xmlcraft_syntax::parse;
use xmlcraft_text::Buffer;

use crate::documents::{fail, json_result, position_arg, position_json, source, source_and_position_properties};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result};

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "xml_entry_helpers",
        title: "What the DTD allows here",
        description: "What the DTD of a document allows at a position (an open document `id`, a file `path` or `text`; give the position as `line` and `column` from 1, \
                      or a byte `offset`): the elements that may come there, with `required` for those the content model cannot do without, the attributes of the \
                      element the position is in (`required`, `used` when the start tag has it, the values an enumeration lists), and the entities that can be \
                      referred to, each with the text a double click in Entry Helpers would put in. `required_content` (default true) says whether an element's \
                      text also has the children it requires. The DTD is the DOCTYPE's internal subset and the external parts that are files (a relative system \
                      identifier is taken from the folder of the document; nothing is downloaded). A document with no DOCTYPE, or one that cannot be read, \
                      has nothing to offer: `grammar` is false.",
        input_schema: {
            let mut properties = source_and_position_properties();
            if let Some(map) = properties.as_object_mut() {
                map.insert(
                    "required_content".into(),
                    json!({ "type": "boolean", "description": "Put the children an element requires too (default true)" }),
                );
            }
            json!({ "type": "object", "properties": properties, "additionalProperties": false })
        },
        read_only: true,
        destructive: false,
        handler: xml_entry_helpers,
    }]
}

fn xml_entry_helpers(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let source = source(a, args)?;
    if source.text.len() > xmlcraft_engine::analysis::MAX_ANALYSED_BYTES {
        return fail("the text is too large to analyse");
    }
    let buffer = Buffer::from_text(&source.text);
    let offset = position_arg(&source.text, &buffer, args)?;
    let required_content = args.get("required_content").and_then(Value::as_bool).unwrap_or(true);
    let parsed = parse(&source.text);
    let root = parsed.syntax();
    let nothing = |why: &str| json_result(json!({ "grammar": false, "why": why, "lossy": source.lossy }));
    let Some(range) = doctype_parts(&root).map(|p| p.range) else { return nothing("the document has no DOCTYPE") };
    let Some(doctype) = source.text.get(range.clone()) else { return nothing("the DOCTYPE is not text") };
    let confinement = a.confinement.clone();
    let mut loader = FileSubsetLoader::confined(move |path| confinement.resolve_read(path).map_err(|e| e.to_string()));
    let location = source.path.as_deref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let Some(grammar) = xmlcraft_engine::schema::read(doctype, range.start, &location, &mut loader) else {
        return nothing("the DOCTYPE is too large to be read");
    };
    let choices = choices_at(&root, offset, &grammar);
    let (line, column) = position_json(&buffer, offset);
    json_result(json!({
        "grammar": true,
        "line": line,
        "column": column,
        "parent": choices.parent,
        "element": choices.element,
        "in_start_tag": choices.in_start_tag,
        "elements": choices.elements.iter().map(|e| {
            let snippet = element_snippet(&grammar, &e.name, required_content);
            json!({ "name": e.name, "required": e.mandatory, "empty": e.empty, "text": snippet.text })
        }).collect::<Vec<_>>(),
        "attributes": choices.attributes.iter().map(|t| {
            json!({ "name": t.name, "required": t.mandatory, "used": t.used, "values": t.values })
        }).collect::<Vec<_>>(),
        "attribute_at": choices.attribute_at,
        "entities": choices.entities.iter().map(|e| json!({ "name": e.name, "value": e.value, "text": format!("&{};", e.name) })).collect::<Vec<_>>(),
        "lossy": source.lossy,
    }))
}
