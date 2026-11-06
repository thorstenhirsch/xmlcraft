//! `xslt_complete`: what may be written at a place of an XSLT stylesheet: the `xsl:` elements that fit where the caret is, the attributes of
//! an element, the values an attribute lists or the names the stylesheet defines, and the XPath functions in an expression.

use serde_json::{Value, json};
use xmlcraft_engine::analysis::MAX_ANALYSED_BYTES;
use xmlcraft_engine::language::Language;
use xmlcraft_engine::xslt_complete::{XsltKind, complete};
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
            json!({ "type": "boolean", "description": "Treat the call as a request for completion (functions are listed before anything is typed)" }),
        );
    }
    vec![ToolDef {
        name: "xslt_complete",
        title: "What may be written here in an XSLT stylesheet",
        description: "What may be written at a position of an XSLT stylesheet (an open document `id`, a file `path`, or `text`; the position as `line` and `column` from 1, or a byte `offset`). \
                      After `<`: the `xsl:` elements that may stand in the element the caret is in (declarations at the top, instructions in a template, `when` in `choose` …); in a start \
                      tag: the attributes the element takes; inside the quotes: the values an attribute lists, or the names the stylesheet defines (`call-template`, `mode`, \
                      `use-attribute-sets`); in an XPath attribute (`select`, `test`, `match` …): function names. Each item has the `label`, the `insert` text, the `cursor` in it, a `detail` \
                      and the part of the text it replaces (`start`, `end`); `kind` says which. `stylesheet` is false when the document does not bind the XSLT namespace on its root.",
        input_schema: json!({ "type": "object", "properties": properties, "additionalProperties": false }),
        read_only: true,
        destructive: false,
        handler: xslt_complete,
    }]
}

fn xslt_complete(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let src = source(a, args)?;
    if src.text.len() > MAX_ANALYSED_BYTES {
        return fail("the text is too large to analyse");
    }
    if src.language != Language::Xml {
        return fail("this reads XSLT stylesheets, which are XML: give language xml for text");
    }
    let buffer = Buffer::from_text(&src.text);
    let offset = position_arg(&src.text, &buffer, args)?;
    let explicit = args.get("explicit").and_then(Value::as_bool).unwrap_or(false);
    let stylesheet = xmlcraft_engine::xslt_complete::xslt_prefix(&src.text).is_some();
    let found = complete(&src.text, offset, explicit);
    let (line, column) = position_json(&buffer, offset);
    let (kind, start, end, items) = match &found {
        Some(c) => (
            match c.kind {
                XsltKind::Element => "element",
                XsltKind::Attribute => "attribute",
                XsltKind::Value => "value",
                XsltKind::Function => "function",
                XsltKind::Variable => "variable",
            },
            c.replace.start,
            c.replace.end,
            c.items.iter().map(|i| json!({ "label": i.label, "insert": i.insert, "cursor": i.cursor, "detail": i.detail })).collect::<Vec<_>>(),
        ),
        None => ("", offset, offset, Vec::new()),
    };
    json_result(json!({
        "line": line, "column": column, "offset": offset, "stylesheet": stylesheet,
        "kind": kind, "start": start, "end": end, "count": items.len(), "items": items, "lossy": src.lossy,
    }))
}
