//! The grid: rows from XML, JSON and YAML, and the edits that change a cell.

use std::collections::HashSet;

use xmlcraft_diff::Family;
use xmlcraft_grid::{Grid, RowKind};
use xmlcraft_text::Edit;

/// The text with `edits` applied (the edits do not overlap; applied from the end).
fn apply(text: &str, edits: &[Edit]) -> String {
    let mut out = text.to_owned();
    let mut sorted: Vec<&Edit> = edits.iter().collect();
    sorted.sort_by_key(|e| std::cmp::Reverse(e.range.start));
    for e in sorted {
        out.replace_range(e.range.clone(), &e.text);
    }
    out
}

fn all_open(grid: &Grid) -> HashSet<Vec<usize>> {
    grid.expandable().into_iter().collect()
}

fn names(grid: &Grid, open: &HashSet<Vec<usize>>) -> Vec<(usize, String, String)> {
    grid.rows(open).into_iter().map(|r| (r.depth, r.name, r.value)).collect()
}

const JSON: &str = "{\n  \"name\": \"Tea\",\n  \"qty\": 2,\n  \"tags\": [\"a\", \"b\"],\n  \"ok\": true,\n  \"none\": null\n}";

#[test]
fn json_rows_nest_and_show_members_items_and_values() {
    let g = Grid::read(JSON, Family::Json).expect("reads");
    let top = names(&g, &HashSet::new());
    assert_eq!(top.len(), 5, "the members of the root object are the rows, closed: {top:?}");
    let open = all_open(&g);
    let rows = names(&g, &open);
    let expected: Vec<(usize, &str, &str)> =
        vec![(0, "name", "Tea"), (0, "qty", "2"), (0, "tags", "[2]"), (1, "[0]", "a"), (1, "[1]", "b"), (0, "ok", "true"), (0, "none", "null")];
    assert_eq!(rows.iter().map(|(d, n, v)| (*d, n.as_str(), v.as_str())).collect::<Vec<_>>(), expected);
    let kinds: Vec<RowKind> = g.rows(&open).into_iter().map(|r| r.kind).collect();
    assert_eq!(kinds[0..3], [RowKind::String, RowKind::Number, RowKind::Array]);
}

#[test]
fn a_json_value_is_replaced_in_place_with_the_right_literal_for_its_type() {
    let g = Grid::read(JSON, Family::Json).expect("reads");
    let open = all_open(&g);
    let row = |name: &str| g.rows(&open).into_iter().find(|r| r.name == name).expect("a row").path;
    let edited = apply(JSON, &g.set_value(JSON, &row("name"), "Chai \"special\"\nlatte").expect("edits"));
    assert_eq!(edited, JSON.replace("\"Tea\"", "\"Chai \\\"special\\\"\\nlatte\""), "only the value changed, escaped");
    assert_eq!(apply(JSON, &g.set_value(JSON, &row("qty"), "-3.5e2").expect("edits")), JSON.replace("2,", "-3.5e2,"));
    assert_eq!(apply(JSON, &g.set_value(JSON, &row("ok"), "false").expect("edits")), JSON.replace("true", "false"));
    for (name, bad) in [("qty", "two"), ("qty", "01"), ("qty", "1."), ("ok", "yes"), ("none", "x")] {
        assert!(g.set_value(JSON, &row(name), bad).is_err(), "{name} = {bad:?}");
    }
    // The type column: any scalar.
    assert_eq!(apply(JSON, &g.set_literal(JSON, &row("none"), "\"now text\"").expect("edits")), JSON.replace("null", "\"now text\""));
    assert!(g.set_literal(JSON, &row("none"), "nope").is_err());
    assert!(g.set_literal(JSON, &row("tags"), "1").is_err(), "an array is not a value");
}

