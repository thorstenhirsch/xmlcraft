//! Comparing lines, trees and three-way merges.

use proptest::prelude::*;
use xmlcraft_diff::{
    ChangeKind, Choice, Family, HunkKind, LineOptions, Node, Region, TreeOptions, Whitespace, compare_documents, compare_mixed, diff_lines,
    diff_words, merge3, split_lines,
};

fn changes(old: &str, new: &str, family: Family) -> Vec<String> {
    compare_documents(old, new, family, &TreeOptions::default()).expect("reads").iter().map(|c| c.describe()).collect()
}

#[test]
fn lines_are_split_on_all_three_breaks_and_compared() {
    let spans = split_lines("a\r\nb\rc\nd");
    assert_eq!(spans, [0..1, 3..4, 5..6, 7..8]);
    assert!(split_lines("").is_empty());
    assert_eq!(split_lines("\n").len(), 1, "an empty line");
    let d = diff_lines("a\nb\nc\nd\n", "a\nB\nc\nd\ne\n", &LineOptions::default());
    let kinds: Vec<_> = d.hunks.iter().map(|h| (h.kind(), h.old.clone(), h.new.clone())).collect();
    assert_eq!(kinds, [(HunkKind::Changed, 1..2, 1..2), (HunkKind::Added, 4..4, 4..5)]);
    assert_eq!(d.same, [(0, 0), (2, 2), (3, 3)]);
    assert!(diff_lines("x\ny\n", "x\ny\n", &LineOptions::default()).is_identical());
    // Line endings are not part of the comparison.
    assert!(diff_lines("a\r\nb\r\n", "a\nb\n", &LineOptions::default()).is_identical());
}

#[test]
fn white_space_case_and_blank_lines_can_be_ignored() {
    let old = "  Hello   World\n\nend\n";
    let new = "hello world\nend\n";
    assert!(!diff_lines(old, new, &LineOptions::default()).is_identical());
    let o = LineOptions { whitespace: Whitespace::Collapse, ignore_case: true, ignore_blank_lines: true, ..LineOptions::default() };
    assert!(diff_lines(old, new, &o).is_identical());
    let ends = LineOptions { whitespace: Whitespace::Ends, ..LineOptions::default() };
    assert!(diff_lines("a \n", "a\n", &ends).is_identical());
    assert!(!diff_lines("a b\n", "ab\n", &ends).is_identical());
    assert!(diff_lines("a b\n", "ab\n", &LineOptions { whitespace: Whitespace::All, ..LineOptions::default() }).is_identical());
}

#[test]
fn the_spans_of_a_hunk_are_the_bytes_of_its_lines() {
    let (old, new) = ("one\ntwo\nthree\n", "one\n2\nthree\nfour\n");
    let d = diff_lines(old, new, &LineOptions::default());
    let first = &d.hunks[0];
    assert_eq!(&old[d.old_span(first)], "two");
    assert_eq!(&new[d.new_span(first)], "2");
    let added = &d.hunks[1];
    assert_eq!(d.old_span(added), 14..14, "an addition has an empty place in the first text");
    assert_eq!(&new[d.new_span(added)], "four");
}

#[test]
fn a_unified_diff_has_hunks_with_context() {
    let old = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";
    let new = "1\n2\n3\nFOUR\n5\n6\n7\n8\n9\nTEN\n";
    let d = diff_lines(old, new, &LineOptions::default());
    let text = d.unified(old, new, "a", "b", 1);
    assert_eq!(text, "--- a\n+++ b\n@@ -3,3 +3,3 @@\n 3\n-4\n+FOUR\n 5\n@@ -9,2 +9,2 @@\n 9\n-10\n+TEN\n");
    assert!(diff_lines("a\n", "a\n", &LineOptions::default()).unified("a\n", "a\n", "a", "b", 3).is_empty());
}

