//! `grid_rows` and `grid_edit`: a document of XML, JSON or YAML as a tree table, and its cells changed. `grid_table` and
//! `grid_table_edit`: the children of a node that repeat as a table of rows and columns, and its cells, rows and columns changed.
//!
//! The rows come from an open document (`id`), a file (`path`, through the root confinement) or `text`. An edit changes an open document
//! with the smallest edits of its text (one undoable step): a value replaced, a name replaced, a member removed with its comma.

use std::collections::HashSet;

use serde_json::{Value, json};
use xmlcraft_engine::grid::read_grid;
use xmlcraft_grid::{CellKind, ColumnKind, Grid, Layout, Table};
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
        ToolDef {
            name: "grid_table",
            title: "Show repeated items as a table",
            description: "Show the children of a node that repeat as a table: XML elements of one name (columns `@attribute`, child element names, `#text`), the items of a JSON or \
                          YAML array of objects (the union of their keys) or of scalars (one `value` column), JSON Lines (the document itself), a BSON file's Extended JSON \
                          (`{\"$numberLong\": \"5\"}` is the value 5). `at` names the node: a path of indexes (as grid_rows), a pointer (`/items/0` for JSON and YAML, \
                          `/library` or `/a/b[2]` for XML) or a byte offset (the deepest table around it); the top by default. Each cell has a `kind` (scalar, nested, \
                          missing) and its `text` (`{3}`, `[5]`, `<2>` for nested ones, with the `drill` path to open). `sort` and `filter` are views: the text is not \
                          changed (grid_table_edit apply_order writes an order).",
            input_schema: json!({ "type": "object", "properties": table_properties(true), "additionalProperties": false }),
            read_only: true,
            destructive: false,
            handler: grid_table,
        },
        ToolDef {
            name: "grid_table_edit",
            title: "Change a table of the grid",
            description: "Change a table of repeated items (see grid_table) with the smallest edits of the text, in its layout (indentation, line endings, JSON commas, YAML \
                          indentation, XML escaping): an open document (`id`) changes as one undoable step; for `text` (or a file named by `path`, which is not \
                          written) the new text is returned. `op`: `set_cell` (`row`, `column`, `value`; a missing cell is created after the column before it that the \
                          row has, and a column that is not in the table yet is added; a data value keeps its type when it reads as one, `literal: true` writes it as \
                          it is), `clear_cell`, `insert_row` (`after`: a row, or none for the top; the neighbour's shape), `duplicate_row` (`row`), `delete_rows` \
                          (`rows`), `move_row` (`row`, `before`: a row or none for the end), `rename_column` (`column`, `new_name`), `delete_column` (`column`), \
                          `apply_order` (`order`: every row once, or `sort`), `paste_tsv` (`row`, `column`, `tsv`; rows are added at the end as needed). Rows are \
                          counted from 0 in the table's order; a column is its label (`@id`, `title`) or index.",
            input_schema: json!({ "type": "object", "properties": table_edit_properties(), "required": ["op"], "additionalProperties": false }),
            read_only: false,
            destructive: false,
            handler: grid_table_edit,
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
    let grid = read_grid(&s.text, s.language).map_err(|e| ToolError::Failed(e.to_string()))?;
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
    let grid = read_grid(&text, doc.language()).map_err(|e| ToolError::Failed(e.to_string()))?;
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

/// The most rows grid_table lists.
const MAX_TABLE_ROWS: usize = 2000;

fn column_property() -> Value {
    json!({ "description": "A column: its label (`@id`, `title`, `value`) or its index from 0", "oneOf": [{ "type": "string" }, { "type": "integer", "minimum": 0 }] })
}

fn table_properties(views: bool) -> Value {
    let mut p = language_source_properties();
    if let Some(map) = p.as_object_mut() {
        map.insert("at".into(), json!({ "description": "The node whose children are the rows: a path of indexes (as grid_rows), a pointer (`/items`, `/library/book[2]`), or a byte offset in it", "oneOf": [path_property(), { "type": "string" }, { "type": "integer", "minimum": 0 }] }));
        map.insert("name".into(), json!({ "type": "string", "description": "XML: the name of the elements that are the rows (default: the name most children have, seen at least twice)" }));
        map.insert("sort".into(), json!({ "type": "object", "properties": { "column": column_property(), "descending": { "type": "boolean" } }, "required": ["column"], "additionalProperties": false, "description": "Sort by a column: numbers by value, text as text, empty cells last" }));
        if views {
            map.insert("filter".into(), json!({ "type": "string", "description": "Only the rows with this text in a cell (any case)" }));
            map.insert(
                "max_rows".into(),
                json!({ "type": "integer", "minimum": 1, "maximum": MAX_TABLE_ROWS, "description": "The most rows listed (default 500)" }),
            );
        }
    }
    p
}

fn table_edit_properties() -> Value {
    let mut p = table_properties(false);
    if let Some(map) = p.as_object_mut() {
        map.insert("op".into(), json!({ "type": "string", "enum": ["set_cell", "clear_cell", "insert_row", "duplicate_row", "delete_rows", "move_row", "rename_column", "delete_column", "apply_order", "paste_tsv"] }));
        map.insert("row".into(), json!({ "type": "integer", "minimum": 0 }));
        map.insert("rows".into(), json!({ "type": "array", "items": { "type": "integer", "minimum": 0 } }));
        map.insert("column".into(), column_property());
        map.insert("value".into(), json!({ "type": "string", "description": "set_cell: the value as typed (or written as it is, with literal)" }));
        map.insert(
            "literal".into(),
            json!({ "type": "boolean", "description": "set_cell: write `value` as it is (`12`, `true`, `null`, `\"text\"`): JSON and YAML" }),
        );
        map.insert("after".into(), json!({ "description": "insert_row: the row the new one follows (none: the top)", "oneOf": [{ "type": "integer", "minimum": 0 }, { "type": "null" }] }));
        map.insert("before".into(), json!({ "description": "move_row: the row it goes before (none: the end)", "oneOf": [{ "type": "integer", "minimum": 0 }, { "type": "null" }] }));
        map.insert("order".into(), json!({ "type": "array", "items": { "type": "integer", "minimum": 0 }, "description": "apply_order: the rows in their new order, every row once" }));
        map.insert(
            "tsv".into(),
            json!({ "type": "string", "description": "paste_tsv: tab-separated text (cells with tabs or line breaks in quotes)" }),
        );
        map.insert("new_name".into(), json!({ "type": "string", "description": "rename_column: the new name" }));
    }
    p
}

fn usize_of(v: &Value, what: &str) -> Result<usize> {
    v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| ToolError::InvalidArgs(format!("{what} is a whole number from 0")))
}

