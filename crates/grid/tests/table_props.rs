//! Properties of table edits: after any sequence of them the document reads again in its format, the rows no edit named keep their bytes,
//! and the table of the result shows the edit. For JSON (compact and pretty), JSON Lines, YAML block style and XML.

use proptest::prelude::*;
use xmlcraft_diff::Family;
use xmlcraft_grid::{CellKind, Flavor, Grid, Table};
use xmlcraft_text::apply_edits;

#[derive(Clone, Copy, Debug)]
enum Style {
    JsonCompact,
    JsonPretty,
    JsonLines,
    Yaml,
    Xml,
}

impl Style {
    fn family(self) -> Family {
        match self {
            Style::Yaml => Family::Yaml,
            Style::Xml => Family::Xml,
            _ => Family::Json,
        }
    }
    fn flavor(self) -> Flavor {
        if let Style::JsonLines = self { Flavor::JsonLines } else { Flavor::Plain }
    }
    fn path(self) -> Vec<usize> {
        match self {
            Style::JsonCompact | Style::JsonPretty | Style::JsonLines => vec![],
            Style::Yaml => vec![0],
            Style::Xml => vec![0],
        }
    }
}

const KEYS: [&str; 4] = ["a", "b", "c", "d"];

/// A document of `rows` (each a list of which keys it has, with values), written in `style`. Every row has `a`.
fn document(style: Style, rows: &[Vec<(usize, String)>]) -> String {
    let json_value = |v: &str| if v.chars().all(|c| c.is_ascii_digit()) { v.to_owned() } else { format!("\"{v}\"") };
    match style {
        Style::JsonCompact => {
            let items: Vec<String> = rows
                .iter()
                .map(|r| format!("{{{}}}", r.iter().map(|(k, v)| format!("\"{}\": {}", KEYS[*k], json_value(v))).collect::<Vec<_>>().join(", ")))
                .collect();
            format!("[{}]", items.join(", "))
        }
        Style::JsonPretty => {
            let items: Vec<String> = rows
                .iter()
                .map(|r| {
                    let members: Vec<String> = r.iter().map(|(k, v)| format!("      \"{}\": {}", KEYS[*k], json_value(v))).collect();
                    format!("    {{\n{}\n    }}", members.join(",\n"))
                })
                .collect();
            format!("[\n{}\n]\n", items.join(",\n"))
        }
        Style::JsonLines => rows
            .iter()
            .map(|r| format!("{{{}}}\n", r.iter().map(|(k, v)| format!("\"{}\": {}", KEYS[*k], json_value(v))).collect::<Vec<_>>().join(", ")))
            .collect(),
        Style::Yaml => {
            let mut out = String::from("items:\n");
            for r in rows {
                for (i, (k, v)) in r.iter().enumerate() {
                    out.push_str(if i == 0 { "  - " } else { "    " });
                    out.push_str(&format!("{}: {v}\n", KEYS[*k]));
                }
            }
            out.push_str("end: 1\n");
            out
        }
        Style::Xml => {
            let mut out = String::from("<root>\n");
            for r in rows {
                // `a` is an attribute, the others are elements.
                let a = r.iter().find(|(k, _)| *k == 0).map(|(_, v)| v.clone()).unwrap_or_default();
                out.push_str(&format!("  <row a=\"{a}\">\n"));
                for (k, v) in r.iter().filter(|(k, _)| *k != 0) {
                    out.push_str(&format!("    <{0}>{v}</{0}>\n", KEYS[*k]));
                }
                out.push_str("  </row>\n");
            }
            out.push_str("  <other/>\n</root>\n");
            out
        }
    }
}

#[derive(Clone, Debug)]
enum Op {
    Set(usize, usize, String),
    Clear(usize, usize),
    Insert(Option<usize>),
    Duplicate(usize),
    Delete(usize),
    Move(usize, Option<usize>),
    Order(Vec<usize>),
    Rename(usize),
    Paste(usize, usize, String, String),
}