#[test]
fn words_that_differ_are_found_in_both_lines() {
    let (a, b) = diff_words("let x = 10;", "let y = 10;");
    assert_eq!((&"let x = 10;"[a[0].clone()], &"let y = 10;"[b[0].clone()]), ("x", "y"));
    let (a, b) = diff_words("same", "same");
    assert!(a.is_empty() && b.is_empty());
    let (a, b) = diff_words("", "new words");
    assert!(a.is_empty());
    assert_eq!((b.len(), b[0].clone()), (1, 0..9));
}

#[test]
fn xml_differences_are_found_by_element_attribute_and_text() {
    let old = r#"<order id="1"><item sku="a">Tea</item><item sku="b">Milk</item><note>x</note></order>"#;
    let new = r#"<order id="2"><item sku="a">Tea</item><item sku="b">Cream</item><extra/></order>"#;
    assert_eq!(
        changes(old, new, Family::Xml),
        [
            "changed /order[1]/@id: 1 → 2",
            "changed /order[1]/item[2]/text()[1]: Milk → Cream",
            "removed /order[1]/note[1]: <note>",
            "added /order[1]/extra[1]: <extra>",
        ]
        .map(String::from)
    );
    assert!(changes(old, old, Family::Xml).is_empty());
}

#[test]
fn white_space_comments_order_and_prefixes_are_options_in_xml() {
    let old = "<r>\n  <a>x</a>\n  <b>y</b> <!-- c -->\n</r>";
    let new = "<r><b>y</b><a> x </a></r>";
    assert!(!compare_documents(old, new, Family::Xml, &TreeOptions::default()).expect("reads").is_empty());
    let o = TreeOptions { whitespace: Whitespace::Collapse, ignore_comments: true, ignore_child_order: true, ..TreeOptions::default() };
    assert!(compare_documents(old, new, Family::Xml, &o).expect("reads").is_empty());
    let by_prefix = TreeOptions { ignore_prefixes: true, ..TreeOptions::default() };
    let (p, q) = ("<p:a xmlns:p=\"u\" p:k=\"1\"><p:b/></p:a>", "<q:a xmlns:q=\"u\" q:k=\"1\"><q:b/></q:a>");
    assert!(!compare_documents(p, q, Family::Xml, &TreeOptions::default()).expect("reads").is_empty());
    assert!(compare_documents(p, q, Family::Xml, &by_prefix).expect("reads").is_empty(), "the same names in the same namespace");
    let other = "<q:a xmlns:q=\"v\" q:k=\"1\"><q:b/></q:a>";
    assert!(!compare_documents(p, other, Family::Xml, &by_prefix).expect("reads").is_empty(), "another namespace is a difference");
    let case = compare_documents("<a>Hi</a>", "<a>hI</a>", Family::Xml, &TreeOptions { ignore_case: true, ..TreeOptions::default() }).expect("reads");
    assert!(case.is_empty());
}