/// The table `at` names in `text`.
fn table_of(grid: &Grid, text: &str, args: &Value) -> Result<Table> {
    let name = match args.get("name") {
        None => None,
        Some(Value::String(n)) => Some(n.as_str()),
        Some(_) => return Err(ToolError::InvalidArgs("name is a string".into())),
    };
    let path = match args.get("at") {
        None => Vec::new(),
        Some(v @ Value::Array(_)) => paths_of(v)?,
        Some(Value::String(pointer)) => grid.path_of_pointer(pointer).map_err(|e| ToolError::Failed(e.to_string()))?,
        Some(v @ Value::Number(_)) => {
            let offset = usize_of(v, "at")?;
            grid.table_path_at(text, offset).ok_or_else(|| ToolError::Failed(format!("no table holds byte {offset}")))?
        }
        Some(_) => return Err(ToolError::InvalidArgs("at is a path of indexes, a pointer or a byte offset".into())),
    };
    grid.table_named(text, &path, name).ok_or_else(|| {
        ToolError::Failed(format!(
            "the node at {path:?} holds no table: its children do not repeat (XML elements of one name, an array of objects or of values); name one with `at`"
        ))
    })
}

fn column_of(table: &Table, v: &Value) -> Result<usize> {
    match v {
        Value::String(label) => table.column_index(label).ok_or_else(|| {
            let labels: Vec<String> = table.columns.iter().map(|c| c.label()).collect();
            ToolError::Failed(format!("no column {label:?}; the columns are {}", labels.join(", ")))
        }),
        v => usize_of(v, "column").and_then(|c| if c < table.columns.len() { Ok(c) } else { fail(format!("there is no column {c}")) }),
    }
}

/// The rows in the order `sort` asks for (of `rows`).
fn sort_of(table: &Table, args: &Value, rows: &[usize]) -> Result<Option<Vec<usize>>> {
    let Some(sort) = args.get("sort") else { return Ok(None) };
    let column = column_of(table, sort.get("column").ok_or_else(|| ToolError::InvalidArgs("sort needs a column".into()))?)?;
    let descending = sort.get("descending").and_then(Value::as_bool).unwrap_or(false);
    Ok(Some(table.sorted_rows(rows, column, !descending)))
}