#[test]
fn a_json_key_is_renamed_and_a_member_is_removed_with_its_comma() {
    let g = Grid::read(JSON, Family::Json).expect("reads");
    let open = all_open(&g);
    let row = |name: &str| g.rows(&open).into_iter().find(|r| r.name == name).expect("a row").path;
    assert_eq!(apply(JSON, &g.rename(JSON, &row("qty"), "quantity \"n\"").expect("edits")), JSON.replace("\"qty\"", "\"quantity \\\"n\\\"\""));
    // Not the last: its comma and the white space after it go.
    let without_qty = apply(JSON, &g.remove(JSON, &row("qty")).expect("edits"));
    assert_eq!(without_qty, "{\n  \"name\": \"Tea\",\n  \"tags\": [\"a\", \"b\"],\n  \"ok\": true,\n  \"none\": null\n}");
    // The last: the comma before it goes.
    let without_none = apply(JSON, &g.remove(JSON, &row("none")).expect("edits"));
    assert_eq!(without_none, "{\n  \"name\": \"Tea\",\n  \"qty\": 2,\n  \"tags\": [\"a\", \"b\"],\n  \"ok\": true\n}");
    // An array item.
    let item = g.rows(&open).into_iter().find(|r| r.name == "[0]").expect("a row").path;
    assert_eq!(apply(JSON, &g.remove(JSON, &item).expect("edits")), JSON.replace("[\"a\", \"b\"]", "[\"b\"]"));
    // Whatever is removed, what is left is valid JSON that reads again.
    for result in [&without_qty, &without_none] {
        assert!(Grid::read(result, Family::Json).is_ok(), "{result}");
    }
    // Rows that are not members have no name to change, and the root is not removed.
    assert!(g.rename(JSON, &item, "x").is_err());
    assert!(g.remove(JSON, &[]).is_err());
}

const XML: &str = "<order id=\"7\" note='a b'><item n=\"1\">Tea</item><item n=\"2\"/><!-- done --><empty></empty></order>";

#[test]
fn xml_rows_show_attributes_elements_text_and_comments_and_an_element_with_only_text_shows_it_in_its_own_row() {
    let g = Grid::read(XML, Family::Xml).expect("reads");
    let open = all_open(&g);
    let rows = names(&g, &open);
    let expected: Vec<(usize, &str, &str)> = vec![
        (0, "order", ""),
        (1, "id", "7"),
        (1, "note", "a b"),
        (1, "item", "Tea"),
        (2, "n", "1"),
        (1, "item", ""),
        (2, "n", "2"),
        (1, "#comment", " done "),
        (1, "empty", ""),
    ];
    assert_eq!(rows.iter().map(|(d, n, v)| (*d, n.as_str(), v.as_str())).collect::<Vec<_>>(), expected, "{rows:?}");
}

#[test]
fn xml_values_are_replaced_in_place_and_escaped() {
    let g = Grid::read(XML, Family::Xml).expect("reads");
    let open = all_open(&g);
    let rows = g.rows(&open);
    let at = |depth: usize, name: &str, value: &str| {
        rows.iter().find(|r| r.depth == depth && r.name == name && r.value == value).expect("a row").path.clone()
    };
    // An attribute, in either kind of quote, with its own quote escaped.
    assert_eq!(apply(XML, &g.set_value(XML, &at(1, "id", "7"), "a\"b&c<d").expect("edits")), XML.replace("id=\"7\"", "id=\"a&quot;b&amp;c&lt;d\""));
    assert_eq!(apply(XML, &g.set_value(XML, &at(1, "note", "a b"), "it's").expect("edits")), XML.replace("note='a b'", "note='it&apos;s'"));
    // The text of an element with only text, and the value of an empty element (self-closed or not).
    assert_eq!(apply(XML, &g.set_value(XML, &at(1, "item", "Tea"), "Milk & <oat>").expect("edits")), XML.replace("Tea", "Milk &amp; &lt;oat&gt;"));
    assert_eq!(apply(XML, &g.set_value(XML, &at(1, "item", ""), "new").expect("edits")), XML.replace("<item n=\"2\"/>", "<item n=\"2\">new</item>"));
    assert_eq!(apply(XML, &g.set_value(XML, &at(1, "empty", ""), "x").expect("edits")), XML.replace("<empty></empty>", "<empty>x</empty>"));
    // A comment.
    assert_eq!(
        apply(XML, &g.set_value(XML, &at(1, "#comment", " done "), " later ").expect("edits")),
        XML.replace("<!-- done -->", "<!-- later -->")
    );
    assert!(g.set_value(XML, &at(1, "#comment", " done "), "a--b").is_err());
    assert!(g.set_value(XML, &at(1, "#comment", " done "), "ends-").is_err());
    // An element that holds elements has no value.
    assert!(g.set_value(XML, &[0], "x").is_err());
    // Every result reads again.
    for edited in [
        apply(XML, &g.set_value(XML, &at(1, "id", "7"), "a\"b&c<d").expect("edits")),
        apply(XML, &g.set_value(XML, &at(1, "item", "Tea"), "Milk & <oat>").expect("edits")),
    ] {
        assert!(Grid::read(&edited, Family::Xml).is_ok(), "{edited}");
    }
}

