//! Tables of repeated items: what the table of XML, JSON, JSONC, JSON Lines, YAML and Extended JSON shows, and every edit of it as the
//! smallest edits of the text.

use xmlcraft_diff::Family;
use xmlcraft_grid::{CellKind, ColumnKind, Flavor, Grid, Layout, Table};
use xmlcraft_text::{Edit, apply_edits};

fn table(text: &str, family: Family, flavor: Flavor, path: &[usize]) -> (Grid, Table) {
    let g = Grid::read_flavored(text, family, flavor).expect("reads");
    let t = g.table(text, path).expect("a table");
    (g, t)
}

fn labels(t: &Table) -> Vec<String> {
    t.columns.iter().map(|c| c.label()).collect()
}

fn cells(t: &Table, row: usize) -> Vec<(CellKind, String)> {
    t.rows[row].cells.iter().map(|c| (c.kind, c.text.clone())).collect()
}

fn apply(text: &str, edits: &[Edit]) -> String {
    apply_edits(text, edits)
}

/// The text after the edits, which must read again as the format, and its table at the same place.
fn after(text: &str, edits: &[Edit], family: Family, flavor: Flavor, path: &[usize]) -> (String, Table) {
    let out = apply(text, edits);
    let g = Grid::read_flavored(&out, family, flavor).unwrap_or_else(|e| panic!("{e}: {out}"));
    let t = g.table(&out, path).unwrap_or_else(|| panic!("no table in {out}"));
    (out, t)
}

// --- JSON --------------------------------------------------------------------------------------------------------------------

const JSON: &str = "[\n  {\"name\": \"Tea\", \"qty\": 2},\n  {\"name\": \"Milk\", \"qty\": 10, \"tags\": [\"a\"]},\n  {\"qty\": 1}\n]";

#[test]
fn a_json_array_of_objects_is_a_table_with_the_union_of_its_keys() {
    let (_, t) = table(JSON, Family::Json, Flavor::Plain, &[]);
    assert_eq!(labels(&t), ["name", "qty", "tags"]);
    assert_eq!(t.layout, Layout::JsonArray);
    assert_eq!(cells(&t, 1), [(CellKind::Scalar, "Milk".into()), (CellKind::Scalar, "10".into()), (CellKind::Nested, "[1]".into())]);
    assert_eq!(cells(&t, 2)[0], (CellKind::Missing, String::new()));
    assert!(t.columns[1].numeric && !t.columns[0].numeric);
    assert_eq!(&JSON[t.rows[0].cells[0].span.clone()], "\"Tea\"");
    // A nested cell drills into its node.
    assert_eq!(t.rows[1].cells[2].drill.as_ref().map(|d| d.path.clone()), Some(vec![1, 2]));
    // Not a table: an object, mixed items, an empty array.
    let g = Grid::read("{\"a\": [1, {\"b\": 2}], \"e\": []}", Family::Json).expect("reads");
    assert!(g.table("{\"a\": [1, {\"b\": 2}], \"e\": []}", &[]).is_none());
    assert!(g.table("{\"a\": [1, {\"b\": 2}], \"e\": []}", &[0]).is_none());
    assert!(g.table("{\"a\": [1, {\"b\": 2}], \"e\": []}", &[1]).is_none());
    // All scalars: one column.
    let (_, s) = table("[1, 2, 3]", Family::Json, Flavor::Plain, &[]);
    assert_eq!((labels(&s), s.columns[0].kind, s.rows.len()), (vec!["value".to_owned()], ColumnKind::Value, 3));
}

#[test]
fn a_json_cell_is_set_in_place_keeping_its_type_when_it_reads_as_one() {
    let (g, t) = table(JSON, Family::Json, Flavor::Plain, &[]);
    let set = |r, c, v: &str| apply(JSON, &g.set_cell(JSON, &t, r, c, v).expect("edits"));
    assert_eq!(set(0, 1, "5"), JSON.replacen("\"qty\": 2", "\"qty\": 5", 1));
    assert_eq!(set(0, 1, "five"), JSON.replacen("\"qty\": 2", "\"qty\": \"five\"", 1), "not a number: a string");
    assert_eq!(set(0, 0, "12"), JSON.replacen("\"Tea\"", "\"12\"", 1), "a string column stays strings");
    assert_eq!(set(0, 0, "a \"b\""), JSON.replacen("\"Tea\"", "\"a \\\"b\\\"\"", 1));
    assert!(g.set_cell(JSON, &t, 1, 2, "x").is_err(), "a nested cell is opened, not overwritten");
    assert_eq!(g.set_cell(JSON, &t, 0, 0, "Tea").expect("edits"), vec![], "the same value: no edit");
    assert_eq!(apply(JSON, &g.set_cell_literal(JSON, &t, 0, 1, "null").expect("edits")), JSON.replacen("\"qty\": 2", "\"qty\": null", 1));
    assert!(g.set_cell_literal(JSON, &t, 0, 1, "\"a\"b\"").is_err(), "not one JSON value");
}