fn value() -> impl Strategy<Value = String> {
    prop_oneof!["[a-z]{1,6}", "[1-9][0-9]{0,3}"]
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (0..8usize, 0..6usize, value()).prop_map(|(r, c, v)| Op::Set(r, c, v)),
        1 => (0..8usize, 0..6usize).prop_map(|(r, c)| Op::Clear(r, c)),
        1 => proptest::option::of(0..8usize).prop_map(Op::Insert),
        1 => (0..8usize).prop_map(Op::Duplicate),
        1 => (0..8usize).prop_map(Op::Delete),
        1 => (0..8usize, proptest::option::of(0..8usize)).prop_map(|(r, b)| Op::Move(r, b)),
        1 => proptest::collection::vec(0..1000usize, 8).prop_map(Op::Order),
        1 => (0..6usize).prop_map(Op::Rename),
        1 => (0..8usize, 0..6usize, value(), value()).prop_map(|(r, c, a, b)| Op::Paste(r, c, a, b)),
    ]
}

fn rows() -> impl Strategy<Value = Vec<Vec<(usize, String)>>> {
    proptest::collection::vec(
        (value(), proptest::collection::vec((1..4usize, value()), 0..3)).prop_map(|(a, rest)| {
            let mut row = vec![(0usize, a)];
            for (k, v) in rest {
                if !row.iter().any(|(x, _)| *x == k) {
                    row.push((k, v));
                }
            }
            row.sort_by_key(|(k, _)| *k);
            row
        }),
        2..6,
    )
}

/// The text of every row, in order.
fn row_texts(text: &str, t: &Table) -> Vec<String> {
    t.rows.iter().map(|r| text.get(r.span.clone()).unwrap_or_default().trim_end().to_owned()).collect()
}

fn read(text: &str, style: Style) -> Result<(Grid, Table), TestCaseError> {
    let g = Grid::read_flavored(text, style.family(), style.flavor()).map_err(|e| TestCaseError::fail(format!("{e}: {text}")))?;
    let name = if let Style::Xml = style { Some("row") } else { None };
    let t = g.table_named(text, &style.path(), name).ok_or_else(|| TestCaseError::fail(format!("no table in {text}")))?;
    Ok((g, t))
}