#[test]
fn json_and_yaml_differences_are_json_pointers() {
    let old = r#"{"name": "x", "tags": ["a", "b", "c"], "n": {"k": 1, "gone": true}}"#;
    let new = r#"{"tags": ["a", "c", "d"], "name": "y", "n": {"k": 1.0, "new": null}}"#;
    let found = changes(old, new, Family::Json);
    assert!(found.contains(&"changed /name: \"x\" → \"y\"".to_owned()), "{found:?}");
    assert!(found.contains(&"removed /tags/1: \"b\"".to_owned()), "{found:?}");
    assert!(found.contains(&"added /tags/2: \"d\"".to_owned()), "{found:?}");
    assert!(found.contains(&"removed /n/gone: true".to_owned()) && found.contains(&"added /n/new: null".to_owned()), "{found:?}");
    assert!(found.contains(&"changed /n/k: 1 → 1.0".to_owned()), "{found:?}");
    // The order of the members is not a difference; 1 and 1.0 are the same number when asked.
    let same = compare_documents(
        r#"{"a": 1, "b": [1, 2]}"#,
        r#"{"b": [1, 2], "a": 1.0}"#,
        Family::Json,
        &TreeOptions { numbers_by_value: true, ..TreeOptions::default() },
    )
    .expect("reads");
    assert!(same.is_empty(), "{same:?}");
    // YAML reads into the same trees, so a JSON and a YAML text of the same data have no difference.
    let across =
        compare_mixed("{\"a\": 1, \"b\": [\"x\", \"y\"]}", Family::Json, "b: [x, y]\na: 1\n", Family::Yaml, &TreeOptions::default()).expect("reads");
    assert!(across.is_empty(), "{across:?}");
    let y = compare_documents("a: 1\nb: [x, y]\n", "a: 1\nb: [x, z]\n", Family::Yaml, &TreeOptions::default()).expect("reads");
    assert_eq!(y.iter().map(|c| (c.kind, c.path.as_str())).collect::<Vec<_>>(), [(ChangeKind::Changed, "/b/1")]);
    assert!(compare_documents("{\"a\": ", "{}", Family::Json, &TreeOptions::default()).is_err());
    assert!(compare_documents("<a>", "<a/>", Family::Xml, &TreeOptions::default()).is_err());
}