#[test]
fn a_missing_json_cell_is_created_in_that_row_only_after_the_column_before_it() {
    let (g, t) = table(JSON, Family::Json, Flavor::Plain, &[]);
    // Row 3 has no name: it goes first, before qty.
    let (out, t2) = after(JSON, &g.set_cell(JSON, &t, 2, 0, "Oat").expect("edits"), Family::Json, Flavor::Plain, &[]);
    assert_eq!(out, JSON.replace("{\"qty\": 1}", "{\"name\": \"Oat\", \"qty\": 1}"));
    assert_eq!(cells(&t2, 2)[0], (CellKind::Scalar, "Oat".into()));
    // Row 1 has no tags: after qty. A number column makes "7" a number.
    let out = apply(JSON, &g.set_cell(JSON, &t, 0, 2, "x").expect("edits"));
    assert_eq!(out, JSON.replace("\"qty\": 2}", "\"qty\": 2, \"tags\": \"x\"}"));
    let out = apply(JSON, &g.set_cell(JSON, &t, 2, 1, "7").expect("edits"));
    assert_eq!(out, JSON.replace("{\"qty\": 1}", "{\"qty\": 7}"));
    // Where it would go is where it goes.
    assert_eq!(t.rows[2].cells[0].span, JSON.find("\"qty\": 1}").map(|i| i..i).expect("there"));
    // An empty value in an empty cell writes nothing.
    assert_eq!(g.set_cell(JSON, &t, 2, 0, "").expect("edits"), vec![]);
}

const PRETTY: &str = "{\n  \"items\": [\n    {\n      \"id\": 1,\n      \"name\": \"a\"\n    },\n    {\n      \"id\": 2\n    }\n  ]\n}\n";

#[test]
fn a_member_created_in_a_pretty_printed_object_gets_its_own_line_and_indentation() {
    let (g, t) = table(PRETTY, Family::Json, Flavor::Plain, &[0]);
    assert_eq!(labels(&t), ["id", "name"]);
    let out = apply(PRETTY, &g.set_cell(PRETTY, &t, 1, 1, "b").expect("edits"));
    assert_eq!(out, PRETTY.replace("\"id\": 2\n", "\"id\": 2,\n      \"name\": \"b\"\n"));
    // A new row has the shape of its neighbour, pretty-printed like it, with the columns every row has.
    let out = apply(PRETTY, &g.insert_row(PRETTY, &t, Some(1)).expect("edits"));
    assert_eq!(out, PRETTY.replace("\"id\": 2\n    }\n", "\"id\": 2\n    },\n    {\n      \"id\": null\n    }\n"));
    let out = apply(PRETTY, &g.insert_row(PRETTY, &t, None).expect("edits"));
    assert!(out.starts_with("{\n  \"items\": [\n    {\n      \"id\": null\n    },\n    {\n      \"id\": 1,"), "{out}");
}

const JSONC: &str = "{\n  // the stock\n  \"items\": [\n    {\"name\": \"Tea\", \"qty\": 2}, // first\n    {\"name\": \"Milk\"} // last\n  ]\n}";