fn cell_text(t: &Table, row: usize, label: &str) -> Option<(CellKind, String)> {
    let c = t.column_index(label)?;
    t.rows.get(row)?.cells.get(c).map(|c| (c.kind, c.text.clone()))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, .. ProptestConfig::default() })]

    #[test]
    fn any_sequence_of_table_edits_keeps_the_document_readable_and_the_other_rows_as_they_were(
        style in prop_oneof![Just(Style::JsonCompact), Just(Style::JsonPretty), Just(Style::JsonLines), Just(Style::Yaml), Just(Style::Xml)],
        start in rows(),
        ops in proptest::collection::vec(op(), 1..7),
    ) {
        let mut text = document(style, &start);
        let mut fresh = 0;
        for op in ops {
            let (g, t) = read(&text, style)?;
            let n = t.rows.len();
            let cols = t.columns.len();
            if cols == 0 {
                // Objects with no members left: nothing to set.
                break;
            }
            let before = row_texts(&text, &t);
            let label = |c: usize| t.columns.get(c % cols).map(|c| c.label()).unwrap_or_default();
            let result = match &op {
                Op::Set(r, c, v) => g.set_cell(&text, &t, r % n, c % cols, v),
                Op::Clear(r, c) => g.clear_cell(&text, &t, r % n, c % cols),
                Op::Insert(a) => g.insert_row(&text, &t, a.map(|a| a % n)),
                Op::Duplicate(r) => g.duplicate_row(&text, &t, r % n),
                Op::Delete(r) if n > 1 => g.delete_rows(&text, &t, &[r % n]),
                Op::Delete(_) => continue,
                Op::Move(r, b) => g.move_row(&text, &t, r % n, b.map(|b| b % n)),
                Op::Order(keys) => {
                    let mut order: Vec<usize> = (0..n).collect();
                    order.sort_by_key(|&i| keys.get(i).copied().unwrap_or(0));
                    g.apply_order(&text, &t, &order)
                }
                Op::Rename(c) => {
                    fresh += 1;
                    let name = format!("{}n{fresh}", if t.columns.get(c % cols).is_some_and(|c| c.label().starts_with('@')) { "@" } else { "" });
                    g.rename_column(&text, &t, c % cols, &name)
                }
                Op::Paste(r, c, a, b) => g.paste_tsv(&text, &t, r % n, c % cols, &format!("{a}\t{b}\n")).map(|p| p.edits),
            };
            // A refusal is allowed (an element into an item that holds text, a nested cell); it must leave nothing half-done.
            let Ok(edits) = result else { continue };
            let out = apply_edits(&text, &edits);
            let (_, t2) = read(&out, style)?;
            let after = row_texts(&out, &t2);
            match &op {
                Op::Set(r, c, v) => {
                    let (r, c) = (r % n, c % cols);
                    let shown = cell_text(&t2, r, &label(c));
                    if t.rows[r].cells[c].kind != CellKind::Nested {
                        prop_assert_eq!(shown, Some((CellKind::Scalar, v.clone())), "{} → {}", text, out);
                    }
                    for i in (0..n).filter(|i| *i != r) {
                        prop_assert_eq!(&before[i], &after[i], "row {} changed: {} → {}", i, text, out);
                    }
                }
                Op::Clear(r, c) => {
                    let (r, c) = (r % n, c % cols);
                    let shown = cell_text(&t2, r, &label(c));
                    prop_assert!(shown.is_none() || shown.as_ref().is_some_and(|s| s.0 == CellKind::Missing), "{:?}: {} → {}", shown, text, out);
                    for i in (0..n).filter(|i| *i != r) {
                        prop_assert_eq!(&before[i], &after[i], "row {} changed", i);
                    }
                }
                Op::Insert(a) => {
                    prop_assert_eq!(after.len(), n + 1);
                    let at = a.map_or(0, |a| a % n + 1);
                    let mut expected = before.clone();
                    expected.insert(at, after[at].clone());
                    prop_assert_eq!(&expected, &after, "{} → {}", text, out);
                }
                Op::Duplicate(r) => {
                    let mut expected = before.clone();
                    expected.insert(r % n + 1, before[r % n].clone());
                    prop_assert_eq!(&expected, &after, "{} → {}", text, out);
                }
                Op::Delete(r) => {
                    let mut expected = before.clone();
                    expected.remove(r % n);
                    prop_assert_eq!(&expected, &after, "{} → {}", text, out);
                }
                Op::Move(..) | Op::Order(_) => {
                    let (mut x, mut y) = (before.clone(), after.clone());
                    x.sort();
                    y.sort();
                    prop_assert_eq!(x, y, "the same rows: {} → {}", text, out);
                }
                Op::Rename(c) => {
                    prop_assert!(t2.column_index(&label(*c)).is_none(), "{} → {}", text, out);
                }
                Op::Paste(..) => prop_assert!(after.len() >= n),
            }
            text = out;
        }
    }

    #[test]
    fn any_text_gives_a_table_or_none_and_any_edit_of_it_is_edits_or_an_error(text in ".{0,120}", typed in ".{0,12}") {
        for (family, flavor) in [(Family::Xml, Flavor::Plain), (Family::Json, Flavor::Plain), (Family::Json, Flavor::JsonLines), (Family::Yaml, Flavor::Plain), (Family::Json, Flavor::ExtendedJson)] {
            let Ok(g) = Grid::read_flavored(&text, family, flavor) else { continue };
            let mut paths = vec![Vec::new()];
            paths.extend(g.expandable());
            for p in paths.into_iter().take(10) {
                let Some(t) = g.table(&text, &p) else { continue };
                let _ = g.table_path_at(&text, text.len() / 2);
                let _ = t.sorted(0, true);
                let _ = t.filtered(&typed);
                for r in 0..t.rows.len().min(3) {
                    for c in 0..t.columns.len().min(3) {
                        let _ = g.set_cell(&text, &t, r, c, &typed);
                        let _ = g.set_cell_literal(&text, &t, r, c, &typed);
                        let _ = g.clear_cell(&text, &t, r, c);
                    }
                    let _ = g.insert_row(&text, &t, Some(r));
                    let _ = g.duplicate_row(&text, &t, r);
                    let _ = g.delete_rows(&text, &t, &[r]);
                    let _ = g.move_row(&text, &t, r, None);
                }
                let _ = g.insert_row(&text, &t, None);
                let _ = g.rename_column(&text, &t, 0, &typed);
                let _ = g.delete_column(&text, &t, 0);
                let _ = g.insert_column(&text, &t, &typed, None);
                let _ = g.paste_tsv(&text, &t, 0, 0, &typed);
                let _ = t.to_tsv(&[0], &[0]);
            }
        }
    }
}
