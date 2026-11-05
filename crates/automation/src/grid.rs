//! `grid_rows` and `grid_edit`: a document of XML, JSON or YAML as a tree table, and its cells changed.
//!
//! The rows come from an open document (`id`), a file (`path`, through the root confinement) or `text`. An edit changes an open document
//! with the smallest edits of its text (one undoable step): a value replaced, a name replaced, a member removed with its comma.

use std::collections::HashSet;

use serde_json::{Value, json};
use xmlcraft_engine::compare::family_of;
use xmlcraft_grid::Grid;
use xmlcraft_text::Buffer;

use crate::documents::{describe, fail, id_arg, index_of, json_result, language_source_properties, position_json, source};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

/// The most rows one answer lists (the count is always the real one).
const MAX_LISTED: usize = 2000;

fn path_property() -> Value {
    json!({ "type": "array", "items": { "type": "integer", "minimum": 0 }, "description": "The row: the index of the child at each step down from the top (as `path` of grid_rows)" })
}

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut rows = language_source_properties();
    if let Some(map) = rows.as_object_mut() {
        map.insert("expand".into(), json!({ "description": "\"all\" to open every row, \"none\" (default) to list the top rows only, or an array of paths to open", "oneOf": [{ "type": "string", "enum": ["all", "none"] }, { "type": "array", "items": path_property() }] }));
        map.insert(
            "max_rows".into(),
            json!({ "type": "integer", "minimum": 1, "maximum": MAX_LISTED, "description": "The most rows listed (default 500)" }),
        );
    }
    vec![
        ToolDef {
            name: "grid_rows",
            title: "List the rows of the grid",
            description: "List the rows of the grid of an XML, JSON or YAML document: each with its `path` (indexes from the top), `depth`, `name` (an element or attribute name, a key, \
                          `[0]` for an array item, `#text`, `#comment`), `type`, `value` (an element that holds only text shows it; a container shows `{3}` or `[2]`), \
                          `children`, whether it is `editable`, and `line` and `column`. An element with only text has no row for the text. `expand` says which rows are open.",
            input_schema: json!({ "type": "object", "properties": rows, "additionalProperties": false }),
            read_only: true,
            destructive: false,
            handler: grid_rows,
        },
        ToolDef {
            name: "grid_edit",
            title: "Change a cell of the grid",
            description: "Change a row of an open document, as one undoable step with the smallest edits of its text: `set_value` (the value of an attribute, text, comment, an element with only \
                          text, a JSON or YAML scalar: written as the type needs, escaped, quoted where YAML needs it), `set_literal` (a JSON scalar of another type: 12, true, null, \
                          \"text\"), `rename` (an element, attribute or key) or `remove` (a row, with its comma or line). What cannot be done (a number that is not a number, a name \
                          that is no name, a block scalar) is an error that says why and nothing is changed.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "minimum": 0, "description": "An open document" },
                    "action": { "type": "string", "enum": ["set_value", "set_literal", "rename", "remove"] },
                    "path": path_property(),
                    "value": { "type": "string", "description": "The new value, literal or name (not for remove)" }
                },
                "required": ["id", "action", "path"],
                "additionalProperties": false
            }),
            read_only: false,
            destructive: false,
            handler: grid_edit,
        },
    ]
}

fn paths_of(value: &Value) -> Result<Vec<usize>> {
    value
        .as_array()
        .ok_or_else(|| ToolError::InvalidArgs("a path is an array of whole numbers".into()))?
        .iter()
        .map(|v| v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| ToolError::InvalidArgs("a path is an array of whole numbers".into())))
        .collect()
}

fn grid_rows(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let s = source(a, args)?;
    let grid = Grid::read(&s.text, family_of(s.language)).map_err(|e| ToolError::Failed(e.to_string()))?;
    let expanded: HashSet<Vec<usize>> = match args.get("expand") {
        None => HashSet::new(),
        Some(Value::String(w)) if w == "none" => HashSet::new(),
        Some(Value::String(w)) if w == "all" => grid.expandable().into_iter().collect(),
        Some(Value::Array(paths)) => paths.iter().map(paths_of_value).collect::<Result<_>>()?,
        Some(_) => return Err(ToolError::InvalidArgs("expand is \"all\", \"none\" or an array of paths".into())),
    };
    let listed = match args.get("max_rows") {
        None => 500,
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| (1..=MAX_LISTED).contains(n))
            .ok_or_else(|| ToolError::InvalidArgs(format!("max_rows is a whole number from 1 to {MAX_LISTED}")))?,
    };
    let rows = grid.rows(&expanded);
    let buffer = Buffer::from_text(&s.text);
    let items: Vec<Value> = rows
        .iter()
        .take(listed)
        .map(|r| {
            let (line, column) = position_json(&buffer, r.span.start.min(buffer.len()));
            json!({ "path": r.path, "depth": r.depth, "name": r.name, "type": r.kind.label(), "value": r.value, "children": r.child_count, "editable": r.editable, "line": line, "column": column })
        })
        .collect();
    json_result(json!({ "count": rows.len(), "truncated": rows.len() > items.len(), "rows": items }))
}

fn paths_of_value(v: &Value) -> Result<Vec<usize>> {
    paths_of(v)
}

fn grid_edit(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let index = index_of(a, id_arg(args)?)?;
    let action = args.get("action").and_then(Value::as_str).ok_or_else(|| ToolError::InvalidArgs("`action` is required".into()))?;
    let path = paths_of(args.get("path").ok_or_else(|| ToolError::InvalidArgs("`path` is required".into()))?)?;
    let value = match args.get("value") {
        None => None,
        Some(Value::String(v)) => Some(v.as_str()),
        Some(_) => return Err(ToolError::InvalidArgs("value is a string".into())),
    };
    let need = |what: &str| value.ok_or_else(|| ToolError::InvalidArgs(format!("{what} needs `value`")));
    let Some(doc) = a.session.document_mut(index) else { return fail("the document disappeared") };
    let text = doc.editor().text();
    let grid = Grid::read(&text, family_of(doc.language())).map_err(|e| ToolError::Failed(e.to_string()))?;
    let edits = match action {
        "set_value" => grid.set_value(&text, &path, need("set_value")?),
        "set_literal" => grid.set_literal(&text, &path, need("set_literal")?),
        "rename" => grid.rename(&text, &path, need("rename")?),
        "remove" => grid.remove(&text, &path),
        other => return Err(ToolError::InvalidArgs(format!("action is set_value, set_literal, rename or remove, not {other:?}"))),
    }
    .map_err(|e| ToolError::Failed(e.to_string()))?;
    let changed = doc.editor_mut().apply_edits(&edits);
    json_result(json!({ "changed": changed, "edits": edits.len(), "document": describe(doc) }))
}