#[test]
fn jsonc_keeps_its_comments_where_they_are() {
    let (g, t) = table(JSONC, Family::Json, Flavor::Plain, &[0]);
    let (out, _) = after(JSONC, &g.set_cell(JSONC, &t, 1, 1, "3").expect("edits"), Family::Json, Flavor::Plain, &[0]);
    assert_eq!(out, JSONC.replace("{\"name\": \"Milk\"}", "{\"name\": \"Milk\", \"qty\": 3}"), "the column holds numbers, so 3 is one");
    // A row after one with a comment after its comma goes after the comment; after the last, the comma goes before the comment.
    let out = apply(JSONC, &g.insert_row(JSONC, &t, Some(0)).expect("edits"));
    assert_eq!(out, JSONC.replace("}, // first\n", "}, // first\n    {\"name\": \"\"},\n"));
    let (out, _) = after(JSONC, &g.insert_row(JSONC, &t, Some(1)).expect("edits"), Family::Json, Flavor::Plain, &[0]);
    assert_eq!(out, JSONC.replace("{\"name\": \"Milk\"} // last\n", "{\"name\": \"Milk\"}, // last\n    {\"name\": \"\"}\n"));
    assert!(out.contains("// the stock"));
}

#[test]
fn rows_are_inserted_duplicated_deleted_and_moved_in_json() {
    let (g, t) = table(JSON, Family::Json, Flavor::Plain, &[]);
    let (out, t2) = after(JSON, &g.duplicate_row(JSON, &t, 0).expect("edits"), Family::Json, Flavor::Plain, &[]);
    assert_eq!(out, JSON.replace("{\"name\": \"Tea\", \"qty\": 2},", "{\"name\": \"Tea\", \"qty\": 2},\n  {\"name\": \"Tea\", \"qty\": 2},"));
    assert_eq!(t2.rows.len(), 4);
    let out = apply(JSON, &g.delete_rows(JSON, &t, &[0, 2]).expect("edits"));
    assert_eq!(out, "[\n  {\"name\": \"Milk\", \"qty\": 10, \"tags\": [\"a\"]}\n]");
    let out = apply(JSON, &g.delete_rows(JSON, &t, &[2]).expect("edits"));
    assert_eq!(
        out, "[\n  {\"name\": \"Tea\", \"qty\": 2},\n  {\"name\": \"Milk\", \"qty\": 10, \"tags\": [\"a\"]}\n]",
        "the last takes the comma before it"
    );
    assert_eq!(apply(JSON, &g.delete_rows(JSON, &t, &[0, 1, 2]).expect("edits")), "[]");
    let out = apply(JSON, &g.move_row(JSON, &t, 2, Some(0)).expect("edits"));
    assert_eq!(out, "[\n  {\"qty\": 1},\n  {\"name\": \"Tea\", \"qty\": 2},\n  {\"name\": \"Milk\", \"qty\": 10, \"tags\": [\"a\"]}\n]");
    assert_eq!(g.move_row(JSON, &t, 1, Some(1)).expect("edits"), vec![], "in its place already");
    let out = apply(JSON, &g.insert_row(JSON, &t, Some(0)).expect("edits"));
    assert_eq!(out, JSON.replace("\"qty\": 2},", "\"qty\": 2},\n  {\"qty\": null},"), "qty is the column every row has");
}

#[test]
fn sorting_and_filtering_are_views_and_apply_order_writes_the_order() {
    let (g, t) = table(JSON, Family::Json, Flavor::Plain, &[]);
    assert_eq!(t.sorted(1, true), [2, 0, 1], "numbers by value: 1 < 2 < 10");
    assert_eq!(t.sorted(1, false), [1, 0, 2]);
    assert_eq!(t.sorted(0, true), [1, 0, 2], "text, and the empty cell last");
    assert_eq!(t.filtered("mil"), [1]);
    assert_eq!(t.filtered(""), [0, 1, 2]);
    let out = apply(JSON, &g.apply_order(JSON, &t, &t.sorted(1, true)).expect("edits"));
    assert_eq!(out, "[\n  {\"qty\": 1},\n  {\"name\": \"Tea\", \"qty\": 2},\n  {\"name\": \"Milk\", \"qty\": 10, \"tags\": [\"a\"]}\n]");
    for bad in [vec![0, 1], vec![0, 0, 1], vec![0, 1, 3]] {
        assert!(g.apply_order(JSON, &t, &bad).is_err(), "{bad:?}");
    }
}