#[test]
fn an_xml_name_is_changed_in_the_start_and_the_end_tag_and_what_is_no_name_is_refused() {
    let g = Grid::read(XML, Family::Xml).expect("reads");
    let open = all_open(&g);
    let rows = g.rows(&open);
    let item = rows.iter().find(|r| r.depth == 1 && r.name == "item" && r.value == "Tea").expect("a row").path.clone();
    assert_eq!(apply(XML, &g.rename(XML, &item, "line").expect("edits")), XML.replace("<item n=\"1\">Tea</item>", "<line n=\"1\">Tea</line>"));
    let selfclosed = rows.iter().find(|r| r.depth == 1 && r.name == "item" && r.value.is_empty()).expect("a row").path.clone();
    assert_eq!(apply(XML, &g.rename(XML, &selfclosed, "x:y").expect("edits")), XML.replace("<item n=\"2\"/>", "<x:y n=\"2\"/>"));
    let attribute = rows.iter().find(|r| r.name == "id").expect("a row").path.clone();
    assert_eq!(apply(XML, &g.rename(XML, &attribute, "key").expect("edits")), XML.replace("id=", "key="));
    for bad in ["", "1a", "a b", "a<b", "-x"] {
        assert!(g.rename(XML, &item, bad).is_err(), "{bad:?}");
    }
    // Removing: an attribute takes the white space before it, an element goes whole.
    assert_eq!(apply(XML, &g.remove(XML, &attribute).expect("edits")), XML.replace(" id=\"7\"", ""));
    assert_eq!(apply(XML, &g.remove(XML, &item).expect("edits")), XML.replace("<item n=\"1\">Tea</item>", ""));
}

const YAML: &str = "name: Tea\nqty: 2\ntags:\n  - a\n  - b\nok: true\nnote: \"quoted\"\n";

