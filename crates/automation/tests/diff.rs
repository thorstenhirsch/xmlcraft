//! `diff_compare` and `diff_merge`, read through the root confinement.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, tool: &str, args: Value) -> Result<Value, ToolError> {
    a.call(tool, &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

#[test]
fn xml_texts_are_compared_as_trees_by_default() {
    let mut a = Automation::new();
    let r = call(
        &mut a,
        "diff_compare",
        json!({ "old": { "text": "<r a=\"1\"><x>1</x></r>" }, "new": { "text": "<r a=\"2\"><x>1</x><y/></r>" }, "format": "xml" }),
    )
    .expect("runs");
    assert_eq!((r["mode"].as_str(), r["identical"].as_bool(), r["count"].as_u64()), (Some("tree"), Some(false), Some(2)), "{r}");
    assert_eq!((r["changes"][0]["kind"].as_str(), r["changes"][0]["path"].as_str()), (Some("changed"), Some("/r[1]/@a")));
    assert_eq!((r["changes"][0]["old_value"].as_str(), r["changes"][0]["new_value"].as_str()), (Some("1"), Some("2")));
    assert_eq!(r["changes"][0]["new"]["line"].as_u64(), Some(1));
    assert_eq!((r["changes"][1]["kind"].as_str(), r["changes"][1]["path"].as_str()), (Some("added"), Some("/r[1]/y[1]")));
}

#[test]
fn json_is_compared_by_pointer_and_text_that_is_not_a_tree_falls_back_to_lines() {
    let mut a = Automation::new();
    let r = call(&mut a, "diff_compare", json!({ "old": { "text": "{\"a\": [1, 2]}" }, "new": { "text": "{\"a\": [1, 3]}" }, "format": "json" }))
        .expect("runs");
    assert_eq!(r["changes"][0]["path"], "/a/1", "{r}");
    // Not well-formed XML: lines, with the words that differ.
    let r = call(&mut a, "diff_compare", json!({ "old": { "text": "<a>\none two\nx\n" }, "new": { "text": "<a>\none three\nx\nfour\n" } }))
        .expect("runs");
    assert_eq!(r["mode"], "lines", "{r}");
    assert_eq!(r["count"].as_u64(), Some(2));
    assert_eq!((r["hunks"][0]["kind"].as_str(), r["hunks"][0]["old"]["first"].as_u64()), (Some("changed"), Some(2)));
    assert_eq!(r["hunks"][0]["words"]["old"], json!(["two"]));
    assert_eq!(r["hunks"][0]["words"]["new"], json!(["three"]));
    assert_eq!((r["hunks"][1]["kind"].as_str(), r["hunks"][1]["new"]["first"].as_u64()), (Some("added"), Some(4)));
    assert!(r["unified"].as_str().expect("unified").contains("-one two\n+one three\n x\n+four\n"), "{r}");
    // Asking for a tree of what is not one is an error that says why.
    let e = call(&mut a, "diff_compare", json!({ "old": { "text": "<a>" }, "new": { "text": "<a/>" }, "mode": "tree" })).expect_err("not a tree");
    assert!(e.to_string().contains("not well-formed"), "{e}");
}

#[test]
fn the_options_say_what_counts_as_the_same() {
    let mut a = Automation::new();
    let (old, new) = ("<r>\n <a>X</a>\n</r>", "<r><a> x </a></r>");
    let r = call(&mut a, "diff_compare", json!({ "old": { "text": old }, "new": { "text": new }, "whitespace": "collapse", "ignore_case": true }))
        .expect("runs");
    assert_eq!(r["identical"], true, "{r}");
    let r = call(&mut a, "diff_compare", json!({ "old": { "text": old }, "new": { "text": new } })).expect("runs");
    assert_eq!(r["identical"], false);
    for bad in [
        json!({ "whitespace": "some" }),
        json!({ "mode": "x" }),
        json!({ "format": "toml" }),
        json!({ "ignore_case": "yes" }),
        json!({ "context": 1000, "mode": "lines" }),
    ] {
        let mut args = json!({ "old": { "text": "a" }, "new": { "text": "b" } });
        args.as_object_mut().expect("object").extend(bad.as_object().expect("object").clone());
        assert!(matches!(call(&mut a, "diff_compare", args.clone()), Err(ToolError::InvalidArgs(_))), "{args}");
    }
    assert!(matches!(call(&mut a, "diff_compare", json!({ "old": { "text": "a" } })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(call(&mut a, "diff_compare", json!({ "old": "a", "new": "b" })), Err(ToolError::InvalidArgs(_))));
}

#[test]
fn files_and_open_documents_are_compared_inside_the_root_only() {
    let inside = tempfile::tempdir().expect("dir");
    let outside = tempfile::tempdir().expect("dir");
    std::fs::write(inside.path().join("a.json"), "{\"k\": 1}").expect("writes");
    std::fs::write(inside.path().join("b.yaml"), "k: 2\n").expect("writes");
    std::fs::write(outside.path().join("s.json"), "{}").expect("writes");
    let mut a = Automation::with_root(inside.path()).expect("root");
    // A JSON file against a YAML file: each named document is read in its own format (both become the same trees).
    let r = call(&mut a, "diff_compare", json!({ "old": { "path": "a.json" }, "new": { "path": "b.yaml" } })).expect("runs");
    assert_eq!((r["format"].clone(), r["changes"][0]["path"].as_str()), (json!({ "old": "json", "new": "yaml" }), Some("/k")), "{r}");
    let opened = call(&mut a, "doc_open", json!({ "path": "b.yaml" })).expect("opens");
    let r = call(&mut a, "diff_compare", json!({ "old": { "path": "a.json" }, "new": { "id": opened["id"] }, "format": "yaml" })).expect("runs");
    assert_eq!(r["format"], json!({ "old": "yaml", "new": "yaml" }), "{r}");
    let e =
        call(&mut a, "diff_compare", json!({ "old": { "path": outside.path().join("s.json").to_str().expect("utf-8") }, "new": { "text": "{}" } }))
            .expect_err("outside the root");
    assert!(e.to_string().contains("outside") || e.to_string().contains("root"), "{e}");
}

#[test]
fn a_merge_takes_both_changes_and_writes_conflicts_as_asked() {
    let mut a = Automation::new();
    let base = "a\nb\nc\nd\n";
    let r = call(&mut a, "diff_merge", json!({ "base": { "text": base }, "ours": { "text": "A\nb\nc\nd\n" }, "theirs": { "text": "a\nb\nc\nD\n" } }))
        .expect("runs");
    assert_eq!((r["text"].as_str(), r["conflicts"].as_u64(), r["clean"].as_bool()), (Some("A\nb\nc\nD\n"), Some(0), Some(true)), "{r}");
    let conflict = json!({ "base": { "text": base }, "ours": { "text": "X\nb\nc\nd\n" }, "theirs": { "text": "Y\nb\nc\nd\n" } });
    let r = call(&mut a, "diff_merge", conflict.clone()).expect("runs");
    assert_eq!((r["conflicts"].as_u64(), r["clean"].as_bool()), (Some(1), Some(false)));
    assert!(r["text"].as_str().expect("text").starts_with("<<<<<<< ours\nX\n||||||| base\na\n=======\nY\n>>>>>>> theirs\n"), "{r}");
    for (choice, expected) in [("ours", "X\nb\nc\nd\n"), ("theirs", "Y\nb\nc\nd\n"), ("both", "X\nY\nb\nc\nd\n"), ("base", base)] {
        let mut args = conflict.clone();
        args["conflicts"] = json!(choice);
        assert_eq!(call(&mut a, "diff_merge", args).expect("runs")["text"], expected, "{choice}");
    }
    let mut bad = conflict;
    bad["conflicts"] = json!("coin");
    assert!(matches!(call(&mut a, "diff_merge", bad), Err(ToolError::InvalidArgs(_))));
}

#[test]
fn a_change_says_where_its_parents_are_and_the_merge_offers_only_what_it_reads() {
    let mut a = Automation::new();
    let r = call(
        &mut a,
        "diff_compare",
        json!({ "old": { "text": "{\"a\": {\"x\": 1}}" }, "new": { "text": "{\"a\": {\"x\": 1, \"y\": 2}}" }, "format": "json" }),
    )
    .expect("runs");
    assert_eq!(r["changes"][0]["kind"], "added", "{r}");
    assert_eq!((r["changes"][0]["new_parent"]["column"].as_u64(), r["changes"][0]["old_parent"]["column"].as_u64()), (Some(7), Some(7)));
    let tool = xmlcraft_automation::find_tool("diff_merge").expect("the tool");
    let properties = tool.input_schema["properties"].as_object().expect("properties").keys().cloned().collect::<Vec<_>>();
    for read in ["base", "ours", "theirs", "conflicts", "whitespace", "ignore_case", "ignore_blank_lines"] {
        assert!(properties.contains(&read.to_owned()), "{read}");
    }
    for ignored in
        ["format", "mode", "ignore_comments", "ignore_child_order", "numbers_by_value", "ignore_prefixes", "ignore_processing_instructions"]
    {
        assert!(!properties.contains(&ignored.to_owned()), "{ignored} is not read by a merge");
    }
}

#[test]
fn side_by_side_rows_pair_the_lines_and_hunks_say_which_differences_count() {
    let mut a = Automation::new();
    // The value changes on line 2; line 4 differs only in the order of its attributes, which is no difference of the trees.
    let (old, new) = ("<r>\n<a>Milk</a>\n<m/>\n<b x=\"1\" y=\"2\"/>\n</r>\n", "<r>\n<a>Oat milk</a>\n<m/>\n<b y=\"2\" x=\"1\"/>\n<n/>\n</r>\n");
    let r = call(&mut a, "diff_hunks", json!({ "old": { "text": old }, "new": { "text": new } })).expect("runs");
    assert_eq!((r["mode"].as_str(), r["hunk_count"].as_u64(), r["count"].as_u64()), (Some("tree"), Some(2), Some(2)), "{r}");
    let row = &r["rows"][1];
    assert_eq!((row["kind"].as_str(), row["left"].as_u64(), row["right"].as_u64()), (Some("changed"), Some(2), Some(2)), "{r}");
    assert_eq!(row["left_parts"], json!([{ "column": 4, "length": 4, "text": "Milk" }]));
    assert_eq!(row["right_parts"], json!([{ "column": 4, "length": 8, "text": "Oat milk" }]));
    assert_eq!(r["hunks"][1]["right"], json!({ "first": 4, "last": 5 }), "{r}");
    // Only the order of the attributes: one hunk, not significant.
    let (old, new) = ("<r>\n<a/>\n<b x=\"1\" y=\"2\"/>\n</r>\n", "<r>\n<a/>\n<b y=\"2\" x=\"1\"/>\n</r>\n");
    let r = call(&mut a, "diff_hunks", json!({ "old": { "text": old }, "new": { "text": new } })).expect("runs");
    assert_eq!((r["hunk_count"].as_u64(), r["count"].as_u64(), r["identical"].as_bool()), (Some(1), Some(0), Some(true)), "{r}");
    assert_eq!(r["hunks"][0]["significant"], false);
    let r = call(&mut a, "diff_hunks", json!({ "old": { "text": old }, "new": { "text": new }, "mode": "lines" })).expect("runs");
    assert_eq!((r["mode"].as_str(), r["count"].as_u64()), (Some("lines"), Some(1)), "lines count every hunk: {r}");
    assert!(matches!(call(&mut a, "diff_hunks", json!({ "old": { "text": "a" } })), Err(ToolError::InvalidArgs(_))));
    assert!(call(&mut a, "diff_hunks", json!({ "old": { "text": "<a>" }, "new": { "text": "<a/>" }, "mode": "tree" })).is_err());
}