#[test]
fn columns_are_inserted_renamed_and_deleted_in_every_row() {
    let (g, t) = table(JSON, Family::Json, Flavor::Plain, &[]);
    let wider = g.insert_column(JSON, &t, "price", Some(1)).expect("a column");
    assert_eq!(labels(&wider), ["name", "qty", "price", "tags"]);
    assert!(wider.rows.iter().all(|r| r.cells[2].kind == CellKind::Missing), "nothing in the text yet");
    let out = apply(JSON, &g.set_cell(JSON, &wider, 1, 2, "3").expect("edits"));
    assert_eq!(out, JSON.replace("\"qty\": 10,", "\"qty\": 10, \"price\": \"3\","));
    assert!(g.insert_column(JSON, &t, "qty", None).is_err());
    let out = apply(JSON, &g.rename_column(JSON, &t, 1, "count").expect("edits"));
    assert_eq!(out, JSON.replace("\"qty\"", "\"count\""));
    let out = apply(JSON, &g.delete_column(JSON, &t, 1).expect("edits"));
    assert_eq!(out, "[\n  {\"name\": \"Tea\"},\n  {\"name\": \"Milk\", \"tags\": [\"a\"]},\n  {}\n]");
    let clash = "[{\"a\": 1, \"b\": 2}, {\"a\": 3}]";
    let (g, t) = table(clash, Family::Json, Flavor::Plain, &[]);
    assert!(g.rename_column(clash, &t, 0, "b").is_err(), "row 1 has b already");
}

#[test]
fn tsv_copies_and_pastes_and_a_paste_adds_rows_at_the_end() {
    let (g, t) = table(JSON, Family::Json, Flavor::Plain, &[]);
    let tsv = t.to_tsv(&[0, 1], &[0, 1]);
    assert_eq!(tsv, "Tea\t2\nMilk\t10\n");
    // Paste it onto rows 2 and 3: row 3 gets a name.
    let pasted = g.paste_tsv(JSON, &t, 1, 0, &tsv).expect("pastes");
    let (out, t2) = after(JSON, &pasted.edits, Family::Json, Flavor::Plain, &[]);
    assert_eq!(cells(&t2, 1)[..2], [(CellKind::Scalar, "Tea".into()), (CellKind::Scalar, "2".into())]);
    assert_eq!(cells(&t2, 2)[..2], [(CellKind::Scalar, "Milk".into()), (CellKind::Scalar, "10".into())]);
    assert!(out.contains("{\"name\": \"Milk\", \"qty\": 10}"), "{out}");
    // Past the end: rows are added; past the last column: skipped.
    let pasted = g.paste_tsv(JSON, &t, 2, 1, "4\tx\ty\n5\n").expect("pastes");
    assert_eq!((pasted.appended, pasted.skipped), (1, 1));
    let (_, t3) = after(JSON, &pasted.edits, Family::Json, Flavor::Plain, &[]);
    assert_eq!(t3.rows.len(), 4);
    assert_eq!(cells(&t3, 3)[1], (CellKind::Scalar, "5".into()));
    assert_eq!(t3.rows[3].cells[1].value_kind, xmlcraft_grid::RowKind::Number);
    assert!(g.paste_tsv(JSON, &t, 9, 0, "a").is_err());
}

// --- JSON Lines and Extended JSON --------------------------------------------------------------------------------------------

const LINES: &str = "{\"id\": 1, \"ok\": true}\n{\"id\": 2}\n{\"id\": 3, \"ok\": false}\n";

#[test]
fn json_lines_are_a_table_of_the_document_itself_one_item_to_a_line() {
    let (g, t) = table(LINES, Family::Json, Flavor::JsonLines, &[]);
    assert_eq!((t.layout, labels(&t), t.rows.len()), (Layout::JsonLines, vec!["id".to_owned(), "ok".to_owned()], 3));
    let out = apply(LINES, &g.set_cell(LINES, &t, 1, 1, "true").expect("edits"));
    assert_eq!(out, LINES.replace("{\"id\": 2}", "{\"id\": 2, \"ok\": true}"), "the column is booleans");
    let out = apply(LINES, &g.insert_row(LINES, &t, Some(2)).expect("edits"));
    assert_eq!(out, format!("{LINES}{{\"id\": null}}\n").replace("false}\n{", "false}\n{"), "a line of its own");
    let out = apply(LINES, &g.delete_rows(LINES, &t, &[0]).expect("edits"));
    assert_eq!(out, "{\"id\": 2}\n{\"id\": 3, \"ok\": false}\n");
    let out = apply(LINES, &g.move_row(LINES, &t, 0, None).expect("edits"));
    assert_eq!(out, "{\"id\": 2}\n{\"id\": 3, \"ok\": false}\n{\"id\": 1, \"ok\": true}\n");
    let no_end = "{\"a\": 1}\n{\"a\": 2}";
    let (g, t) = table(no_end, Family::Json, Flavor::JsonLines, &[]);
    assert_eq!(apply(no_end, &g.delete_rows(no_end, &t, &[1]).expect("edits")), "{\"a\": 1}");
    assert!(Grid::read_flavored("{\"a\": 1}\n{oops}\n", Family::Json, Flavor::JsonLines).is_err());
}

