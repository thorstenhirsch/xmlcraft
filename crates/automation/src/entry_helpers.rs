//! `xml_entry_helpers`: what the DTD of a document allows at a place (the elements that may come, the attributes of the element the place
//! is in, the entities that can be referred to), and the text each would put in. The DTD is read the way validation reads it: its external
//! parts are files read through the root confinement; nothing is downloaded. A document with no DOCTYPE that names an XML Schema
//! (`xsi:schemaLocation`, `xsi:noNamespaceSchemaLocation`, or else the schema that ships with the app for the namespace of its root element) is answered from the schema instead: the same lists, from the declarations
//! of the type of the element the place is in (and the values its text or an attribute may have).

use serde_json::{Value, json};
use xmlcraft_complete::{Choices, choices_at, element_snippet, xsd_choices_at, xsd_element_snippet};
use xmlcraft_dtd::doctype_parts;
use xmlcraft_engine::xsd_schemas::implicit_hints;
use xmlcraft_platform::subset::FileSubsetLoader;
use xmlcraft_syntax::{SyntaxNode, parse};
use xmlcraft_text::Buffer;
use xmlcraft_validate::{load_hinted, root_namespace, schema_hints};
use xmlcraft_xsd::ResolvingLoader;

use crate::documents::{fail, json_result, position_arg, position_json, source, source_and_position_properties};
use crate::tools::ToolDef;
use crate::validate::schema_resolver;
use crate::{Automation, Content, Result};

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "xml_entry_helpers",
        title: "What the DTD or schema allows here",
        description: "What the DTD (or, with no DOCTYPE, the XML Schema a document names) of a document allows at a position (an open document `id`, a file `path` or `text`; give the position as `line` and `column` from 1, \
                      or a byte `offset`): the elements that may come there, with `required` for those the content model cannot do without, the attributes of the \
                      element the position is in (`required`, `used` when the start tag has it, the values an enumeration lists), and the entities that can be \
                      referred to, each with the text a double click in Entry Helpers would put in. `required_content` (default true) says whether an element's \
                      text also has the children it requires. The DTD is the DOCTYPE's internal subset and the external parts that are files (a relative system \
                      identifier is taken from the folder of the document; nothing is downloaded). `kind` says \"dtd\" or \"xsd\": with an XML Schema names are written as the document writes them (with the prefix the namespace has there, `declare` giving the namespace \
                      declaration the text brings when it has none), attributes also say the `values` their type lists, and `values` says what the text of the \
                      element the position is in may be. A document with neither, or one that cannot be read, has nothing to offer: `grammar` is false and \
                      `why` says why.",
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
    let location = source.path.as_deref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    // A DOCTYPE is the document's own say; a schema is looked for only when there is none (as validation does).
    let Some(range) = doctype_parts(&root).map(|p| p.range) else {
        return xsd_answer(a, &root, &location, offset, required_content, &buffer, source.lossy);
    };
    let Some(doctype) = source.text.get(range.clone()) else { return nothing("the DOCTYPE is not text") };
    let confinement = a.confinement.clone();
    let mut loader = FileSubsetLoader::confined(move |path| confinement.resolve_read(path).map_err(|e| e.to_string()));
    let Some(grammar) = xmlcraft_engine::schema::read(doctype, range.start, &location, &mut loader) else {
        return nothing("the DOCTYPE is too large to be read");
    };
    let choices = choices_at(&root, offset, &grammar);
    let texts: Vec<String> = choices.elements.iter().map(|e| element_snippet(&grammar, &e.name, required_content).text).collect();
    answer("dtd", &choices, &texts, &buffer, offset, source.lossy)
}

/// The answer from the XML Schema the document names.
fn xsd_answer(
    a: &Automation,
    root: &SyntaxNode,
    location: &str,
    offset: usize,
    required_content: bool,
    buffer: &Buffer,
    lossy: bool,
) -> Result<Vec<Content>> {
    let nothing = |why: &str| json_result(json!({ "grammar": false, "why": why, "lossy": lossy }));
    // The schemas the document names; when it names none, the one known for the namespace of its root element (in the catalogs or
    // the bundle), which is what an XML Schema document, or any other of a namespace that ships with the app, is offered things from.
    let named = schema_hints(root);
    let implicit = named.is_empty();
    let hints = if implicit { root_namespace(root).map(|ns| implicit_hints(&ns)).unwrap_or_default() } else { named };
    if hints.is_empty() {
        return nothing("the document has no DOCTYPE and names no schema");
    }
    let resolver = schema_resolver(a);
    let mut loader = ResolvingLoader::new(&resolver);
    let loaded = match load_hinted(hints, location, &mut loader) {
        Ok(loaded) => loaded,
        // A schema the document did not name, and there is none for its namespace: the same as naming none.
        Err(_) if implicit => return nothing("the document has no DOCTYPE and names no schema, and none is known for its namespace"),
        Err(miss) => return nothing(&miss.why),
    };
    let schema = &loaded.assembly.schema;
    let choices = xsd_choices_at(root, offset, schema);
    let texts: Vec<String> = choices.elements.iter().map(|e| xsd_element_snippet(schema, &choices.scope, e, required_content).text).collect();
    answer("xsd", &choices, &texts, buffer, offset, lossy)
}

/// A namespace declaration a name brings, as JSON.
fn declaration_json(declare: &Option<xmlcraft_complete::Declaration>) -> Value {
    match declare {
        Some((prefix, namespace)) => json!({ "prefix": prefix, "namespace": namespace }),
        None => Value::Null,
    }
}

/// The JSON of what a grammar allows at a place: `texts` are what putting each of the elements in would write.
fn answer(kind: &str, choices: &Choices, texts: &[String], buffer: &Buffer, offset: usize, lossy: bool) -> Result<Vec<Content>> {
    let (line, column) = position_json(buffer, offset);
    json_result(json!({
        "grammar": true,
        "kind": kind,
        "line": line,
        "column": column,
        "parent": choices.parent,
        "element": choices.element,
        "in_start_tag": choices.in_start_tag,
        "elements": choices.elements.iter().zip(texts).map(|(e, text)| {
            json!({ "name": e.name, "required": e.mandatory, "empty": e.empty, "text": text, "declare": declaration_json(&e.declare) })
        }).collect::<Vec<_>>(),
        "attributes": choices.attributes.iter().map(|t| {
            json!({ "name": t.name, "required": t.mandatory, "used": t.used, "values": t.values, "declare": declaration_json(&t.declare) })
        }).collect::<Vec<_>>(),
        "attribute_at": choices.attribute_at,
        "values": choices.values,
        "entities": choices.entities.iter().map(|e| json!({ "name": e.name, "value": e.value, "text": format!("&{};", e.name) })).collect::<Vec<_>>(),
        "lossy": lossy,
    }))
}
