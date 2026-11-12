//! `grid_rows` and `grid_edit`, through the tool table.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, tool: &str, args: Value) -> Result<Value, ToolError> {
    a.call(tool, &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

const JSON: &str = "{\n  \"name\": \"Tea\",\n  \"qty\": 2,\n  \"tags\": [\"a\", \"b\"]\n}";

fn open(a: &mut Automation, dir: &std::path::Path, name: &str, text: &str) -> u64 {
    std::fs::write(dir.join(name), text).expect("writes");
    call(a, "doc_open", json!({ "path": name })).expect("opens")["id"].as_u64().expect("id")
}

#[test]
fn rows_are_listed_with_paths_values_and_places_and_open_as_asked() {
    let mut a = Automation::new();
    let closed = call(&mut a, "grid_rows", json!({ "text": JSON, "language": "json" })).expect("runs");
    assert_eq!(closed["count"].as_u64(), Some(3), "{closed}");
    assert_eq!(
        (closed["rows"][2]["name"].as_str(), closed["rows"][2]["value"].as_str(), closed["rows"][2]["children"].as_u64()),
        (Some("tags"), Some("[2]"), Some(2))
    );
    let open_all = call(&mut a, "grid_rows", json!({ "text": JSON, "language": "json", "expand": "all" })).expect("runs");
    assert_eq!(open_all["count"].as_u64(), Some(5));
    let item = &open_all["rows"][3];
    assert_eq!(
        (item["name"].as_str(), item["path"].clone(), item["depth"].as_u64(), item["line"].as_u64()),
        (Some("[0]"), json!([2, 0]), Some(1), Some(4))
    );
    let some = call(&mut a, "grid_rows", json!({ "text": JSON, "language": "json", "expand": [[2]] })).expect("runs");
    assert_eq!(some["count"].as_u64(), Some(5));
    let xml = call(&mut a, "grid_rows", json!({ "text": "<a n=\"1\"><b>x</b></a>", "expand": "all" })).expect("runs");
    assert_eq!(xml["rows"].as_array().map(Vec::len), Some(3), "the text of <b> is in the row of b: {xml}");
    assert!(matches!(call(&mut a, "grid_rows", json!({ "text": JSON, "language": "json", "expand": "some" })), Err(ToolError::InvalidArgs(_))));
    assert!(call(&mut a, "grid_rows", json!({ "text": "{", "language": "json" })).is_err());
}

#[test]
fn an_edit_changes_the_open_document_with_the_smallest_edit_and_one_undo_takes_it_back() {
    let dir = tempfile::tempdir().expect("dir");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let id = open(&mut a, dir.path(), "d.json", JSON);
    call(&mut a, "grid_edit", json!({ "id": id, "action": "set_value", "path": [1], "value": "5" })).expect("edits");
    assert_eq!(
        call(&mut a, "doc_text", json!({ "id": id })).expect("text")["text"].as_str(),
        Some(JSON.replace("\"qty\": 2", "\"qty\": 5").as_str())
    );
    call(&mut a, "grid_edit", json!({ "id": id, "action": "rename", "path": [0], "value": "title" })).expect("edits");
    call(&mut a, "grid_edit", json!({ "id": id, "action": "remove", "path": [2, 0] })).expect("edits");
    let text = call(&mut a, "doc_text", json!({ "id": id })).expect("text")["text"].as_str().expect("text").to_owned();
    assert_eq!(text, "{\n  \"title\": \"Tea\",\n  \"qty\": 5,\n  \"tags\": [\"b\"]\n}");
    call(&mut a, "grid_edit", json!({ "id": id, "action": "set_literal", "path": [1], "value": "null" })).expect("edits");
    assert!(call(&mut a, "doc_text", json!({ "id": id })).expect("text")["text"].as_str().is_some_and(|t| t.contains("\"qty\": null")));
    call(&mut a, "doc_undo", json!({ "id": id })).expect("undo");
    assert!(
        call(&mut a, "doc_text", json!({ "id": id })).expect("text")["text"].as_str().is_some_and(|t| t.contains("\"qty\": 5")),
        "one undo is one edit"
    );
}

#[test]
fn what_cannot_be_done_is_an_error_that_says_why_and_leaves_the_text_alone() {
    let dir = tempfile::tempdir().expect("dir");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let id = open(&mut a, dir.path(), "d.json", JSON);
    let e = call(&mut a, "grid_edit", json!({ "id": id, "action": "set_value", "path": [1], "value": "two" })).expect_err("not a number");
    assert!(e.to_string().contains("not a number"), "{e}");
    let e = call(&mut a, "grid_edit", json!({ "id": id, "action": "set_value", "path": [9], "value": "x" })).expect_err("no such row");
    assert!(e.to_string().contains("not there"), "{e}");
    assert!(matches!(call(&mut a, "grid_edit", json!({ "id": id, "action": "set_value", "path": [1] })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(call(&mut a, "grid_edit", json!({ "id": id, "action": "frobnicate", "path": [1] })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(call(&mut a, "grid_edit", json!({ "id": id, "action": "remove", "path": ["x"] })), Err(ToolError::InvalidArgs(_))));
    assert_eq!(call(&mut a, "doc_text", json!({ "id": id })).expect("text")["text"].as_str(), Some(JSON));
    // XML and YAML documents too.
    let xml = open(&mut a, dir.path(), "d.xml", "<a n=\"1\">x</a>");
    call(&mut a, "grid_edit", json!({ "id": xml, "action": "set_value", "path": [0, 0], "value": "a&b" })).expect("edits");
    assert_eq!(call(&mut a, "doc_text", json!({ "id": xml })).expect("text")["text"].as_str(), Some("<a n=\"a&amp;b\">x</a>"));
    let yaml = open(&mut a, dir.path(), "d.yaml", "k: v\nn: 1\n");
    call(&mut a, "grid_edit", json!({ "id": yaml, "action": "remove", "path": [0] })).expect("edits");
    assert_eq!(call(&mut a, "doc_text", json!({ "id": yaml })).expect("text")["text"].as_str(), Some("n: 1\n"));
}

const ITEMS: &str = "[\n  {\"name\": \"Tea\", \"qty\": 2},\n  {\"name\": \"Milk\", \"qty\": 10},\n  {\"qty\": 1}\n]";

#[test]
fn grid_table_shows_columns_rows_and_cells_and_sorts_and_filters_as_views() {
    let mut a = Automation::new();
    let t = call(&mut a, "grid_table", json!({ "text": ITEMS, "language": "json" })).expect("a table");
    assert_eq!(t["columns"].as_array().map(|c| c.iter().map(|c| c["label"].as_str().unwrap_or("")).collect::<Vec<_>>()), Some(vec!["name", "qty"]));
    assert_eq!((t["count"].as_u64(), t["layout"].as_str()), (Some(3), Some("json array")));
    assert_eq!(t["rows"][2]["cells"][0]["kind"].as_str(), Some("missing"));
    assert_eq!((t["rows"][1]["cells"][1]["text"].as_str(), t["rows"][1]["cells"][1]["type"].as_str()), (Some("10"), Some("number")));
    assert_eq!(t["rows"][1]["line"].as_u64(), Some(3));
    let sorted = call(&mut a, "grid_table", json!({ "text": ITEMS, "language": "json", "sort": { "column": "qty" } })).expect("sorted");
    let order: Vec<u64> = sorted["rows"].as_array().map(|r| r.iter().filter_map(|r| r["row"].as_u64()).collect()).unwrap_or_default();
    assert_eq!(order, [2, 0, 1], "numbers by value");
    let filtered = call(&mut a, "grid_table", json!({ "text": ITEMS, "language": "json", "filter": "MILK" })).expect("filtered");
    assert_eq!((filtered["shown"].as_u64(), filtered["rows"][0]["row"].as_u64()), (Some(1), Some(1)));
    // XML: the repeated elements of a pointer, with attribute columns; and the deepest table around an offset.
    let xml = "<lib><book id=\"1\"><t>A</t></book><book id=\"2\"><t>B</t></book></lib>";
    let x = call(&mut a, "grid_table", json!({ "text": xml, "at": "/lib" })).expect("a table");
    assert_eq!(
        (x["item_name"].as_str(), x["columns"][0]["label"].as_str(), x["rows"][1]["cells"][1]["text"].as_str()),
        (Some("book"), Some("@id"), Some("B"))
    );
    let by_offset = call(&mut a, "grid_table", json!({ "text": xml, "at": 20 })).expect("a table");
    assert_eq!(by_offset["path"], json!([0]));
    // JSON Lines and YAML.
    let lines = call(&mut a, "grid_table", json!({ "text": "{\"a\": 1}\n{\"a\": 2}\n", "language": "jsonl" })).expect("a table");
    assert_eq!((lines["layout"].as_str(), lines["count"].as_u64()), (Some("json lines"), Some(2)));
    let yaml = call(&mut a, "grid_table", json!({ "text": "items:\n  - a: 1\n  - a: 2\n", "language": "yaml", "at": "/items" })).expect("a table");
    assert_eq!(yaml["layout"].as_str(), Some("yaml block sequence"));
}

#[test]
fn grid_table_edit_changes_an_open_document_as_one_undoable_step_or_returns_the_new_text() {
    let dir = tempfile::tempdir().expect("dir");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let id = open(&mut a, dir.path(), "items.json", ITEMS);
    let text = |a: &mut Automation| call(a, "doc_text", json!({ "id": id })).expect("text")["text"].as_str().unwrap_or_default().to_owned();
    // A missing cell, created in that row only.
    call(&mut a, "grid_table_edit", json!({ "id": id, "op": "set_cell", "row": 2, "column": "name", "value": "Oat" })).expect("edits");
    assert_eq!(text(&mut a), ITEMS.replace("{\"qty\": 1}", "{\"name\": \"Oat\", \"qty\": 1}"));
    call(&mut a, "doc_undo", json!({ "id": id })).expect("undo");
    assert_eq!(text(&mut a), ITEMS, "one undo is the whole edit");
    // A column that is not there yet.
    call(&mut a, "grid_table_edit", json!({ "id": id, "op": "set_cell", "row": 0, "column": "price", "value": "3.5" })).expect("edits");
    assert!(text(&mut a).contains("{\"name\": \"Tea\", \"qty\": 2, \"price\": \"3.5\"}"), "{}", text(&mut a));
    call(&mut a, "doc_undo", json!({ "id": id })).expect("undo");
    // Sort, then write the order.
    call(&mut a, "grid_table_edit", json!({ "id": id, "op": "apply_order", "sort": { "column": "qty", "descending": true } })).expect("edits");
    assert_eq!(text(&mut a), "[\n  {\"name\": \"Milk\", \"qty\": 10},\n  {\"name\": \"Tea\", \"qty\": 2},\n  {\"qty\": 1}\n]");
    call(&mut a, "doc_undo", json!({ "id": id })).expect("undo");
    assert_eq!(text(&mut a), ITEMS);
    // Text in, text out: rows, columns, a paste.
    let run = |a: &mut Automation, args: Value| call(a, "grid_table_edit", args).expect("edits")["text"].as_str().unwrap_or_default().to_owned();
    let base = json!({ "text": ITEMS, "language": "json" });
    let with = |extra: Value| {
        let mut args = base.clone();
        if let (Some(m), Some(e)) = (args.as_object_mut(), extra.as_object()) {
            m.extend(e.clone());
        }
        args
    };
    assert_eq!(run(&mut a, with(json!({ "op": "delete_rows", "rows": [0, 1] }))), "[\n  {\"qty\": 1}\n]");
    assert_eq!(run(&mut a, with(json!({ "op": "rename_column", "column": "qty", "new_name": "n" }))), ITEMS.replace("\"qty\"", "\"n\""));
    assert_eq!(
        run(&mut a, with(json!({ "op": "move_row", "row": 0, "before": null }))),
        "[\n  {\"name\": \"Milk\", \"qty\": 10},\n  {\"qty\": 1},\n  {\"name\": \"Tea\", \"qty\": 2}\n]"
    );
    let pasted =
        call(&mut a, "grid_table_edit", with(json!({ "op": "paste_tsv", "row": 2, "column": 0, "tsv": "Oat\t4\nRye\t5\n" }))).expect("pastes");
    assert_eq!((pasted["appended"].as_u64(), pasted["cells"].as_u64()), (Some(1), Some(4)));
    assert!(
        pasted["text"].as_str().is_some_and(|t| t.ends_with("{\"name\": \"Oat\", \"qty\": 4},\n  {\"name\": \"Rye\", \"qty\": 5}\n]")),
        "{pasted}"
    );
    // XML through a file in the root (the file is not written).
    std::fs::write(dir.path().join("lib.xml"), "<lib><book id=\"1\"/><book id=\"2\"/></lib>").expect("writes");
    let out = call(&mut a, "grid_table_edit", json!({ "path": "lib.xml", "at": "/lib", "op": "duplicate_row", "row": 1 })).expect("edits");
    assert_eq!(out["text"].as_str(), Some("<lib><book id=\"1\"/><book id=\"2\"/><book id=\"2\"/></lib>"));
    assert_eq!(std::fs::read_to_string(dir.path().join("lib.xml")).ok().as_deref(), Some("<lib><book id=\"1\"/><book id=\"2\"/></lib>"));
}

#[test]
fn grid_table_tools_refuse_bad_arguments_places_outside_the_root_and_what_holds_no_table() {
    let dir = tempfile::tempdir().expect("dir");
    let outside = tempfile::tempdir().expect("dir");
    std::fs::write(outside.path().join("x.json"), ITEMS).expect("writes");
    let mut a = Automation::with_root(dir.path()).expect("root");
    let escaped = outside.path().join("x.json").display().to_string();
    assert!(call(&mut a, "grid_table", json!({ "path": escaped })).is_err(), "outside the root");
    assert!(call(&mut a, "grid_table", json!({ "path": "../x.json" })).is_err());
    assert!(call(&mut a, "grid_table_edit", json!({ "path": escaped, "op": "insert_row" })).is_err());
    let base = |extra: Value| {
        let mut args = json!({ "text": ITEMS, "language": "json" });
        if let (Some(m), Some(e)) = (args.as_object_mut(), extra.as_object()) {
            m.extend(e.clone());
        }
        args
    };
    for (tool, args) in [
        ("grid_table", base(json!({ "max_rows": 0 }))),
        ("grid_table", base(json!({ "at": true }))),
        ("grid_table", base(json!({ "filter": 3 }))),
        ("grid_table_edit", base(json!({}))),
        ("grid_table_edit", base(json!({ "op": "explode" }))),
        ("grid_table_edit", base(json!({ "op": "set_cell", "column": "name", "value": "x" }))),
        ("grid_table_edit", base(json!({ "op": "set_cell", "row": -1, "column": "name", "value": "x" }))),
        ("grid_table_edit", base(json!({ "op": "delete_rows", "rows": [] }))),
        ("grid_table_edit", base(json!({ "op": "apply_order" }))),
    ] {
        assert!(matches!(call(&mut a, tool, args.clone()), Err(ToolError::InvalidArgs(_))), "{tool} {args}");
    }
    for (tool, args) in [
        ("grid_table", json!({ "text": "{\"a\": 1}", "language": "json" })),
        ("grid_table", base(json!({ "at": "/9" }))),
        ("grid_table", base(json!({ "sort": { "column": "nope" } }))),
        ("grid_table_edit", base(json!({ "op": "set_cell", "row": 9, "column": "name", "value": "x" }))),
        ("grid_table_edit", base(json!({ "op": "apply_order", "order": [0, 0, 1] }))),
        ("grid_table_edit", base(json!({ "op": "rename_column", "column": "qty", "new_name": "name" }))),
        ("grid_table_edit", base(json!({ "op": "clear_cell", "row": 0, "column": 7 }))),
    ] {
        assert!(matches!(call(&mut a, tool, args.clone()), Err(ToolError::Failed(_))), "{tool} {args}");
    }
}