#[test]
fn bson_wrappers_are_one_value_and_are_edited_inside() {
    let ejson =
        "[{\"_id\": {\"$oid\": \"64b0\"}, \"n\": {\"$numberLong\": \"5\"}}, {\"_id\": {\"$oid\": \"64b1\"}, \"n\": {\"$numberLong\": \"7\"}}]";
    let (g, t) = table(ejson, Family::Json, Flavor::ExtendedJson, &[]);
    assert_eq!(cells(&t, 0), [(CellKind::Scalar, "64b0".into()), (CellKind::Scalar, "5".into())]);
    assert!(t.columns[1].numeric);
    let out = apply(ejson, &g.set_cell(ejson, &t, 1, 1, "9").expect("edits"));
    assert_eq!(out, ejson.replace("\"7\"", "\"9\""), "the wrapper stays");
    let (_, plain) = table(ejson, Family::Json, Flavor::Plain, &[]);
    assert_eq!(plain.rows[0].cells[0].kind, CellKind::Nested, "plain JSON shows the object");
}

// --- YAML ----------------------------------------------------------------------------------------------------------------

const YAML: &str = "items:\n  - name: Tea\n    qty: 2\n  - name: Milk\n    qty: 10\n    tags: [a]\n  - qty: 1\nnext: true\n";

#[test]
fn a_yaml_block_sequence_is_a_table_and_its_cells_are_written_with_yaml_indentation() {
    let (g, t) = table(YAML, Family::Yaml, Flavor::Plain, &[0]);
    assert_eq!((t.layout, labels(&t)), (Layout::YamlBlock, vec!["name".to_owned(), "qty".to_owned(), "tags".to_owned()]));
    let set = |r, c, v: &str| after(YAML, &g.set_cell(YAML, &t, r, c, v).expect("edits"), Family::Yaml, Flavor::Plain, &[0]).0;
    assert_eq!(set(0, 0, "Chai"), YAML.replace("Tea", "Chai"));
    assert_eq!(set(0, 0, "true"), YAML.replace("Tea", "\"true\""), "a string column: quoted where it would read as something else");
    assert_eq!(set(2, 0, "Oat"), YAML.replace("  - qty: 1\n", "  - name: Oat\n    qty: 1\n"), "first, before qty, after the dash");
    assert_eq!(set(0, 2, "x"), YAML.replace("    qty: 2\n", "    qty: 2\n    tags: x\n"));
    let clear = |r, c| apply(YAML, &g.clear_cell(YAML, &t, r, c).expect("edits"));
    assert_eq!(clear(0, 1), YAML.replace("    qty: 2\n", ""));
    assert_eq!(clear(0, 0), YAML.replace("  - name: Tea\n    qty: 2\n", "  - qty: 2\n"), "the next member moves up after the dash");
    assert_eq!(clear(2, 1), YAML.replace("  - qty: 1\n", "  - {}\n"), "the only member: an empty mapping");
}

