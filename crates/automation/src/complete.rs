//! `xml_complete`: what the document itself can offer at a place (element, end-tag, attribute, value and entity names it already
//! uses), with the text the popup would write. A grammar's answer is `xml_entry_helpers`.

use serde_json::{Value, json};
use xmlcraft_complete::{attribute_snippet, entity_snippet};
use xmlcraft_engine::analysis::MAX_ANALYSED_BYTES;
use xmlcraft_engine::language::Language;
use xmlcraft_engine::xml_complete::{DocumentChoices, Kind, context, document_choices};
use xmlcraft_text::Buffer;

use crate::documents::{fail, json_result, language_source_properties, position_arg, position_json, source};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result};

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut properties = language_source_properties();
    if let Some(map) = properties.as_object_mut() {
        map.insert("line".into(), json!({ "type": "integer", "minimum": 1 }));
        map.insert("column".into(), json!({ "type": "integer", "minimum": 1 }));
        map.insert(
            "offset".into(),
            json!({ "type": "integer", "minimum": 0, "description": "A byte offset of the text, instead of line and column" }),
        );
        map.insert(
            "explicit".into(),
            json!({ "type": "boolean", "description": "Treat the call as a request for completion (an element in content, an attribute after a space)" }),
        );
    }
    vec![ToolDef {
        name: "xml_complete",
        title: "What the document itself offers here",
        description: "What the names in an XML document offer at a position (an open document `id`, a file `path` or `text`; the position as \
                      `line` and `column` from 1, or a byte `offset`): after `<` the element names the document uses, after `</` the names an \
                      end tag can close, after a space in a start tag the attribute names it uses, inside an attribute value the values that \
                      attribute has, and after `&` the entities it refers to. Each item has the `label`, the `insert` text, the `cursor` in it, \
                      a `detail` and the part of the text it replaces (`start`, `end`); `kind` says which. With `explicit` the popup is asked \
                      for where nothing was typed. This answer comes from the document alone; what a DTD or XML Schema allows is \
                      `xml_entry_helpers`.",
        input_schema: json!({ "type": "object", "properties": properties, "additionalProperties": false }),
        read_only: true,
        destructive: false,
        handler: xml_complete,
    }]
}

struct Item {
    label: String,
    insert: String,
    cursor: usize,
    detail: String,
}

fn xml_complete(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let src = source(a, args)?;
    if src.text.len() > MAX_ANALYSED_BYTES {
        return fail("the text is too large to analyse");
    }
    if src.language != Language::Xml {
        return fail("this reads XML documents: give language xml for text, or open an XML document");
    }
    let buffer = Buffer::from_text(&src.text);
    let offset = position_arg(&src.text, &buffer, args)?;
    let explicit = args.get("explicit").and_then(Value::as_bool).unwrap_or(false);
    let choices = document_choices(&src.text);
    let (line, column) = position_json(&buffer, offset);
    let Some(found) = context(&src.text, offset, explicit) else {
        return json_result(json!({
            "line": line, "column": column, "offset": offset, "kind": "", "partial": "",
            "start": offset, "end": offset, "count": 0, "items": [], "lossy": src.lossy,
        }));
    };
    let partial = found.partial;
    let mut items = items(&found.kind, &choices);
    if matches!(found.kind, Kind::Attribute) {
        items.retain(|item| item.label != partial);
    }
    let items = narrowed(items, &partial);
    json_result(json!({
        "line": line,
        "column": column,
        "offset": offset,
        "kind": kind_name(&found.kind),
        "partial": partial,
        "start": found.start,
        "end": offset,
        "count": items.len(),
        "items": items.iter().map(|i| json!({
            "label": i.label, "insert": i.insert, "cursor": i.cursor, "detail": i.detail,
        })).collect::<Vec<_>>(),
        "lossy": src.lossy,
    }))
}

fn kind_name(kind: &Kind) -> &'static str {
    match kind {
        Kind::Element => "element",
        Kind::EndTag => "end-tag",
        Kind::Attribute => "attribute",
        Kind::Value(_) => "value",
        Kind::Entity => "entity",
    }
}

fn items(kind: &Kind, choices: &DocumentChoices) -> Vec<Item> {
    match kind {
        Kind::Element => choices
            .elements
            .iter()
            .map(|name| Item {
                label: name.clone(),
                insert: format!("<{name}></{name}>"),
                cursor: name.len() + 2,
                detail: "used in this document".to_owned(),
            })
            .collect(),
        Kind::EndTag => choices
            .elements
            .iter()
            .map(|name| Item { label: name.clone(), insert: format!("{name}>"), cursor: name.len() + 1, detail: "used in this document".to_owned() })
            .collect(),
        Kind::Attribute => choices
            .attributes
            .iter()
            .map(|name| {
                let snippet = attribute_snippet(name, None);
                Item {
                    label: name.clone(),
                    insert: snippet.text.trim_start().to_owned(),
                    cursor: snippet.caret.saturating_sub(1),
                    detail: "used in this document".to_owned(),
                }
            })
            .collect(),
        Kind::Value(name) => choices
            .values
            .get(name)
            .into_iter()
            .flatten()
            .map(|value| Item { label: value.clone(), insert: value.clone(), cursor: value.len(), detail: "used in this document".to_owned() })
            .collect(),
        Kind::Entity => choices
            .entities
            .iter()
            .map(|name| {
                let snippet = entity_snippet(name);
                Item { label: format!("&{name};"), insert: snippet.text, cursor: snippet.caret, detail: "used in this document".to_owned() }
            })
            .collect(),
    }
}

fn narrowed(items: Vec<Item>, partial: &str) -> Vec<Item> {
    if partial.is_empty() {
        return items;
    }
    let p = partial.to_lowercase();
    let (mut starts, mut contains) = (Vec::new(), Vec::new());
    for item in items {
        let label = item.label.trim_start_matches(['<', '&']).to_lowercase();
        if label.starts_with(&p) {
            starts.push(item);
        } else if label.contains(&p) {
            contains.push(item);
        }
    }
    starts.extend(contains);
    starts
}