fn kind_name(kind: CellKind) -> &'static str {
    match kind {
        CellKind::Scalar => "scalar",
        CellKind::Nested => "nested",
        CellKind::Missing => "missing",
    }
}

fn column_kind_name(kind: ColumnKind) -> &'static str {
    match kind {
        ColumnKind::Attribute => "attribute",
        ColumnKind::Element => "element",
        ColumnKind::Text => "text",
        ColumnKind::Member => "member",
        ColumnKind::Value => "value",
    }
}

fn layout_name(layout: Layout) -> &'static str {
    match layout {
        Layout::XmlElements => "xml elements",
        Layout::JsonArray => "json array",
        Layout::JsonLines => "json lines",
        Layout::YamlBlock => "yaml block sequence",
        Layout::YamlFlow => "yaml flow sequence",
        Layout::YamlDocuments => "yaml documents",
    }
}

fn grid_table(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let s = source(a, args)?;
    let grid = read_grid(&s.text, s.language).map_err(|e| ToolError::Failed(e.to_string()))?;
    let table = table_of(&grid, &s.text, args)?;
    let listed = match args.get("max_rows") {
        None => 500,
        Some(v) => usize_of(v, "max_rows")
            .ok()
            .filter(|n| (1..=MAX_TABLE_ROWS).contains(n))
            .ok_or_else(|| ToolError::InvalidArgs(format!("max_rows is a whole number from 1 to {MAX_TABLE_ROWS}")))?,
    };
    let filter = match args.get("filter") {
        None => None,
        Some(Value::String(f)) => Some(f.as_str()),
        Some(_) => return Err(ToolError::InvalidArgs("filter is a string".into())),
    };
    let shown = filter.map_or_else(|| (0..table.rows.len()).collect(), |f| table.filtered(f));
    let order = sort_of(&table, args, &shown)?.unwrap_or(shown);
    let buffer = Buffer::from_text(&s.text);
    let rows: Vec<Value> = order
        .iter()
        .take(listed)
        .filter_map(|&r| table.rows.get(r).map(|row| (r, row)))
        .map(|(r, row)| {
            let (line, column) = position_json(&buffer, row.span.start.min(buffer.len()));
            let cells: Vec<Value> = row
                .cells
                .iter()
                .map(|c| match c.kind {
                    CellKind::Missing => json!({ "kind": kind_name(c.kind) }),
                    CellKind::Scalar => json!({ "kind": kind_name(c.kind), "text": c.text, "type": c.value_kind.label() }),
                    CellKind::Nested => json!({
                        "kind": kind_name(c.kind),
                        "text": c.text,
                        "count": c.count,
                        "drill": c.drill.as_ref().map(|d| json!({ "path": d.path, "name": d.name })),
                    }),
                })
                .collect();
            json!({ "row": r, "path": row.path, "line": line, "column": column, "cells": cells })
        })
        .collect();
    let columns: Vec<Value> = table
        .columns
        .iter()
        .map(|c| json!({ "label": c.label(), "name": c.name, "kind": column_kind_name(c.kind), "numeric": c.numeric, "type": c.value_kind.map(|k| k.label()) }))
        .collect();
    json_result(json!({
        "path": table.path,
        "item_name": table.item_name,
        "layout": layout_name(table.layout),
        "columns": columns,
        "hidden_columns": table.hidden_columns,
        "count": table.total_rows,
        "cut": !table.is_whole(),
        "shown": order.len(),
        "truncated": order.len() > rows.len(),
        "rows": rows,
    }))
}