#[test]
fn yaml_rows_are_inserted_deleted_and_moved_as_lines() {
    let (g, t) = table(YAML, Family::Yaml, Flavor::Plain, &[0]);
    let (out, t2) = after(YAML, &g.insert_row(YAML, &t, Some(0)).expect("edits"), Family::Yaml, Flavor::Plain, &[0]);
    assert_eq!(out, YAML.replace("    qty: 2\n", "    qty: 2\n  - qty: null\n"));
    assert_eq!(t2.rows.len(), 4);
    let out = apply(YAML, &g.duplicate_row(YAML, &t, 1).expect("edits"));
    assert_eq!(out, YAML.replace("    tags: [a]\n", "    tags: [a]\n  - name: Milk\n    qty: 10\n    tags: [a]\n"));
    assert_eq!(apply(YAML, &g.delete_rows(YAML, &t, &[1]).expect("edits")), YAML.replace("  - name: Milk\n    qty: 10\n    tags: [a]\n", ""));
    assert_eq!(apply(YAML, &g.delete_rows(YAML, &t, &[0, 1, 2]).expect("edits")), "items:\n  []\nnext: true\n", "empty, not null");
    let (out, _) = after(YAML, &g.move_row(YAML, &t, 2, Some(0)).expect("edits"), Family::Yaml, Flavor::Plain, &[0]);
    assert_eq!(
        out,
        "items:\n  - qty: 1\n  - name: Milk\n    qty: 10\n    tags: [a]\n  - name: Tea\n    qty: 2\nnext: true\n".replace(
            "  - name: Milk\n    qty: 10\n    tags: [a]\n  - name: Tea\n    qty: 2\n",
            "  - name: Tea\n    qty: 2\n  - name: Milk\n    qty: 10\n    tags: [a]\n"
        )
    );
    let flow = "list: [{a: 1}, {a: 2}]\n";
    let (g, t) = table(flow, Family::Yaml, Flavor::Plain, &[0]);
    assert_eq!(t.layout, Layout::YamlFlow);
    assert_eq!(apply(flow, &g.set_cell(flow, &t, 0, 0, "5").expect("edits")), "list: [{a: 5}, {a: 2}]\n");
    assert_eq!(apply(flow, &g.insert_row(flow, &t, Some(1)).expect("edits")), "list: [{a: 1}, {a: 2}, {a: null}]\n");
    let streamed = "---\na: 1\n---\na: 2\n";
    assert_eq!(Grid::read(streamed, Family::Yaml).expect("reads").table(streamed, &[]).map(|t| t.layout), Some(Layout::YamlDocuments));
    let docs = "a: 1\n---\na: 2\n";
    let (g, t) = table(docs, Family::Yaml, Flavor::Plain, &[]);
    assert_eq!(t.layout, Layout::YamlDocuments);
    assert_eq!(apply(docs, &g.set_cell(docs, &t, 1, 0, "3").expect("edits")), "a: 1\n---\na: 3\n");
    assert!(g.insert_row(docs, &t, None).is_err() && g.delete_rows(docs, &t, &[0]).is_err());
}

// --- XML -----------------------------------------------------------------------------------------------------------------

const XML: &str = "<library name=\"City\">\n  <book id=\"b1\" lang=\"en\">\n    <title>Almanac</title>\n    <author>Ada</author>\n    <year>2019</year>\n  </book>\n  <book id=\"b2\">\n    <title>Nebel &amp; Wege</title>\n    <author>Jonas</author>\n    <author>Mira</author>\n  </book>\n  <!-- more soon -->\n  <shelf/>\n</library>\n";

#[test]
fn repeated_xml_elements_are_rows_with_attribute_and_element_columns() {
    let (_, t) = table(XML, Family::Xml, Flavor::Plain, &[0]);
    assert_eq!(t.item_name.as_deref(), Some("book"));
    assert_eq!(labels(&t), ["@id", "@lang", "title", "author", "year"]);
    assert_eq!(cells(&t, 0)[..3], [(CellKind::Scalar, "b1".into()), (CellKind::Scalar, "en".into()), (CellKind::Scalar, "Almanac".into())]);
    assert_eq!(cells(&t, 1)[1], (CellKind::Missing, String::new()));
    assert_eq!(cells(&t, 1)[2], (CellKind::Scalar, "Nebel & Wege".into()), "decoded");
    assert_eq!(cells(&t, 1)[3], (CellKind::Nested, "<2>".into()), "two authors");
    assert!(t.columns[4].numeric);
    // Drilling into the two authors is the table of the authors of that book.
    let g = Grid::read(XML, Family::Xml).expect("reads");
    let drill = t.rows[1].cells[3].drill.clone().expect("drills");
    let authors = g.table_named(XML, &drill.path, drill.name.as_deref()).expect("a table");
    assert_eq!((labels(&authors), authors.rows.len()), (vec!["#text".to_owned()], 2));
    assert_eq!(authors.rows[1].cells[0].text, "Mira");
    // One element of a name is no table, unless asked by name.
    assert!(g.table(XML, &[0, 1]).is_none());
}