#[test]
fn a_change_knows_its_places_in_both_texts() {
    let (old, new) = (r#"{"a": {"b": 1}, "c": 2}"#, r#"{"a": {"b": 3}, "d": 4}"#);
    let found = compare_documents(old, new, Family::Json, &TreeOptions::default()).expect("reads");
    let changed = found.iter().find(|c| c.kind == ChangeKind::Changed).expect("a change");
    assert_eq!((&old[changed.old.clone().expect("old")], &new[changed.new.clone().expect("new")]), ("1", "3"));
    let removed = found.iter().find(|c| c.kind == ChangeKind::Removed).expect("a removal");
    assert_eq!(&old[removed.old.clone().expect("old")], "2");
    assert_eq!(&new[removed.new_parent.clone()], new, "it came out of the root");
    let added = found.iter().find(|c| c.kind == ChangeKind::Added).expect("an addition");
    assert_eq!((&new[added.new.clone().expect("new")], added.old_parent.clone()), ("4", 0..old.len()));
}

#[test]
fn a_kind_that_changed_is_one_change() {
    assert_eq!(changes(r#"{"a": [1]}"#, r#"{"a": {"b": 1}}"#, Family::Json), ["changed /a: [five] → {…}".replace("five", "…")]);
    assert_eq!(changes(r#"{"a": 1}"#, r#"{"a": "1"}"#, Family::Json), ["changed /a: 1 → \"1\""]);
}

#[test]
fn three_way_merges_keep_both_sets_of_changes() {
    let base = "a\nb\nc\nd\ne\n";
    let m = merge3(base, "a\nB\nc\nd\ne\n", "a\nb\nc\nd\nE\n", &LineOptions::default());
    assert_eq!(m.conflict_count(), 0);
    assert_eq!(m.render(Choice::Markers), "a\nB\nc\nd\nE\n");
    // The same change on both sides is no conflict.
    let m = merge3(base, "a\nX\nc\nd\ne\n", "a\nX\nc\nd\ne\n", &LineOptions::default());
    assert_eq!((m.conflict_count(), m.render(Choice::Base)), (0, "a\nX\nc\nd\ne\n".to_owned()));
    // Different changes to the same line are a conflict, settled as asked.
    let m = merge3(base, "a\nOURS\nc\nd\ne\n", "a\nTHEIRS\nc\nd\ne\n", &LineOptions::default());
    assert_eq!(m.conflict_count(), 1);
    assert_eq!(m.render(Choice::Ours), "a\nOURS\nc\nd\ne\n");
    assert_eq!(m.render(Choice::Theirs), "a\nTHEIRS\nc\nd\ne\n");
    assert_eq!(m.render(Choice::Both), "a\nOURS\nTHEIRS\nc\nd\ne\n");
    assert_eq!(m.render(Choice::Base), base);
    assert_eq!(m.render(Choice::Markers), "a\n<<<<<<< ours\nOURS\n||||||| base\nb\n=======\nTHEIRS\n>>>>>>> theirs\nc\nd\ne\n");
    // A change against a deletion of the same line is a conflict; the deletion alone is not.
    assert_eq!(merge3(base, "a\nc\nd\ne\n", base, &LineOptions::default()).render(Choice::Markers), "a\nc\nd\ne\n");
    assert_eq!(merge3(base, "a\nBB\nc\nd\ne\n", "a\nc\nd\ne\n", &LineOptions::default()).conflict_count(), 1);
    // Each conflict can be settled on its own.
    let two = merge3("1\n2\n3\n4\n5\n", "1\nA\n3\nC\n5\n", "1\nB\n3\nD\n5\n", &LineOptions::default());
    assert_eq!(two.conflict_count(), 2);
    assert_eq!(two.render_with(&|i| if i == 0 { Choice::Ours } else { Choice::Theirs }), "1\nA\n3\nD\n5\n");
    // A last line without a break is kept as it is.
    assert_eq!(merge3("a\nb", "a\nB", "a\nb", &LineOptions::default()).render(Choice::Markers), "a\nB");
}

/// Whether every `@@ -a,b +c,d @@` block has b lines of the old side (context and `-`) and d lines of the new (context and `+`).
fn counts_agree(unified: &str) -> bool {
    let mut blocks: Vec<(usize, usize, usize, usize)> = Vec::new();
    for line in unified.lines().skip(2) {
        if let Some(head) = line.strip_prefix("@@ -") {
            let numbers: Vec<usize> =
                head.split(|c: char| !c.is_ascii_digit()).filter(|p| !p.is_empty()).map(|p| p.parse().expect("number")).collect();
            blocks.push((numbers[1], numbers[3], 0, 0));
        } else if let Some(last) = blocks.last_mut() {
            match line.chars().next() {
                Some(' ') => {
                    last.2 += 1;
                    last.3 += 1;
                }
                Some('-') => last.2 += 1,
                Some('+') => last.3 += 1,
                _ => {}
            }
        }
    }
    blocks.iter().all(|b| b.0 == b.2 && b.1 == b.3)
}

fn arb_json() -> impl Strategy<Value = String> {
    let leaf = prop_oneof![
        Just("null".to_owned()),
        Just("true".to_owned()),
        (-5i32..5).prop_map(|n| n.to_string()),
        "[a-c ]{0,3}".prop_map(|s| serde_json::to_string(&s).unwrap_or_default()),
    ];
    leaf.prop_recursive(3, 20, 4, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(|v| format!("[{}]", v.join(","))),
            proptest::collection::vec(("[a-c]{1,2}", inner), 0..4).prop_map(|v| {
                let mut seen = std::collections::HashSet::new();
                let members: Vec<String> = v
                    .into_iter()
                    .filter(|(k, _)| seen.insert(k.clone()))
                    .map(|(k, x)| format!("{}:{}", serde_json::to_string(&k).unwrap_or_default(), x))
                    .collect();
                format!("{{{}}}", members.join(","))
            }),
        ]
    })
}

fn arb_xml() -> impl Strategy<Value = String> {
    let leaf = prop_oneof![Just(String::new()), "[a-c ]{1,3}"];
    let element = leaf.prop_recursive(3, 12, 3, |inner| {
        ("[a-c]", proptest::collection::vec(("[x-z]", "[a-c]{0,2}"), 0..3), proptest::collection::vec(inner, 0..3)).prop_map(
            |(name, attributes, children)| {
                let mut seen = std::collections::HashSet::new();
                let attrs: String = attributes.into_iter().filter(|(k, _)| seen.insert(k.clone())).map(|(k, v)| format!(" {k}=\"{v}\"")).collect();
                let content: String = children.into_iter().map(|c| if c.starts_with('<') || c.is_empty() { c } else { c.replace('<', "") }).collect();
                format!("<{name}{attrs}>{content}</{name}>")
            },
        )
    });
    element.prop_map(|e| if e.starts_with('<') { e } else { format!("<r>{e}</r>") })
}

fn text() -> impl Strategy<Value = String> {
    proptest::collection::vec(prop_oneof![Just("a"), Just("b"), Just("c"), Just("d"), Just("")], 0..12).prop_map(|lines| lines.join("\n"))
}

proptest! {
    #[test]
    fn hunks_and_same_lines_cover_both_texts(a in text(), b in text()) {
        let d = diff_lines(&a, &b, &LineOptions::default());
        let old_covered: usize = d.hunks.iter().map(|h| h.old.len()).sum::<usize>() + d.same.len();
        let new_covered: usize = d.hunks.iter().map(|h| h.new.len()).sum::<usize>() + d.same.len();
        prop_assert_eq!((old_covered, new_covered), (d.old_lines.len(), d.new_lines.len()));
        prop_assert_eq!(d.is_identical(), split_lines(&a).iter().map(|r| &a[r.clone()]).eq(split_lines(&b).iter().map(|r| &b[r.clone()])));
    }

    #[test]
    fn a_merge_with_an_unchanged_side_is_the_other_side(base in text(), changed in text()) {
        let o = LineOptions::default();
        let left = merge3(&base, &changed, &base, &o);
        let right = merge3(&base, &base, &changed, &o);
        prop_assert_eq!(left.conflict_count() + right.conflict_count(), 0);
        let lines = |t: &str| split_lines(t).iter().map(|r| t[r.clone()].to_owned()).collect::<Vec<_>>();
        prop_assert_eq!(lines(&left.render(Choice::Markers)), lines(&changed));
        prop_assert_eq!(lines(&right.render(Choice::Markers)), lines(&changed));
    }

    #[test]
    fn the_same_change_on_both_sides_is_never_a_conflict(base in text(), changed in text()) {
        let m = merge3(&base, &changed, &changed, &LineOptions::default());
        prop_assert_eq!(m.conflict_count(), 0);
    }

    #[test]
    fn any_text_compares_without_a_panic(a in "[ -~]{0,60}", b in "[ -~]{0,60}") {
        for family in [Family::Xml, Family::Json, Family::Yaml] {
            let _ = compare_documents(&a, &b, family, &TreeOptions::default());
        }
    }

    #[test]
    fn a_well_formed_document_has_no_difference_with_itself(json in arb_json(), xml in arb_xml()) {
        let none = TreeOptions::default();
        for (text, family) in [(&json, Family::Json), (&json, Family::Yaml), (&xml, Family::Xml)] {
            let found = compare_documents(text, text, family, &none).map_err(|e| TestCaseError::fail(format!("{e} in {text}")))?;
            prop_assert!(found.is_empty(), "{text}: {found:?}");
        }
        // Whatever the options leave out, a document is still the same as itself.
        let all = TreeOptions {
            whitespace: Whitespace::Collapse,
            ignore_case: true,
            ignore_comments: true,
            ignore_child_order: true,
            numbers_by_value: true,
            ignore_prefixes: true,
            ..none
        };
        for (text, family) in [(&json, Family::Json), (&xml, Family::Xml)] {
            prop_assert!(compare_documents(text, text, family, &all).map_err(|e| TestCaseError::fail(e.to_string()))?.is_empty());
        }
    }

    #[test]
    fn what_differs_is_found_and_what_is_found_differs(a in arb_json(), b in arb_json()) {
        let found = compare_documents(&a, &b, Family::Json, &TreeOptions::default()).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let (x, y): (serde_json::Value, serde_json::Value) = (serde_json::from_str(&a).expect("json"), serde_json::from_str(&b).expect("json"));
        // (serde_json compares 1 and 1.0 as different too, and the order of members does not matter to either.)
        prop_assert_eq!(found.is_empty(), x == y, "{} / {}: {:?}", a, b, found);
    }

    #[test]
    fn a_unified_diff_has_the_counts_it_says(a in text(), b in text(), context in 0usize..4, blank in proptest::bool::ANY) {
        let o = LineOptions { ignore_blank_lines: blank, ..LineOptions::default() };
        let d = diff_lines(&a, &b, &o);
        prop_assert!(counts_agree(&d.unified(&a, &b, "a", "b", context)));
    }
}

#[test]
fn the_regions_of_a_merge_say_who_changed_what() {
    let m = merge3("a\nb\nc\nd\ne\n", "a\nB\nc\nd\ne\n", "a\nb\nc\nd\nE\n", &LineOptions::default());
    assert_eq!(m.regions, [Region::Same("a\n".into()), Region::Ours("B\n".into()), Region::Same("c\nd\n".into()), Region::Theirs("E\n".into())]);
    let both = merge3("a\nb\n", "a\nX\n", "a\nX\n", &LineOptions::default());
    assert_eq!(both.regions, [Region::Same("a\n".into()), Region::Both("X\n".into())]);
}

#[test]
fn a_reordered_and_changed_element_list_is_one_change_when_the_order_does_not_matter() {
    let (old, new) = ("<r><a>1</a><b>2</b><c>3</c></r>", "<r><c>3</c><b>9</b><a>1</a></r>");
    let o = TreeOptions { ignore_child_order: true, ..TreeOptions::default() };
    let found: Vec<String> = compare_documents(old, new, Family::Xml, &o).expect("reads").iter().map(|c| c.describe()).collect();
    assert_eq!(found, ["changed /r[1]/b[1]/text()[1]: 2 → 9"]);
    // In order, the same texts differ in more places.
    assert!(changes(old, new, Family::Xml).len() > 1);
}

#[test]
fn equal_elements_are_found_wherever_they_are_when_the_order_does_not_matter() {
    let (old, new) = ("<r><i>1</i><i>2</i><i>3</i></r>", "<r><i>3</i><i>2</i><i>9</i></r>");
    let o = TreeOptions { ignore_child_order: true, ..TreeOptions::default() };
    let found: Vec<String> = compare_documents(old, new, Family::Xml, &o).expect("reads").iter().map(|c| c.describe()).collect();
    assert_eq!(
        found,
        ["changed /r[1]/i[3]/text()[1]: 1 → 9"],
        "the one that has no equal is the one that changed (named by its place in the second document)"
    );
}

#[test]
fn removed_nodes_are_named_by_their_place_in_the_first_document() {
    let found = changes(r#"{"a": [1, {"y": 2, "z": 3}]}"#, r#"{"a": [{"y": 2}]}"#, Family::Json);
    assert!(found.contains(&"removed /a/0: 1".to_owned()), "{found:?}");
    assert!(found.contains(&"removed /a/1/z: 3".to_owned()), "the object was at 1 before it was at 0: {found:?}");
    let xml = changes("<r><a/><b><c/></b></r>", "<r><b/></r>", Family::Xml);
    assert!(xml.iter().any(|c| c.starts_with("removed /r[1]/a[1]")) && xml.iter().any(|c| c.starts_with("removed /r[1]/b[1]/c[1]")), "{xml:?}");
}

#[test]
fn a_change_of_the_root_has_no_parent() {
    let found = compare_documents("[1]", "{\"a\": 1}", Family::Json, &TreeOptions::default()).expect("reads");
    assert_eq!(found.len(), 1);
    assert_eq!(
        (found[0].kind, found[0].path.as_str(), found[0].old_parent.clone(), found[0].new_parent.clone()),
        (ChangeKind::Changed, "", 0..0, 0..0)
    );
}

#[test]
fn many_changed_items_are_compared_in_linear_time() {
    let items = |f: &dyn Fn(usize) -> String| format!("[{}]", (0..30_000).map(f).collect::<Vec<_>>().join(","));
    let (old, new) = (items(&|i| format!("{{\"v\": {i}}}")), items(&|i| format!("{{\"v\": {}}}", i + 1_000_000)));
    for ignore_child_order in [false, true] {
        let started = std::time::Instant::now();
        let found = compare_documents(&old, &new, Family::Json, &TreeOptions { ignore_child_order, ..TreeOptions::default() }).expect("reads");
        assert_eq!(found.len(), 30_000);
        assert!(started.elapsed() < std::time::Duration::from_secs(20), "{:?}", started.elapsed());
    }
}

#[test]
fn white_space_order_numbers_and_prefixes_apply_where_the_documentation_says() {
    let ends = TreeOptions { whitespace: Whitespace::Ends, ..TreeOptions::default() };
    assert!(compare_documents(r#"{"a": " x "}"#, r#"{"a": "x"}"#, Family::Json, &ends).expect("reads").is_empty(), "strings too");
    let order = TreeOptions { ignore_child_order: true, ..TreeOptions::default() };
    assert!(compare_documents("[1, 2, 3]", "[3, 1, 2]", Family::Json, &order).expect("reads").is_empty(), "the items of an array too");
    assert!(!compare_documents("[1, 2, 3]", "[3, 1, 2]", Family::Json, &TreeOptions::default()).expect("reads").is_empty());
    let by_value = TreeOptions { numbers_by_value: true, ..TreeOptions::default() };
    assert!(compare_documents(r#"{"a": 0xFF, "b": -0x10}"#, r#"{"a": 255, "b": -16}"#, Family::Json, &by_value).expect("reads").is_empty());
    assert!(!compare_documents(r#"{"a": 0xFF}"#, r#"{"a": 255}"#, Family::Json, &TreeOptions::default()).expect("reads").is_empty());
    // The reserved prefixes: `xml:` is a namespace of its own, and declarations are not compared when prefixes are ignored.
    let prefixes = TreeOptions { ignore_prefixes: true, ..TreeOptions::default() };
    let (p, q) = (r#"<p:a xmlns:p="u" xml:lang="en"/>"#, r#"<q:a xmlns:q="u" xml:lang="en"/>"#);
    assert!(compare_documents(p, q, Family::Xml, &prefixes).expect("reads").is_empty());
    assert!(!compare_documents(p, r#"<q:a xmlns:q="u" xml:lang="de"/>"#, Family::Xml, &prefixes).expect("reads").is_empty());
    assert!(!compare_documents(r#"<a xmlns:p="u"/>"#, r#"<a xmlns:q="u"/>"#, Family::Xml, &TreeOptions::default()).expect("reads").is_empty());
    assert!(compare_documents(r#"<a xmlns:p="u"/>"#, r#"<a xmlns:q="u"/>"#, Family::Xml, &prefixes).expect("reads").is_empty());
}

#[test]
fn data_nested_too_deeply_is_refused() {
    use xmlcraft_json_syntax::{Data, Value};
    let mut value = Value { span: 0..0, data: Data::Null };
    for _ in 0..600 {
        value = Value { span: 0..0, data: Data::Array(vec![value]) };
    }
    assert!(Node::from_value(&value).is_err());
    let mut shallow = Value { span: 0..0, data: Data::Null };
    for _ in 0..100 {
        shallow = Value { span: 0..0, data: Data::Array(vec![shallow]) };
    }
    assert!(Node::from_value(&shallow).is_ok());
}

#[test]
fn a_unified_diff_counts_right_with_blank_lines_ignored_and_with_huge_context() {
    let (old, new) = ("a\n\nb\nc\nd\ne\nf\n", "a\nb\nc\nd\nE\nf\n");
    let o = LineOptions { ignore_blank_lines: true, ..LineOptions::default() };
    let d = diff_lines(old, new, &o);
    assert_eq!(d.hunks.len(), 1, "the blank line is no difference");
    let text = d.unified(old, new, "a", "b", 3);
    assert!(counts_agree(&text), "{text}");
    assert!(text.contains("@@ -1,7 +1,6 @@"), "{text}");
    // A difference of blank lines alone has no block.
    assert!(
        diff_lines("a\n\nb\n", "a\nb\n", &o).unified("a\n\nb\n", "a\nb\n", "a", "b", 3).is_empty()
            || diff_lines("a\n\nb\n", "a\nb\n", &o).hunks.is_empty()
    );
    for context in [usize::MAX, usize::MAX / 2 + 1] {
        assert!(counts_agree(&d.unified(old, new, "a", "b", context)));
    }
}

#[test]
fn a_merge_keeps_what_the_options_ignore_and_marks_conflicts_cleanly() {
    let case = LineOptions { ignore_case: true, ..LineOptions::default() };
    assert_eq!(merge3("a\nb\n", "a\nb\n", "A\nb\n", &case).render(Choice::Markers), "A\nb\n", "their change of case is not undone");
    assert_eq!(merge3("a\nb\n", "A\nb\n", "a\nb\n", &case).render(Choice::Markers), "A\nb\n");
    assert_eq!(merge3("a\nb\n", "A\nb\n", "a\nB\n", &case).render(Choice::Markers), "A\nB\n", "each side's change of case stays");
    let blank = LineOptions { ignore_blank_lines: true, ..LineOptions::default() };
    let merged = merge3("a\nb\n", "a\n\nb\n", "a\nX\nb\n", &blank);
    assert_eq!(
        (merged.conflict_count(), merged.render(Choice::Markers)),
        (0, "a\nX\nb\n".to_owned()),
        "a blank line is no change: {:?}",
        merged.regions
    );
    assert_eq!(merge3("a\nb\n", "a\n\nb\n", "a\nb\n", &blank).render(Choice::Markers), "a\n\nb\n");
    assert_eq!(merge3("a\nb\n", "a\n\nb\n", "a\nb\n", &LineOptions::default()).render(Choice::Markers), "a\n\nb\n");
    // A side whose last line has no break still gets the markers on lines of their own.
    let conflict = merge3("a\nb", "a\nX", "a\nY", &LineOptions::default());
    assert_eq!(conflict.render(Choice::Markers), "a\n<<<<<<< ours\nX\n||||||| base\nb\n=======\nY\n>>>>>>> theirs\n");
}

#[test]
fn what_is_removed_deep_inside_a_moved_node_is_named_by_the_first_document() {
    let found = changes(r#"{"a": [1, {"y": [5, 6]}]}"#, r#"{"a": [{"y": [6]}]}"#, Family::Json);
    assert!(found.contains(&"removed /a/1/y/0: 5".to_owned()), "{found:?}");
}

#[test]
fn blank_lines_added_by_their_side_stay_and_blocks_of_ignored_differences_are_not_written() {
    let blank = LineOptions { ignore_blank_lines: true, ..LineOptions::default() };
    assert_eq!(merge3("a\nb\n", "a\nb\n", "a\n\nb\n", &blank).render(Choice::Markers), "a\n\nb\n");
    let (old, new) = ("a\n\nb\nc\nd\ne\nf\ng\nh\ni\nJ\n", "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n");
    let d = diff_lines(old, new, &blank);
    let text = d.unified(old, new, "a", "b", 1);
    assert_eq!(text.matches("@@ -").count(), 1, "one block, for the difference that counts: {text}");
    assert!(counts_agree(&text));
}