#[test]
fn yaml_rows_and_edits_write_plain_scalars_where_they_read_back_the_same_and_quote_the_rest() {
    let g = Grid::read(YAML, Family::Yaml).expect("reads");
    let open = all_open(&g);
    let rows = g.rows(&open);
    let row = |name: &str| rows.iter().find(|r| r.name == name).expect("a row").path.clone();
    assert_eq!(rows.iter().find(|r| r.name == "tags").map(|r| r.value.as_str()), Some("[2]"));
    assert_eq!(apply(YAML, &g.set_value(YAML, &row("name"), "Chai latte").expect("edits")), YAML.replace("Tea", "Chai latte"));
    // A string that would read as something else is quoted.
    for (typed, written) in [
        ("true", "\"true\""),
        ("12", "\"12\""),
        ("a: b", "\"a: b\""),
        ("- x", "\"- x\""),
        ("#c", "\"#c\""),
        ("null", "\"null\""),
        (" pad", "\"\\u0020pad\""),
    ] {
        let edited = apply(YAML, &g.set_value(YAML, &row("name"), typed).expect("edits"));
        if typed == " pad" {
            assert!(edited.contains("name: \" pad\"") || edited.contains("name: \"\\u0020pad\"") || edited.contains("name: \" pad\""), "{edited}");
        } else {
            assert!(edited.starts_with(&format!("name: {written}\n")), "{typed:?} → {edited}");
        }
        assert!(Grid::read(&edited, Family::Yaml).is_ok(), "{edited}");
    }
    assert_eq!(apply(YAML, &g.set_value(YAML, &row("qty"), "0x1F").expect("edits")), YAML.replace("qty: 2", "qty: 0x1F"));
    assert!(g.set_value(YAML, &row("qty"), "two").is_err());
    assert!(g.set_value(YAML, &row("name"), "two\nlines").is_err(), "a string with line breaks is edited in the text");
    // A member and a list item are removed with their lines.
    assert_eq!(apply(YAML, &g.remove(YAML, &row("qty")).expect("edits")), YAML.replace("qty: 2\n", ""));
    let item = rows.iter().find(|r| r.name == "[0]").expect("a row").path.clone();
    assert_eq!(apply(YAML, &g.remove(YAML, &item).expect("edits")), YAML.replace("  - a\n", ""));
    assert_eq!(apply(YAML, &g.rename(YAML, &row("ok"), "fine").expect("edits")), YAML.replace("ok:", "fine:"));
}

#[test]
fn a_block_scalar_and_a_flow_collection_are_edited_in_the_text() {
    let y = "text: |\n  one\n  two\nflow: {a: 1, b: 2}\n";
    let g = Grid::read(y, Family::Yaml).expect("reads");
    let open = all_open(&g);
    let rows = g.rows(&open);
    let text = rows.iter().find(|r| r.name == "text").expect("a row").path.clone();
    assert!(g.set_value(y, &text, "x").is_err());
    let a = rows.iter().find(|r| r.name == "a").expect("a row").path.clone();
    assert!(g.remove(y, &a).is_err(), "a row in flow style is removed in the text");
}

#[test]
fn an_edit_made_on_a_text_that_changed_since_is_refused_not_applied_somewhere_else() {
    let g = Grid::read(JSON, Family::Json).expect("reads");
    let open = all_open(&g);
    let name = g.rows(&open).into_iter().find(|r| r.name == "name").expect("a row").path;
    assert!(g.set_value("{}", &name, "x").is_err(), "the place is gone");
    assert!(g.set_value("", &name, "x").is_err());
    assert!(g.set_value(JSON, &[9, 9], "x").is_err(), "the row is gone");
}

#[test]
fn text_that_is_not_the_format_is_refused_and_the_rows_of_a_large_document_are_capped() {
    assert!(Grid::read("{", Family::Json).is_err());
    assert!(Grid::read("<a>", Family::Xml).is_err());
    let big = format!("[{}]", (0..150_000).map(|i| i.to_string()).collect::<Vec<_>>().join(","));
    let g = Grid::read(&big, Family::Json).expect("reads");
    let open = all_open(&g);
    assert_eq!(g.rows(&open).len(), xmlcraft_grid::MAX_ROWS);
}

proptest::proptest! {
    #[test]
    fn any_text_gives_a_grid_or_an_error_and_any_edit_on_it_is_an_edit_or_an_error(text in ".{0,200}", typed in ".{0,20}") {
        for family in [Family::Xml, Family::Json, Family::Yaml] {
            if let Ok(g) = Grid::read(&text, family) {
                let open = all_open(&g);
                for row in g.rows(&open).into_iter().take(20) {
                    let _ = g.set_value(&text, &row.path, &typed);
                    let _ = g.rename(&text, &row.path, &typed);
                    let _ = g.remove(&text, &row.path);
                    let _ = g.set_literal(&text, &row.path, &typed);
                }
            }
        }
    }
}