#[test]
fn xml_cells_are_set_created_and_cleared_with_escaping_and_indentation() {
    let (g, t) = table(XML, Family::Xml, Flavor::Plain, &[0]);
    let set = |r, c, v: &str| after(XML, &g.set_cell(XML, &t, r, c, v).expect("edits"), Family::Xml, Flavor::Plain, &[0]).0;
    assert_eq!(set(0, 0, "a\"&b"), XML.replace("id=\"b1\"", "id=\"a&quot;&amp;b\""));
    assert_eq!(set(0, 2, "<New>"), XML.replace("Almanac", "&lt;New&gt;"));
    assert_eq!(set(1, 1, "de"), XML.replace("<book id=\"b2\">", "<book id=\"b2\" lang=\"de\">"), "after the attribute before it");
    assert_eq!(
        set(1, 4, "2021"),
        XML.replace("    <author>Mira</author>\n", "    <author>Mira</author>\n    <year>2021</year>\n"),
        "after the last author"
    );
    assert!(g.set_cell(XML, &t, 1, 3, "x").is_err(), "two authors: drill in");
    assert!(g.set_cell_literal(XML, &t, 0, 0, "1").is_err());
    let clear = |r, c| apply(XML, &g.clear_cell(XML, &t, r, c).expect("edits"));
    assert_eq!(clear(0, 1), XML.replace(" lang=\"en\"", ""));
    assert_eq!(clear(0, 4), XML.replace("    <year>2019</year>\n", ""), "with its line");
    assert_eq!(clear(1, 3), XML.replace("    <author>Jonas</author>\n    <author>Mira</author>\n", ""));
}

#[test]
fn xml_rows_are_inserted_duplicated_deleted_and_moved_and_other_children_stay_put() {
    let (g, t) = table(XML, Family::Xml, Flavor::Plain, &[0]);
    let (out, t2) = after(XML, &g.insert_row(XML, &t, Some(1)).expect("edits"), Family::Xml, Flavor::Plain, &[0]);
    assert_eq!(
        out,
        XML.replace(
            "    <author>Mira</author>\n  </book>\n",
            "    <author>Mira</author>\n  </book>\n  <book id=\"\">\n    <title></title>\n    <author></author>\n  </book>\n"
        )
    );
    assert_eq!(t2.rows.len(), 3);
    let out = apply(XML, &g.delete_rows(XML, &t, &[0]).expect("edits"));
    assert!(out.starts_with("<library name=\"City\">\n  <book id=\"b2\">"), "{out}");
    assert!(out.contains("<!-- more soon -->\n  <shelf/>"));
    let (out, t3) = after(XML, &g.move_row(XML, &t, 1, Some(0)).expect("edits"), Family::Xml, Flavor::Plain, &[0]);
    assert_eq!(t3.rows[0].cells[0].text, "b2");
    assert!(out.ends_with("  <!-- more soon -->\n  <shelf/>\n</library>\n"));
    let (_, t4) = after(XML, &g.duplicate_row(XML, &t, 0).expect("edits"), Family::Xml, Flavor::Plain, &[0]);
    assert_eq!((t4.rows.len(), t4.rows[1].cells[0].text.as_str()), (3, "b1"));
    // A new element in an item that has none: one level deeper, on its own lines.
    let empty = "<list>\n  <item id=\"1\"/>\n  <item id=\"2\">\n    <name>x</name>\n  </item>\n</list>";
    let (g, t) = table(empty, Family::Xml, Flavor::Plain, &[0]);
    let out = apply(empty, &g.set_cell(empty, &t, 0, 1, "y").expect("edits"));
    assert_eq!(out, empty.replace("<item id=\"1\"/>", "<item id=\"1\">\n    <name>y</name>\n  </item>"));
    // Renaming a column renames every element of it, start and end tags.
    let out = apply(XML, &{
        let (g, t) = table(XML, Family::Xml, Flavor::Plain, &[0]);
        g.rename_column(XML, &t, 3, "writer").expect("edits")
    });
    assert_eq!(out, XML.replace("<author>", "<writer>").replace("</author>", "</writer>"));
}