fn grid_table_edit(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let op = args.get("op").and_then(Value::as_str).ok_or_else(|| ToolError::InvalidArgs("`op` is required".into()))?;
    let s = source(a, args)?;
    let grid = read_grid(&s.text, s.language).map_err(|e| ToolError::Failed(e.to_string()))?;
    let mut table = table_of(&grid, &s.text, args)?;
    let text = s.text.as_str();
    let row = || usize_of(args.get("row").ok_or_else(|| ToolError::InvalidArgs(format!("{op} needs `row`")))?, "row");
    let optional_row = |key: &str| match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => usize_of(v, key).map(Some),
    };
    let string = |key: &str| args.get(key).and_then(Value::as_str).ok_or_else(|| ToolError::InvalidArgs(format!("{op} needs `{key}` (a string)")));
    let column_arg = || args.get("column").ok_or_else(|| ToolError::InvalidArgs(format!("{op} needs `column`")));
    let failed = |e: xmlcraft_grid::GridError| ToolError::Failed(e.to_string());
    let mut report = json!({});
    let edits = match op {
        "set_cell" => {
            let r = row()?;
            let value = string("value")?;
            let wanted = column_arg()?;
            let c = match (wanted, column_of(&table, wanted)) {
                (_, Ok(c)) => c,
                // A column the table does not have yet: added at the end, written by this cell.
                (Value::String(label), Err(_)) => {
                    table = grid.insert_column(text, &table, label, table.columns.len().checked_sub(1)).map_err(failed)?;
                    table.column_index(label).ok_or_else(|| ToolError::Failed("the new column is not there".into()))?
                }
                (_, Err(e)) => return Err(e),
            };
            if args.get("literal").and_then(Value::as_bool).unwrap_or(false) {
                grid.set_cell_literal(text, &table, r, c, value)
            } else {
                grid.set_cell(text, &table, r, c, value)
            }
        }
        "clear_cell" => grid.clear_cell(text, &table, row()?, column_of(&table, column_arg()?)?),
        "insert_row" => grid.insert_row(text, &table, optional_row("after")?),
        "duplicate_row" => grid.duplicate_row(text, &table, row()?),
        "delete_rows" => {
            let rows = args.get("rows").and_then(Value::as_array).ok_or_else(|| ToolError::InvalidArgs("delete_rows needs `rows`".into()))?;
            let rows = rows.iter().map(|v| usize_of(v, "a row")).collect::<Result<Vec<_>>>()?;
            if rows.is_empty() {
                return Err(ToolError::InvalidArgs("delete_rows needs at least one row".into()));
            }
            grid.delete_rows(text, &table, &rows)
        }
        "move_row" => grid.move_row(text, &table, row()?, optional_row("before")?),
        "rename_column" => grid.rename_column(text, &table, column_of(&table, column_arg()?)?, string("new_name")?),
        "delete_column" => grid.delete_column(text, &table, column_of(&table, column_arg()?)?),
        "apply_order" => {
            let all: Vec<usize> = (0..table.rows.len()).collect();
            let order = match (args.get("order"), sort_of(&table, args, &all)?) {
                (Some(order), None) => order
                    .as_array()
                    .ok_or_else(|| ToolError::InvalidArgs("order is an array of rows".into()))?
                    .iter()
                    .map(|v| usize_of(v, "a row"))
                    .collect::<Result<Vec<_>>>()?,
                (None, Some(sorted)) => sorted,
                _ => return Err(ToolError::InvalidArgs("apply_order needs `order` or `sort` (one of them)".into())),
            };
            grid.apply_order(text, &table, &order)
        }
        "paste_tsv" => {
            let pasted = grid.paste_tsv(text, &table, row()?, column_of(&table, column_arg()?)?, string("tsv")?).map_err(failed)?;
            report = json!({ "cells": pasted.cells, "skipped": pasted.skipped, "appended": pasted.appended });
            Ok(pasted.edits)
        }
        other => {
            return Err(ToolError::InvalidArgs(format!(
                "op is set_cell, clear_cell, insert_row, duplicate_row, delete_rows, move_row, rename_column, delete_column, apply_order or paste_tsv, not {other:?}"
            )));
        }
    }
    .map_err(failed)?;
    let mut out = match s.id {
        Some(id) => {
            let index = index_of(a, id)?;
            let Some(doc) = a.session.document_mut(index) else { return fail("the document disappeared") };
            // (The edits were made on the document's text as it is: nothing ran in between.)
            let changed = doc.editor_mut().apply_edits(&edits);
            json!({ "changed": changed, "edits": edits.len(), "document": describe(doc) })
        }
        None => json!({ "changed": !edits.is_empty(), "edits": edits.len(), "text": xmlcraft_text::apply_edits(text, &edits) }),
    };
    if let (Some(map), Some(extra)) = (out.as_object_mut(), report.as_object()) {
        map.extend(extra.clone());
    }
    json_result(out)
}