#[test]
fn an_xml_column_is_an_attribute_by_its_at_sign_and_text_items_have_a_text_column() {
    let text = "<tags><tag>a</tag><tag k=\"1\">b</tag><tag/></tags>";
    let (g, t) = table(text, Family::Xml, Flavor::Plain, &[0]);
    assert_eq!(labels(&t), ["@k", "#text"]);
    assert_eq!(apply(text, &g.set_cell(text, &t, 2, 1, "c").expect("edits")), text.replace("<tag/>", "<tag>c</tag>"));
    let wider = g.insert_column(text, &t, "@lang", Some(0)).expect("a column");
    assert_eq!(wider.columns[1].kind, ColumnKind::Attribute);
    assert_eq!(apply(text, &g.set_cell(text, &wider, 0, 1, "en").expect("edits")), text.replace("<tag>a", "<tag lang=\"en\">a"));
    assert!(g.insert_column(text, &t, "1bad", None).is_err());
    let out = apply(text, &g.insert_row(text, &t, None).expect("edits"));
    assert_eq!(out, "<tags><tag/><tag>a</tag><tag k=\"1\">b</tag><tag/></tags>");
    assert!(g.set_cell(text, &t, 0, 0, "x").is_ok());
}

#[test]
fn a_table_that_is_out_of_date_or_of_another_text_is_refused() {
    let (g, t) = table(JSON, Family::Json, Flavor::Plain, &[]);
    assert!(g.set_cell("[]", &t, 0, 0, "x").is_err());
    let other = "[{\"name\": \"Tea\", \"qty\": 2}]";
    let (g2, _) = table(other, Family::Json, Flavor::Plain, &[]);
    assert!(g2.set_cell(other, &t, 0, 0, "x").is_err(), "the rows are elsewhere");
    assert!(g.set_cell(JSON, &t, 9, 0, "x").is_err() && g.set_cell(JSON, &t, 0, 9, "x").is_err());
}

#[test]
fn pointers_and_offsets_find_tables() {
    let g = Grid::read(XML, Family::Xml).expect("reads");
    assert_eq!(g.path_of_pointer("/library").expect("there"), vec![0]);
    assert_eq!(g.path_of_pointer("/library/book[2]").expect("there"), vec![0, 4], "the attribute and the white space are children too");
    assert!(g.path_of_pointer("/library/book[0]").is_err() && g.path_of_pointer("/nope").is_err());
    let at = XML.find("Ada").expect("there");
    assert_eq!(g.table_path_at(XML, at), Some(vec![0]));
    let g = Grid::read(PRETTY, Family::Json).expect("reads");
    assert_eq!(g.path_of_pointer("/items").expect("there"), vec![0]);
    assert_eq!(g.path_of_pointer("/items/1").expect("there"), vec![0, 1]);
    assert_eq!(g.names_along(&[0, 1]), ["items", "[1]"]);
}

#[test]
fn a_wide_table_shows_two_hundred_columns_and_counts_the_rest() {
    let members: Vec<String> = (0..250).map(|i| format!("\"k{i}\": {i}")).collect();
    let text = format!("[{{{}}}]", members.join(", "));
    let (_, t) = table(&text, Family::Json, Flavor::Plain, &[]);
    assert_eq!((t.columns.len(), t.hidden_columns), (xmlcraft_grid::MAX_COLUMNS, 50));
}

#[test]
fn new_lines_use_the_documents_line_ending_and_indentation() {
    let crlf = "[\r\n\t{\"a\": 1},\r\n\t{\"a\": 2}\r\n]\r\n";
    let (g, t) = table(crlf, Family::Json, Flavor::Plain, &[]);
    assert_eq!(apply(crlf, &g.insert_row(crlf, &t, Some(1)).expect("edits")), "[\r\n\t{\"a\": 1},\r\n\t{\"a\": 2},\r\n\t{\"a\": null}\r\n]\r\n");
    let xml = "<r>\r\n\t<i n=\"1\"/>\r\n\t<i n=\"2\"><v>x</v></i>\r\n</r>";
    let (g, t) = table(xml, Family::Xml, Flavor::Plain, &[0]);
    assert_eq!(apply(xml, &g.set_cell(xml, &t, 0, 1, "y").expect("edits")), xml.replace("<i n=\"1\"/>", "<i n=\"1\">\r\n\t\t<v>y</v>\r\n\t</i>"));
    assert_eq!(apply(xml, &g.delete_rows(xml, &t, &[0]).expect("edits")), "<r>\r\n\t<i n=\"2\"><v>x</v></i>\r\n</r>");
}
