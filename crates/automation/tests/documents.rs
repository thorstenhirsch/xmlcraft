//! The document and XML tools, driven through `Automation::call` with real files: what an agent or a script would do.

use std::path::Path;

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, tool: &str, args: Value) -> Result<Value, ToolError> {
    a.call(tool, &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

fn ok(a: &mut Automation, tool: &str, args: Value) -> Value {
    call(a, tool, args).unwrap_or_else(|e| panic!("{tool} failed: {e}"))
}

fn err(a: &mut Automation, tool: &str, args: Value) -> String {
    call(a, tool, args).expect_err("the call should fail").to_string()
}

fn file(dir: &Path, name: &str, bytes: &[u8]) -> String {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("writes");
    path.to_str().expect("utf-8 path").to_owned()
}

#[test]
fn a_file_is_opened_read_edited_and_saved_with_its_bytes_kept() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "a.xml", b"\xEF\xBB\xBF<a>\r\n  <b>x</b>\r\n</a>\r\n");
    let mut a = Automation::new();
    let opened = ok(&mut a, "doc_open", json!({ "path": path }));
    let id = opened["id"].as_u64().expect("an id");
    assert_eq!(
        (opened["name"].as_str(), opened["encoding"].as_str(), opened["bom"].as_bool(), opened["line_ending"].as_str()),
        (Some("a.xml"), Some("UTF-8"), Some(true), Some("CRLF"))
    );
    assert_eq!((opened["dirty"].as_bool(), opened["lines"].as_u64(), opened["already_open"].as_bool()), (Some(false), Some(4), Some(false)));
    // Opening it again shows the open one.
    assert_eq!(ok(&mut a, "doc_open", json!({ "path": path }))["already_open"], true);
    assert_eq!(ok(&mut a, "doc_list", json!({}))["count"], 1);
    // Reading: all of it, and lines 2 to 3.
    assert_eq!(ok(&mut a, "doc_text", json!({ "id": id }))["text"], "<a>\r\n  <b>x</b>\r\n</a>\r\n");
    assert_eq!(ok(&mut a, "doc_text", json!({ "id": id, "from_line": 2, "to_line": 2 }))["text"], "  <b>x</b>\r\n");
    // Editing by line and column (1-based): x becomes yy, and a new line is inserted before </a>.
    let edited = ok(
        &mut a,
        "doc_edit",
        json!({ "id": id, "edits": [
            { "line": 2, "column": 6, "end_line": 2, "end_column": 7, "text": "yy" },
            { "line": 3, "column": 1, "text": "  <c/>\r\n" },
        ] }),
    );
    assert_eq!((edited["changed"].as_bool(), edited["document"]["dirty"].as_bool()), (Some(true), Some(true)));
    assert_eq!(ok(&mut a, "doc_text", json!({ "id": id }))["text"], "<a>\r\n  <b>yy</b>\r\n  <c/>\r\n</a>\r\n");
    // One undo takes both edits back.
    assert_eq!(ok(&mut a, "doc_undo", json!({ "id": id }))["done"], true);
    assert_eq!(ok(&mut a, "doc_text", json!({ "id": id }))["text"], "<a>\r\n  <b>x</b>\r\n</a>\r\n");
    assert_eq!(ok(&mut a, "doc_undo", json!({ "id": id, "redo": true }))["done"], true);
    // Saving writes the byte-order mark and the CRLFs the file had.
    let saved = ok(&mut a, "doc_save", json!({ "id": id }));
    assert_eq!(saved["document"]["dirty"], false);
    assert_eq!(std::fs::read(&path).expect("reads"), b"\xEF\xBB\xBF<a>\r\n  <b>yy</b>\r\n  <c/>\r\n</a>\r\n");
    assert_eq!(ok(&mut a, "doc_close", json!({ "id": id }))["closed"], true);
    assert_eq!(ok(&mut a, "doc_list", json!({}))["count"], 0);
}

#[test]
fn unsaved_changes_and_files_changed_on_disk_are_never_lost_silently() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "a.xml", b"<a/>");
    let mut a = Automation::new();
    let id = ok(&mut a, "doc_open", json!({ "path": path }))["id"].as_u64().expect("id");
    ok(&mut a, "doc_set_text", json!({ "id": id, "text": "<changed/>" }));
    assert!(err(&mut a, "doc_close", json!({ "id": id })).contains("unsaved changes"));
    // Someone else wrote the file meanwhile.
    std::fs::write(&path, b"<theirs/>").expect("writes");
    let refused = err(&mut a, "doc_save", json!({ "id": id }));
    assert!(refused.contains("changed on disk") && refused.contains("force"), "{refused}");
    assert_eq!(std::fs::read(&path).expect("reads"), b"<theirs/>");
    ok(&mut a, "doc_save", json!({ "id": id, "force": true }));
    assert_eq!(std::fs::read(&path).expect("reads"), b"<changed/>");
    // Discarding is explicit.
    ok(&mut a, "doc_set_text", json!({ "id": id, "text": "<more/>" }));
    assert_eq!(ok(&mut a, "doc_close", json!({ "id": id, "discard": true }))["discarded_changes"], true);
    assert!(err(&mut a, "doc_text", json!({ "id": id })).contains("no document has the id"));
}

#[test]
fn a_file_with_invalid_bytes_is_not_overwritten_but_can_be_saved_elsewhere() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "bad.xml", b"<a>\xFF</a>");
    let mut a = Automation::new();
    let opened = ok(&mut a, "doc_open", json!({ "path": path }));
    assert_eq!(opened["lossy"], true);
    let id = opened["id"].as_u64().expect("id");
    assert!(err(&mut a, "doc_save", json!({ "id": id })).contains("not valid"));
    assert_eq!(std::fs::read(&path).expect("reads"), b"<a>\xFF</a>");
    let copy = dir.path().join("copy.xml");
    ok(&mut a, "doc_save", json!({ "id": id, "path": copy.to_str() }));
    assert!(copy.exists());
    assert_eq!(std::fs::read(&path).expect("reads"), b"<a>\xFF</a>");
}

#[test]
fn a_character_the_encoding_cannot_hold_is_an_error_unless_references_are_asked_for() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "latin.xml", b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a><!-- c --></a>");
    let mut a = Automation::new();
    let id = ok(&mut a, "doc_open", json!({ "path": path }))["id"].as_u64().expect("id");
    // Inside the comment, in front of the `c`.
    let column = ok(&mut a, "doc_text", json!({ "id": id }))["text"].as_str().and_then(|t| t.find("c -->")).expect("the comment") + 1;
    ok(&mut a, "doc_edit", json!({ "id": id, "edits": [{ "line": 1, "column": column, "text": "日" }] }));
    let refused = err(&mut a, "doc_save", json!({ "id": id }));
    assert!(refused.contains("U+65E5") && refused.contains("line 1"), "{refused}");
    // Inside the comment a reference would mean something else: refused too, with the way out.
    let still = err(&mut a, "doc_save", json!({ "id": id, "unmappable": "references" }));
    assert!(still.contains("U+65E5"), "{still}");
    // Character data takes one.
    ok(&mut a, "doc_set_text", json!({ "id": id, "text": "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a>日</a>" }));
    ok(&mut a, "doc_save", json!({ "id": id, "unmappable": "references" }));
    assert_eq!(std::fs::read(&path).expect("reads"), b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a>&#x65E5;</a>");
    // Or change the encoding: the declaration follows.
    ok(&mut a, "doc_set_text", json!({ "id": id, "text": "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a>日</a>" }));
    ok(&mut a, "doc_set_encoding", json!({ "id": id, "encoding": "utf-8" }));
    ok(&mut a, "doc_save", json!({ "id": id, "force": true }));
    assert_eq!(String::from_utf8_lossy(&std::fs::read(&path).expect("reads")), "<?xml version=\"1.0\" encoding=\"UTF-8\"?><a>日</a>");
    assert!(err(&mut a, "doc_set_encoding", json!({ "id": id, "encoding": "klingon" })).contains("klingon"));
}

#[test]
fn bad_edits_and_bad_arguments_are_refused_with_a_reason() {
    let mut a = Automation::new();
    let id = ok(&mut a, "doc_new", json!({ "text": "ab\ncd" }))["id"].as_u64().expect("id");
    assert!(err(&mut a, "doc_edit", json!({ "id": id, "edits": [{ "line": 9, "column": 1, "text": "x" }] })).contains("there is no line 9"));
    assert!(err(&mut a, "doc_edit", json!({ "id": id, "edits": [{ "line": 1, "column": 1, "end_column": 2, "text": "x" }, { "line": 1, "column": 1, "end_column": 2, "text": "y" }] })).contains("overlap"));
    assert!(
        err(&mut a, "doc_edit", json!({ "id": id, "edits": [{ "line": 2, "column": 2, "end_line": 1, "end_column": 1, "text": "x" }] }))
            .contains("end is before the start")
    );
    assert!(err(&mut a, "doc_edit", json!({ "id": id, "edits": [] })).contains("doc_edit"), "an empty list is refused by the schema");
    assert!(
        err(&mut a, "doc_edit", json!({ "id": id, "edits": [{ "line": 0, "column": 1, "text": "" }] })).contains("doc_edit"),
        "lines count from 1"
    );
    assert!(err(&mut a, "doc_text", json!({ "id": id, "from_line": 3, "to_line": 2 })).contains("after"));
    assert!(err(&mut a, "doc_text", json!({ "id": 999 })).contains("no document has the id 999"));
    assert!(err(&mut a, "doc_text", json!({})).contains("doc_text"));
    // Nothing was changed by all that.
    assert_eq!(ok(&mut a, "doc_text", json!({ "id": id }))["text"], "ab\ncd");
    // A column past the end of a line is the end of the line; a multi-line replace works.
    ok(&mut a, "doc_edit", json!({ "id": id, "edits": [{ "line": 1, "column": 99, "end_line": 2, "end_column": 1, "text": "-" }] }));
    assert_eq!(ok(&mut a, "doc_text", json!({ "id": id }))["text"], "ab-cd");
    // The message names the file (the separators are the platform's: a root-relative path is written with backslashes on Windows).
    assert!(err(&mut a, "doc_open", json!({ "path": "/no/such/file.xml" })).contains("file.xml"));
}

#[test]
fn well_formedness_is_reported_for_a_document_a_file_or_text() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "bad.xml", b"<a>\n  <b></c>\n</a>");
    let mut a = Automation::new();
    let from_file = ok(&mut a, "xml_wellformed", json!({ "path": path }));
    assert_eq!((from_file["well_formed"].as_bool(), from_file["errors"].as_u64()), (Some(false), Some(1)));
    let problem = &from_file["problems"][0];
    assert_eq!((problem["line"].as_u64(), problem["column"].as_u64(), problem["severity"].as_str()), (Some(2), Some(6), Some("error")));
    assert_eq!(problem["code"], "xml.mismatched-end-tag");
    assert!(problem["message"].as_str().is_some_and(|m| m.contains("</c>")));
    let from_text = ok(&mut a, "xml_wellformed", json!({ "text": "<a/>" }));
    assert_eq!((from_text["well_formed"].as_bool(), from_text["problems"].as_array().map(Vec::len)), (Some(true), Some(0)));
    let id = ok(&mut a, "doc_open", json!({ "path": path }))["id"].as_u64().expect("id");
    assert_eq!(ok(&mut a, "xml_wellformed", json!({ "id": id }))["errors"], 1);
    ok(&mut a, "doc_set_text", json!({ "id": id, "text": "<a/>" }));
    assert_eq!(ok(&mut a, "xml_wellformed", json!({ "id": id }))["well_formed"], true, "the document's text as it is now");
    // Exactly one source.
    assert!(err(&mut a, "xml_wellformed", json!({})).contains("exactly one"));
    assert!(err(&mut a, "xml_wellformed", json!({ "id": id, "text": "<a/>" })).contains("exactly one"));
    // A warning is not an error.
    let warned = ok(&mut a, "xml_wellformed", json!({ "text": "<?xml version=\"1.1\"?><a/>" }));
    assert_eq!((warned["well_formed"].as_bool(), warned["warnings"].as_u64()), (Some(true), Some(1)));
}

#[test]
fn many_problems_are_counted_but_listed_in_part() {
    let mut a = Automation::new();
    let text = "<a>".to_owned() + &"&bad ".repeat(2000) + "</a>";
    let r = ok(&mut a, "xml_wellformed", json!({ "text": text }));
    assert!(r["errors"].as_u64() > Some(500), "{}", r["errors"]);
    assert_eq!((r["problems"].as_array().map(Vec::len), r["truncated"].as_bool()), (Some(500), Some(true)));
}

#[test]
fn the_outline_lists_elements_with_positions() {
    let mut a = Automation::new();
    let r = ok(&mut a, "xml_outline", json!({ "text": "<lib>\n  <book id=\"b1\"/>\n  <book name=\"two\"><t/></book>\n</lib>" }));
    let items: Vec<(u64, &str, Option<&str>, u64)> = r["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|i| (i["depth"].as_u64().expect("depth"), i["name"].as_str().expect("name"), i["label"].as_str(), i["line"].as_u64().expect("line")))
        .collect();
    assert_eq!(items, [(0, "lib", None, 1), (1, "book", Some("b1"), 2), (1, "book", Some("two"), 3), (2, "t", None, 3)]);
    assert_eq!((r["count"].as_u64(), r["well_formed"].as_bool(), r["truncated"].as_bool()), (Some(4), Some(true), Some(false)));
    let shape: Vec<(&str, u64, &str)> = r["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|i| (i["kind"].as_str().expect("kind"), i["children"].as_u64().expect("children"), i["preview"].as_str().expect("preview")))
        .collect();
    assert_eq!(shape, [("element", 2, ""), ("empty", 0, ""), ("element", 1, ""), ("empty", 0, "")]);
    let r = ok(&mut a, "xml_outline", json!({ "text": "<a><t>  Hello\n  world </t></a>" }));
    assert_eq!((r["items"][1]["kind"].as_str(), r["items"][1]["preview"].as_str()), (Some("leaf"), Some("Hello world")));
    let r = ok(&mut a, "xml_outline", json!({ "text": "{\"n\": 1, \"o\": {\"s\": \"x\"}}", "language": "json" }));
    let kinds: Vec<&str> = r["items"].as_array().expect("items").iter().filter_map(|i| i["kind"].as_str()).collect();
    assert_eq!(kinds, ["number", "object", "string"]);
}

#[test]
fn formatting_text_files_and_documents() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "a.xml", b"<a><b><c/></b>text<d/></a>");
    let mut a = Automation::new();
    // Text and files are returned, never written.
    let r = ok(&mut a, "xml_format", json!({ "text": "<a><b/><c/></a>" }));
    assert_eq!((r["text"].as_str(), r["changed"].as_bool()), (Some("<a>\n  <b/>\n  <c/>\n</a>"), Some(true)));
    let tabs = ok(&mut a, "xml_format", json!({ "text": "<a><b/></a>", "tabs": true }));
    assert_eq!(tabs["text"], "<a>\n\t<b/>\n</a>");
    let four = ok(&mut a, "xml_format", json!({ "text": "<a><b><c/></b></a>", "indent": 4 }));
    assert_eq!(four["text"], "<a>\n    <b>\n        <c/>\n    </b>\n</a>");
    let mini = ok(&mut a, "xml_format", json!({ "text": "<a>\n  <b/>\n</a>\n", "minify": true }));
    assert_eq!(mini["text"], "<a><b/></a>");
    let nothing = ok(&mut a, "xml_format", json!({ "text": "<a>\n  <b/>\n</a>" }));
    assert_eq!((nothing["changed"].as_bool(), nothing["edits"].as_u64()), (Some(false), Some(0)));
    let from_file = ok(&mut a, "xml_format", json!({ "path": path }));
    assert_eq!(from_file["text"], "<a><b><c/></b>text<d/></a>", "mixed content: left as it is");
    assert_eq!(std::fs::read(&path).expect("reads"), b"<a><b><c/></b>text<d/></a>", "the file is not written");
    // On a document it is one undoable edit, in the document's own style.
    let id = ok(&mut a, "doc_open", json!({ "path": path }))["id"].as_u64().expect("id");
    ok(&mut a, "doc_set_text", json!({ "id": id, "text": "<a>\r\n  <b><c/></b>\r\n</a>" }));
    let done = ok(&mut a, "xml_format", json!({ "id": id }));
    assert_eq!(done["changed"], true);
    assert_eq!(ok(&mut a, "doc_text", json!({ "id": id }))["text"], "<a>\r\n  <b>\r\n    <c/>\r\n  </b>\r\n</a>");
    ok(&mut a, "doc_undo", json!({ "id": id }));
    assert_eq!(ok(&mut a, "doc_text", json!({ "id": id }))["text"], "<a>\r\n  <b><c/></b>\r\n</a>");
    // Broken markup is refused with its position, and nothing changes.
    let refused = err(&mut a, "xml_format", json!({ "text": "<a>\n<b></a>" }));
    assert!(refused.contains("not well-formed") && refused.contains("line 2"), "{refused}");
    assert!(err(&mut a, "xml_format", json!({ "text": "<a/>", "indent": 99 })).contains("xml_format"));
}

#[test]
fn files_outside_the_root_are_out_of_reach_and_the_root_is_what_refuses_them() {
    let root = tempfile::tempdir().expect("root");
    let outside = tempfile::tempdir().expect("outside");
    let inside_path = file(root.path(), "in.xml", b"<in/>");
    let outside_path = file(outside.path(), "out.xml", b"<out/>");
    let mut confined = Automation::with_root(root.path()).expect("root");
    // Inside works, also by a path relative to the root.
    assert_eq!(ok(&mut confined, "xml_wellformed", json!({ "path": "in.xml" }))["well_formed"], true);
    assert_eq!(ok(&mut confined, "doc_open", json!({ "path": inside_path }))["name"], "in.xml");
    // Outside is refused by every tool that takes a path: reading, opening, saving elsewhere.
    for (tool, args) in [
        ("xml_wellformed", json!({ "path": outside_path })),
        ("xml_outline", json!({ "path": outside_path })),
        ("xml_format", json!({ "path": outside_path })),
        ("doc_open", json!({ "path": outside_path })),
        ("xml_wellformed", json!({ "path": "../out.xml" })),
    ] {
        let message = err(&mut confined, tool, args.clone());
        assert!(message.contains("outside the root"), "{tool} {args}: {message}");
    }
    let id = ok(&mut confined, "doc_open", json!({ "path": "in.xml" }))["id"].as_u64().expect("id");
    let target = outside.path().join("copy.xml");
    assert!(call(&mut confined, "doc_save", json!({ "id": id, "path": target.to_str() })).is_err());
    assert!(!target.exists(), "nothing was written outside the root");
    // The same calls without a root succeed: the root is what stopped them.
    let mut free = Automation::new();
    assert_eq!(ok(&mut free, "xml_wellformed", json!({ "path": outside_path }))["well_formed"], true);
    let id = ok(&mut free, "doc_open", json!({ "path": outside_path }))["id"].as_u64().expect("id");
    ok(&mut free, "doc_save", json!({ "id": id, "path": target.to_str() }));
    assert!(target.exists());
}

#[test]
fn finding_and_replacing_text_and_patterns() {
    let mut a = Automation::new();
    let id = ok(&mut a, "doc_new", json!({ "text": "<book id=\"b1\"/>\n<Book id=\"b2\"/>\n<notebook/>" }))["id"].as_u64().expect("id");
    let found = ok(&mut a, "doc_find", json!({ "id": id, "pattern": "book" }));
    assert_eq!((found["count"].as_u64(), found["more"].as_bool()), (Some(2), Some(false)), "case-sensitive: `book` and the one in `notebook`");
    assert_eq!(
        (found["matches"][0]["line"].as_u64(), found["matches"][0]["column"].as_u64(), found["matches"][0]["text"].as_str()),
        (Some(1), Some(2), Some("book"))
    );
    assert_eq!(ok(&mut a, "doc_find", json!({ "id": id, "pattern": "book", "case_sensitive": false }))["count"], 3);
    assert_eq!(ok(&mut a, "doc_find", json!({ "id": id, "pattern": "book", "case_sensitive": false, "whole_word": true }))["count"], 2);
    // The same on text and on a file, and the limit.
    assert_eq!(ok(&mut a, "doc_find", json!({ "text": "aaa", "pattern": "a", "limit": 2 }))["more"], true);
    // A regular expression with groups: the replacement is one undoable step.
    let replaced = ok(&mut a, "doc_replace", json!({ "id": id, "pattern": "id=\"(\\w+)\"", "regex": true, "replacement": "key='$1'" }));
    assert_eq!(replaced["replaced"], 2);
    assert_eq!(ok(&mut a, "doc_text", json!({ "id": id }))["text"], "<book key='b1'/>\n<Book key='b2'/>\n<notebook/>");
    ok(&mut a, "doc_undo", json!({ "id": id }));
    assert_eq!(ok(&mut a, "doc_text", json!({ "id": id }))["text"], "<book id=\"b1\"/>\n<Book id=\"b2\"/>\n<notebook/>");
    // In a text search `$` is just a dollar sign.
    ok(&mut a, "doc_replace", json!({ "id": id, "pattern": "notebook", "replacement": "$1" }));
    assert!(ok(&mut a, "doc_text", json!({ "id": id }))["text"].as_str().is_some_and(|t| t.ends_with("<$1/>")));
    // Bad patterns and arguments say what is wrong.
    assert!(err(&mut a, "doc_find", json!({ "id": id, "pattern": "(", "regex": true })).contains("not valid"));
    assert!(err(&mut a, "doc_find", json!({ "id": id, "pattern": "" })).contains("pattern"), "the schema wants a pattern");
    assert!(err(&mut a, "doc_replace", json!({ "id": 99, "pattern": "a", "replacement": "b" })).contains("no document"));
    assert!(err(&mut a, "doc_find", json!({ "pattern": "a" })).contains("exactly one"));
    // Too many matches: nothing is replaced.
    let many = ok(&mut a, "doc_new", json!({ "text": "x".repeat(10_001) }))["id"].as_u64().expect("id");
    assert!(err(&mut a, "doc_replace", json!({ "id": many, "pattern": "x", "replacement": "y" })).contains("nothing was replaced"));
    assert_eq!(ok(&mut a, "doc_text", json!({ "id": many }))["text"].as_str().map(|t| t.chars().all(|c| c == 'x')), Some(true));
    assert!(err(&mut a, "doc_replace", json!({ "id": many, "pattern": "x", "replacement": "y" })).contains("more than 10000 matches"));
    // Few matches, but more to write than may be: the other reason says so.
    let big = ok(&mut a, "doc_new", json!({ "text": "x".repeat(5000) }))["id"].as_u64().expect("id");
    let refused = err(&mut a, "doc_replace", json!({ "id": big, "pattern": "x", "replacement": "y".repeat(4000) }));
    assert!(refused.contains("16 MiB") && refused.contains("nothing was replaced"), "{refused}");
}

#[test]
fn finding_in_a_file_says_when_it_was_not_read_exactly_and_when_a_match_is_cut() {
    let dir = tempfile::tempdir().expect("dir");
    // A file that is not valid UTF-8 is searched as it is read (the bad byte replaced), and the answer says so.
    let bad = file(dir.path(), "bad.xml", b"<a>\xFF needle</a>");
    let mut a = Automation::new();
    let found = ok(&mut a, "doc_find", json!({ "path": bad, "pattern": "needle" }));
    assert_eq!((found["count"].as_u64(), found["lossy"].as_bool()), (Some(1), Some(true)));
    let clean = file(dir.path(), "clean.xml", b"<a>needle</a>");
    let found = ok(&mut a, "doc_find", json!({ "path": clean, "pattern": "needle" }));
    assert_eq!((found["count"].as_u64(), found["lossy"].as_bool()), (Some(1), Some(false)));
    assert_eq!((found["matches"][0]["start"].as_u64(), found["matches"][0]["end"].as_u64()), (Some(3), Some(9)));
    // A match longer than the text reported for it: the positions are the whole match, and the text says it was cut.
    let long = format!("<a>{}</a>", "z".repeat(500));
    let found = ok(&mut a, "doc_find", json!({ "text": long, "pattern": "z+", "regex": true }));
    let m = &found["matches"][0];
    assert_eq!((m["end"].as_u64().zip(m["start"].as_u64()).map(|(e, s)| e - s), m["text_truncated"].as_bool()), (Some(500), Some(true)));
    assert_eq!(m["text"].as_str().map(|t| t.chars().count()), Some(200));
    let found = ok(&mut a, "doc_find", json!({ "text": "<a>zz</a>", "pattern": "z+", "regex": true }));
    assert_eq!(found["matches"][0]["text_truncated"], false);
    // A file outside the root is not searched.
    let inside = dir.path().join("inside");
    std::fs::create_dir(&inside).expect("creates");
    let mut confined = Automation::with_root(&inside).expect("root");
    assert!(call(&mut confined, "doc_find", json!({ "path": clean, "pattern": "needle" })).is_err());
}

#[test]
fn comments_and_selections_work_on_the_structure() {
    let mut a = Automation::new();
    let text = "<lib>\n  <book id=\"b1\">\n    <t>x</t>\n  </book>\n</lib>\n";
    let id = ok(&mut a, "doc_new", json!({ "text": text }))["id"].as_u64().expect("id");
    // Growing from the middle of the value: the word, the value with its quotes, the attribute, the tag.
    let grown = |a: &mut Automation, times: u64| {
        let r = ok(a, "doc_expand_selection", json!({ "id": id, "line": 2, "column": 14, "times": times }));
        let c = &r["carets"][0];
        (c["start"].as_u64().unwrap_or(0), c["end"].as_u64().unwrap_or(0), r["grew"].as_bool())
    };
    let book = text.find("<book").expect("there") as u64;
    assert_eq!(grown(&mut a, 1), (book + 10, book + 12, Some(true)), "the word `b1`");
    assert_eq!(grown(&mut a, 2), (book + 9, book + 13, Some(true)), "`\"b1\"`");
    assert_eq!(grown(&mut a, 3), (book + 6, book + 13, Some(true)), "the attribute `id=\"b1\"`");
    assert_eq!(grown(&mut a, 4), (book, book + 14, Some(true)), "the start tag");
    // Going back: the selection steps back through what it was.
    let r = ok(&mut a, "doc_shrink_selection", json!({ "id": id }));
    assert_eq!((r["shrank"].as_bool(), r["carets"][0]["start"].as_u64()), (Some(true), Some(book + 6)), "back to the attribute");
    // A selection set another way cannot be shrunk.
    ok(&mut a, "doc_select", json!({ "id": id, "line": 1, "column": 1 }));
    assert_eq!(ok(&mut a, "doc_shrink_selection", json!({ "id": id }))["shrank"], false);
    // Comments: lines of a selection, one block; again to bring them back; `--` is refused with nothing changed.
    let done = ok(&mut a, "doc_toggle_comment", json!({ "id": id, "line": 2, "column": 1, "end_line": 4, "end_column": 10 }));
    assert_eq!(done["changed"], true);
    assert_eq!(ok(&mut a, "doc_text", json!({ "id": id }))["text"], "<lib>\n  <!-- <book id=\"b1\">\n    <t>x</t>\n  </book> -->\n</lib>\n");
    let refused = err(&mut a, "doc_toggle_comment", json!({ "id": id, "line": 1, "column": 1, "end_line": 5, "end_column": 1 }));
    assert!(refused.contains("--") && refused.contains("nothing was changed"), "{refused}");
    let placed = ok(&mut a, "doc_select", json!({ "id": id, "line": 3, "column": 2 }));
    err(&mut a, "doc_toggle_comment", json!({ "id": id, "line": 1, "column": 1, "end_line": 5, "end_column": 1 }));
    let still = ok(&mut a, "doc_shrink_selection", json!({ "id": id })); // changes nothing: it only says where the selection is
    assert_eq!(still["carets"], placed["carets"], "a refused call leaves the selection where it was");
    ok(&mut a, "doc_toggle_comment", json!({ "id": id, "line": 2, "column": 1, "end_line": 4, "end_column": 5 }));
    assert_eq!(ok(&mut a, "doc_text", json!({ "id": id }))["text"], text);
    // A rectangle: a selection on each line, and typing edits them all.
    let rows = ok(&mut a, "doc_new", json!({ "text": "abcdef\nab\n\nabcdefgh\n" }))["id"].as_u64().expect("id");
    let rect = ok(&mut a, "doc_select", json!({ "id": rows, "line": 1, "column": 2, "end_line": 4, "end_column": 4, "columns": true }));
    assert_eq!(rect["carets"].as_array().map(Vec::len), Some(4));
    ok(&mut a, "doc_type", json!({ "id": rows, "text": "X" }));
    assert_eq!(ok(&mut a, "doc_text", json!({ "id": rows }))["text"], "aXdef\naX\nX\naXdefgh\n");
    assert!(err(&mut a, "doc_select", json!({ "id": rows, "line": 1, "column": 1, "columns": true })).contains("end_line"));
    // Bad arguments.
    assert!(err(&mut a, "doc_select", json!({ "id": id, "line": 1, "column": 1, "end_line": 2 })).contains("go together"));
    assert!(err(&mut a, "doc_select", json!({ "id": id, "line": 99, "column": 1 })).contains("no line 99"));
    assert!(err(&mut a, "doc_select", json!({ "id": id, "line": 1 })).contains("doc_select"), "the schema wants a column");
}

#[test]
fn what_can_be_folded_is_listed_and_folding_it_hides_lines() {
    let text = "<lib>\n  <book>\n    <t>x</t>\n  </book>\n  <book>\n    <t>y</t>\n  </book>\n</lib>\n";
    let mut a = Automation::new();
    let listed = ok(&mut a, "xml_folds", json!({ "text": text }));
    let folds = listed["folds"].as_array().expect("a list");
    let summary: Vec<(u64, u64, u64, u64, &str)> = folds
        .iter()
        .map(|f| {
            (
                f["first_line"].as_u64().unwrap_or(0),
                f["last_line"].as_u64().unwrap_or(0),
                f["end_line"].as_u64().unwrap_or(0),
                f["level"].as_u64().unwrap_or(0),
                f["kind"].as_str().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(summary, [(1, 7, 8, 1, "element"), (2, 3, 4, 2, "element"), (5, 6, 7, 2, "element")], "lines count from 1");
    // Folding: at a line, everything, a level, and open again. The text itself never changes.
    let id = ok(&mut a, "doc_new", json!({ "text": text }))["id"].as_u64().expect("id");
    let folded = ok(&mut a, "doc_fold", json!({ "id": id, "action": "fold", "line": 3, "column": 5 }));
    assert_eq!(
        (folded["changed"].as_bool(), folded["visible_lines"].as_u64()),
        (Some(true), Some(8)),
        "line 3 is hidden; there are 9 lines in all (the last is empty)"
    );
    assert_eq!(folded["folded"][0]["first_line"], 2);
    assert_eq!(ok(&mut a, "doc_text", json!({ "id": id }))["text"], text);
    let all = ok(&mut a, "doc_fold", json!({ "id": id, "action": "fold_all" }));
    assert_eq!(all["visible_lines"], 3, "line 1, the root's end tag on line 8 and the empty last line");
    let level2 = ok(&mut a, "doc_fold", json!({ "id": id, "action": "level", "level": 2 }));
    assert_eq!(level2["folded"].as_array().map(Vec::len), Some(2));
    assert_eq!(level2["visible_lines"], 7, "one line of each book is hidden");
    let open = ok(&mut a, "doc_fold", json!({ "id": id, "action": "unfold_all" }));
    assert_eq!((open["changed"].as_bool(), open["visible_lines"].as_u64()), (Some(true), Some(9)));
    // `unfold` at a line a fold hides works without the caret having to go there (which would open the fold by itself).
    ok(&mut a, "doc_fold", json!({ "id": id, "action": "fold", "line": 3, "column": 5 }));
    ok(&mut a, "doc_fold", json!({ "id": id, "action": "fold", "line": 2, "column": 1 }));
    assert_eq!(
        ok(&mut a, "doc_fold", json!({ "id": id, "action": "fold", "line": 2, "column": 1 }))["changed"],
        false,
        "folded already: the outer one"
    );
    let opened = ok(&mut a, "doc_fold", json!({ "id": id, "action": "unfold", "line": 3, "column": 1 }));
    assert_eq!(
        (opened["changed"].as_bool(), opened["folded"].as_array().map(Vec::len)),
        (Some(true), Some(1)),
        "line 3 is in the first book, which is folded; it opens: {opened}"
    );
    assert_eq!(listed["truncated"], false);
    let wide = format!("<r>\n{}</r>", "<i>\n x\n</i>\n".repeat(50_010));
    let capped = ok(&mut a, "xml_folds", json!({ "text": wide }));
    assert_eq!((capped["count"].as_u64(), capped["truncated"].as_bool()), (Some(50_000), Some(true)), "the list says it is not all of it");
    // Opening everything needs no analysis: it works on a text that is too large to have one, where folding says why it cannot.
    let huge = ok(&mut a, "doc_new", json!({ "text": "x\n".repeat(17_000_000) }))["id"].as_u64().expect("id");
    assert_eq!(ok(&mut a, "doc_fold", json!({ "id": huge, "action": "unfold_all" }))["changed"], false);
    assert!(err(&mut a, "doc_fold", json!({ "id": huge, "action": "fold_all" })).contains("too large"));
    ok(&mut a, "doc_close", json!({ "id": huge, "discard": true }));
    // Bad arguments say what is wrong.
    assert!(err(&mut a, "doc_fold", json!({ "id": id, "action": "fold", "line": 2 })).contains("go together"));
    assert!(err(&mut a, "doc_fold", json!({ "id": id, "action": "level" })).contains("level is missing"));
    assert!(err(&mut a, "doc_fold", json!({ "id": id, "action": "squash" })).contains("doc_fold"), "the schema lists the actions");
    assert!(err(&mut a, "doc_fold", json!({ "id": id, "action": "fold", "line": 99, "column": 1 })).contains("no line 99"));
    assert!(err(&mut a, "xml_folds", json!({})).contains("exactly one"));
}

#[test]
fn typing_in_the_name_of_a_tag_renames_the_other_tag_of_the_element_too() {
    let mut a = Automation::new();
    let id = ok(&mut a, "doc_new", json!({ "text": "<a>\n  <item id=\"1\">x</item>\n</a>" }))["id"].as_u64().expect("id");
    let text = |a: &mut Automation| ok(a, "doc_text", json!({ "id": id }))["text"].as_str().expect("text").to_owned();
    // After `item` in the start tag.
    ok(&mut a, "doc_type", json!({ "id": id, "text": "s", "line": 2, "column": 8 }));
    assert_eq!(text(&mut a), "<a>\n  <items id=\"1\">x</items>\n</a>");
    // Backspace in the end tag, twice: both names lose two letters, in one undo step.
    ok(&mut a, "doc_key", json!({ "id": id, "key": "Backspace", "times": 2, "line": 2, "column": 25 }));
    assert_eq!(text(&mut a), "<a>\n  <ite id=\"1\">x</ite>\n</a>");
    ok(&mut a, "doc_undo", json!({ "id": id }));
    assert_eq!(text(&mut a), "<a>\n  <items id=\"1\">x</items>\n</a>");
    // A caret that is not in a name types as ever: an attribute value is not a name.
    ok(&mut a, "doc_type", json!({ "id": id, "text": "9", "line": 2, "column": 14 }));
    assert_eq!(text(&mut a), "<a>\n  <items id=\"91\">x</items>\n</a>");
}

#[test]
fn doc_element_renames_wraps_unwraps_selects_and_gives_the_path() {
    let mut a = Automation::new();
    let id = ok(&mut a, "doc_new", json!({ "text": "<a>\n  <b>x</b>\n  <b>y</b>\n</a>" }))["id"].as_u64().expect("id");
    let text = |a: &mut Automation| ok(a, "doc_text", json!({ "id": id }))["text"].as_str().expect("text").to_owned();
    // The path: the second of two brothers has a position.
    let path = ok(&mut a, "doc_element", json!({ "id": id, "action": "xpath", "line": 3, "column": 8 }));
    assert_eq!(path["xpath"], "/a/b[2]");
    // Both tags are renamed, as one undo step.
    ok(&mut a, "doc_element", json!({ "id": id, "action": "rename", "name": "item", "line": 2, "column": 7 }));
    assert_eq!(text(&mut a), "<a>\n  <item>x</item>\n  <b>y</b>\n</a>");
    ok(&mut a, "doc_undo", json!({ "id": id }));
    assert_eq!(text(&mut a), "<a>\n  <b>x</b>\n  <b>y</b>\n</a>");
    // Wrap the text of the second, then take the new tags away again.
    ok(&mut a, "doc_element", json!({ "id": id, "action": "wrap", "name": "w", "line": 3, "column": 6, "end_line": 3, "end_column": 7 }));
    assert_eq!(text(&mut a), "<a>\n  <b>x</b>\n  <b><w>y</w></b>\n</a>");
    ok(&mut a, "doc_element", json!({ "id": id, "action": "unwrap", "line": 3, "column": 10 }));
    assert_eq!(text(&mut a), "<a>\n  <b>x</b>\n  <b>y</b>\n</a>");
    // Select: the carets then span the element.
    let selected = ok(&mut a, "doc_element", json!({ "id": id, "action": "select", "line": 2, "column": 7 }));
    assert_eq!(
        (selected["carets"][0]["line"].as_u64(), selected["carets"][0]["column"].as_u64(), selected["carets"][0]["end_column"].as_u64()),
        (Some(2), Some(3), Some(11)),
        "<b>x</b>"
    );
    // A refused wrap leaves the selection where it was.
    let refused =
        err(&mut a, "doc_element", json!({ "id": id, "action": "wrap", "name": "1 x", "line": 3, "column": 3, "end_line": 3, "end_column": 5 }));
    assert!(refused.contains("not an element name"), "{refused}");
    // The path of the element at the caret, which is the selected one (no position given): the selection is reported and left alone.
    let reported = ok(&mut a, "doc_element", json!({ "id": id, "action": "xpath" }));
    assert_eq!(reported["xpath"], "/a/b[1]");
    assert_eq!(
        (reported["carets"][0]["line"].as_u64(), reported["carets"][0]["column"].as_u64(), reported["carets"][0]["end_column"].as_u64()),
        (Some(2), Some(3), Some(11)),
        "the selection is still <b>x</b>"
    );
    // A name that is not one is refused and changes nothing.
    let refused = err(&mut a, "doc_element", json!({ "id": id, "action": "rename", "name": "1 x", "line": 2, "column": 7 }));
    assert!(refused.contains("not an element name"), "{refused}");
    assert_eq!(text(&mut a), "<a>\n  <b>x</b>\n  <b>y</b>\n</a>");
}

#[test]
fn typing_and_keys_work_as_the_keyboard_does() {
    let mut a = Automation::new();
    let id = ok(&mut a, "doc_new", json!({ "text": "<a b=/>" }))["id"].as_u64().expect("id");
    // A quote after `=` is paired and the closing quote stepped over: the value is typed as it would be by hand.
    let typed = ok(&mut a, "doc_type", json!({ "id": id, "text": "\"v\"", "line": 1, "column": 6 }));
    assert_eq!(typed["typed"], 3);
    assert_eq!(ok(&mut a, "doc_text", json!({ "id": id }))["text"], "<a b=\"v\"/>");
    assert_eq!((typed["carets"][0]["line"].as_u64(), typed["carets"][0]["column"].as_u64()), (Some(1), Some(9)), "after the closing quote");
    // Backspace between an empty pair takes both quotes; a Backspace that is a Backspace takes one character.
    ok(&mut a, "doc_set_text", json!({ "id": id, "text": "<a b=\"\"/>" }));
    let after = ok(&mut a, "doc_key", json!({ "id": id, "key": "Backspace", "line": 1, "column": 7 }));
    assert_eq!(ok(&mut a, "doc_text", json!({ "id": id }))["text"], "<a b=/>");
    assert_eq!(after["carets"][0]["column"], 6);
    ok(&mut a, "doc_key", json!({ "id": id, "key": "Backspace", "times": 2 }));
    assert_eq!(ok(&mut a, "doc_text", json!({ "id": id }))["text"], "<a />", "two characters before the caret");
    // Enter and Tab are keys too, and a line feed or a tab in the typed text are those keys.
    let id2 = ok(&mut a, "doc_new", json!({ "text": "" }))["id"].as_u64().expect("id");
    ok(&mut a, "doc_type", json!({ "id": id2, "text": "<a>" }));
    assert_eq!(ok(&mut a, "doc_text", json!({ "id": id2 }))["text"], "<a></a>", "the end tag is written with the start tag");
    // Enter between them: a line for the content, and the end tag on its own; then a tab is another indent step.
    ok(&mut a, "doc_type", json!({ "id": id2, "text": "\n\t<b/>" }));
    assert_eq!(ok(&mut a, "doc_text", json!({ "id": id2 }))["text"], "<a>\n    <b/>\n</a>");
    // Typing is undone like any other editing: the typing after the Enter is one step, the Enter another, and `<a>` with its end tag the first.
    for (undone, expected) in [("the typing", "<a>\n  \n</a>"), ("the Enter", "<a></a>"), ("the start tag and its end tag", "")] {
        ok(&mut a, "doc_undo", json!({ "id": id2 }));
        assert_eq!(ok(&mut a, "doc_text", json!({ "id": id2 }))["text"], expected, "after undoing {undone}");
    }
    // Line breaks of any kind are one Enter, and a call that cannot be done changes nothing.
    let id3 = ok(&mut a, "doc_new", json!({ "text": "" }))["id"].as_u64().expect("id");
    let typed = ok(&mut a, "doc_type", json!({ "id": id3, "text": "a\r\nb\rc\nd" }));
    assert_eq!((ok(&mut a, "doc_text", json!({ "id": id3 }))["text"].as_str(), typed["typed"].as_u64()), (Some("a\nb\nc\nd"), Some(7)));
    assert!(err(&mut a, "doc_type", json!({ "id": id3, "text": "xy\u{7}z", "line": 1, "column": 1 })).contains("control character"));
    assert_eq!(ok(&mut a, "doc_text", json!({ "id": id3 }))["text"], "a\nb\nc\nd", "nothing was typed before the bad character was found");
    assert!(err(&mut a, "doc_type", json!({ "id": id3, "text": "x", "line": 1 })).contains("go together"));
    assert!(err(&mut a, "doc_key", json!({ "id": id3, "key": "Enter", "column": 1 })).contains("go together"));
    // Bad arguments say what is wrong.
    assert!(err(&mut a, "doc_type", json!({ "id": id2, "text": "a\u{7}b" })).contains("control character"));
    assert!(err(&mut a, "doc_type", json!({ "id": id2, "text": "" })).contains("doc_type"), "the schema wants something to type");
    assert!(err(&mut a, "doc_key", json!({ "id": id2, "key": "F13" })).contains("doc_key"), "the schema lists the keys");
    assert!(err(&mut a, "doc_type", json!({ "id": id2, "text": "x", "line": 9, "column": 1 })).contains("no line 9"));
    assert!(err(&mut a, "doc_type", json!({ "id": 999, "text": "x" })).contains("no document"));
}

#[test]
fn the_matching_tag_is_found_from_a_position_in_text_a_file_or_a_document() {
    let text = "<lib>\n  <book id=\"1\">x</book>\n  <br/>\n</lib>";
    let mut a = Automation::new();
    // From just before the `b` of `book` (line 2, column 4): the end tag, and where the caret goes (the same place in its name).
    let found = ok(&mut a, "xml_matching_tag", json!({ "text": text, "line": 2, "column": 4 }));
    let m = &found["match"];
    assert_eq!((m["tag"]["line"].as_u64(), m["tag"]["column"].as_u64()), (Some(2), Some(3)));
    assert_eq!((m["partner"]["line"].as_u64(), m["partner"]["column"].as_u64(), m["partner"]["end_column"].as_u64()), (Some(2), Some(17), Some(24)));
    assert_eq!(
        (m["partner_name"]["start"].as_u64(), m["partner_name"]["end"].as_u64()),
        (text.rfind("book").map(|i| i as u64), text.rfind("book").map(|i| i as u64 + 4))
    );
    assert_eq!(
        (m["jump_to"]["line"].as_u64(), m["jump_to"]["column"].as_u64()),
        (Some(2), Some(19)),
        "the start of the name in the end tag, as in the start tag"
    );
    let inside_name = ok(&mut a, "xml_matching_tag", json!({ "text": text, "line": 2, "column": 6 }));
    assert_eq!(inside_name["match"]["jump_to"]["column"].as_u64(), Some(21), "two letters into the name, two letters into the other");
    // By byte offset, and the same from the end tag back to the start tag.
    let end_tag = text.find("</book>").expect("there") as u64;
    let back = ok(&mut a, "xml_matching_tag", json!({ "text": text, "offset": end_tag + 1 }));
    assert_eq!(back["match"]["partner"]["start"].as_u64(), text.find("<book").map(|i| i as u64));
    // No pair: an empty-element tag, text, and a position that is not in the text.
    assert!(ok(&mut a, "xml_matching_tag", json!({ "text": text, "line": 3, "column": 4 }))["match"].is_null());
    assert!(
        ok(&mut a, "xml_matching_tag", json!({ "text": text, "line": 2, "column": 1 }))["match"].is_null(),
        "in white space, away from every tag"
    );
    assert!(err(&mut a, "xml_matching_tag", json!({ "text": text, "line": 9, "column": 1 })).contains("no line 9"));
    assert!(err(&mut a, "xml_matching_tag", json!({ "text": text, "offset": 9999 })).contains("offset"));
    assert!(err(&mut a, "xml_matching_tag", json!({ "text": text })).contains("line is missing"));
    // A file and an open document give the same answer; one that is not valid UTF-8 says it was read lossily.
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "a.xml", text.as_bytes());
    assert_eq!(ok(&mut a, "xml_matching_tag", json!({ "path": path, "line": 2, "column": 4 }))["match"], *m);
    let id = ok(&mut a, "doc_new", json!({ "text": text }))["id"].as_u64().expect("id");
    assert_eq!(ok(&mut a, "xml_matching_tag", json!({ "id": id, "line": 2, "column": 4 }))["match"], *m);
    let bad = file(dir.path(), "bad.xml", b"<a>\xFF</a>");
    assert_eq!(ok(&mut a, "xml_matching_tag", json!({ "path": bad, "offset": 1 }))["lossy"], true);
    // The root keeps it to its files.
    let inside = dir.path().join("inside");
    std::fs::create_dir(&inside).expect("creates");
    let mut confined = Automation::with_root(&inside).expect("root");
    assert!(call(&mut confined, "xml_matching_tag", json!({ "path": path, "offset": 1 })).is_err());
}

#[test]
fn a_deleted_file_or_another_route_to_it_does_not_get_around_the_save_checks() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "a.xml", b"<a/>");
    let mut a = Automation::new();
    let id = ok(&mut a, "doc_open", json!({ "path": path }))["id"].as_u64().expect("id");
    ok(&mut a, "doc_set_text", json!({ "id": id, "text": "<b/>" }));
    // The file was deleted meanwhile: saving does not quietly bring it back.
    std::fs::remove_file(&path).expect("removes");
    let refused = err(&mut a, "doc_save", json!({ "id": id }));
    assert!(refused.contains("is gone") && refused.contains("force"), "{refused}");
    assert!(!Path::new(&path).exists());
    ok(&mut a, "doc_save", json!({ "id": id, "force": true }));
    assert_eq!(std::fs::read(&path).expect("reads"), b"<b/>");
}

#[cfg(unix)]
#[test]
fn a_link_to_the_file_is_the_same_file_for_the_save_checks() {
    let dir = tempfile::tempdir().expect("dir");
    let bad = file(dir.path(), "bad.xml", b"<a>\xFF</a>");
    let link = dir.path().join("link.xml");
    std::os::unix::fs::symlink(&bad, &link).expect("links");
    let mut a = Automation::new();
    let id = ok(&mut a, "doc_open", json!({ "path": bad }))["id"].as_u64().expect("id");
    // Opening the file through the link shows the same document.
    assert_eq!(ok(&mut a, "doc_open", json!({ "path": link.to_str() }))["id"].as_u64(), Some(id));
    // Saving to the link is saving over the file whose bytes were not valid: refused.
    assert!(err(&mut a, "doc_save", json!({ "id": id, "path": link.to_str() })).contains("not valid"));
    assert_eq!(std::fs::read(&bad).expect("reads"), b"<a>\xFF</a>");
    // And a file that changed on disk is noticed through the link too.
    let good = file(dir.path(), "good.xml", b"<a/>");
    let good_link = dir.path().join("good-link.xml");
    std::os::unix::fs::symlink(&good, &good_link).expect("links");
    let id = ok(&mut a, "doc_open", json!({ "path": good }))["id"].as_u64().expect("id");
    ok(&mut a, "doc_set_text", json!({ "id": id, "text": "<mine/>" }));
    std::fs::write(&good, b"<theirs, longer/>").expect("writes");
    assert!(err(&mut a, "doc_save", json!({ "id": id, "path": good_link.to_str() })).contains("changed on disk"));
    assert_eq!(std::fs::read(&good).expect("reads"), b"<theirs, longer/>");
}

#[test]
fn the_files_that_were_opened_are_listed_newest_first_and_the_list_can_be_cleared() {
    let dir = tempfile::tempdir().expect("dir");
    let (first, second) = (file(dir.path(), "first.xml", b"<a/>"), file(dir.path(), "second.xml", b"<b/>"));
    let mut a = Automation::new();
    assert_eq!(ok(&mut a, "recent_files", json!({}))["count"], 0);
    ok(&mut a, "doc_open", json!({ "path": first }));
    ok(&mut a, "doc_open", json!({ "path": second }));
    ok(&mut a, "doc_new", json!({ "text": "<c/>" })); // no file, nothing to remember
    let listed = ok(&mut a, "recent_files", json!({}));
    let names: Vec<String> = listed["files"]
        .as_array()
        .expect("a list")
        .iter()
        .filter_map(|f| f.as_str())
        .map(|f| f.rsplit(['/', '\\']).next().unwrap_or(f).to_owned())
        .collect();
    assert_eq!(names, ["second.xml", "first.xml"]);
    // Opening the first again moves it to the front; reading with `clear` empties the list afterwards.
    ok(&mut a, "doc_open", json!({ "path": first }));
    let cleared = ok(&mut a, "recent_files", json!({ "clear": true }));
    assert_eq!((cleared["count"].as_u64(), cleared["cleared"].as_bool()), (Some(2), Some(true)));
    assert!(cleared["files"][0].as_str().is_some_and(|f| f.ends_with("first.xml")));
    assert_eq!(ok(&mut a, "recent_files", json!({}))["count"], 0);
    // A file written by doc_save is a recent file too (save as: a new path).
    let id = ok(&mut a, "doc_new", json!({ "text": "<d/>" }))["id"].as_u64().expect("id");
    let target = dir.path().join("saved.xml");
    ok(&mut a, "doc_save", json!({ "id": id, "path": target.to_str().expect("utf-8") }));
    let listed = ok(&mut a, "recent_files", json!({}));
    assert!(listed["files"][0].as_str().is_some_and(|f| f.ends_with("saved.xml")), "{listed}");
}

#[test]
fn line_endings_and_indentation_are_set_without_touching_anything_else() {
    let mut a = Automation::new();
    let id = ok(&mut a, "doc_new", json!({ "text": "<a>\n<b/>\r\n</a>" }))["id"].as_u64().expect("id");
    let text = |a: &mut Automation| ok(a, "doc_text", json!({ "id": id }))["text"].as_str().expect("text").to_owned();
    let changed = ok(&mut a, "doc_set_line_endings", json!({ "id": id, "eol": "crlf" }));
    assert_eq!(changed["changed"], 1, "only the one LF");
    assert_eq!(text(&mut a), "<a>\r\n<b/>\r\n</a>");
    ok(&mut a, "doc_undo", json!({ "id": id }));
    assert_eq!(text(&mut a), "<a>\n<b/>\r\n</a>");
    assert!(err(&mut a, "doc_set_line_endings", json!({ "id": id, "eol": "cr" })).contains("eol"), "an ending the editor does not write is refused");
    let indent = ok(&mut a, "doc_set_indent", json!({ "id": id, "style": "spaces", "size": 4 }));
    assert_eq!((indent["style"].as_str(), indent["size"].as_u64()), (Some("spaces"), Some(4)));
    assert_eq!(text(&mut a), "<a>\n<b/>\r\n</a>", "the text is not changed");
    // What Enter writes follows: after a start tag the new line is one level in, with four spaces now, with a tab after the next call.
    ok(&mut a, "doc_key", json!({ "id": id, "key": "Enter", "line": 1, "column": 4 }));
    assert!(text(&mut a).starts_with("<a>\n    \n"), "four spaces: {:?}", text(&mut a));
    ok(&mut a, "doc_undo", json!({ "id": id }));
    ok(&mut a, "doc_set_indent", json!({ "id": id, "style": "tabs" }));
    ok(&mut a, "doc_key", json!({ "id": id, "key": "Enter", "line": 1, "column": 4 }));
    assert!(text(&mut a).starts_with("<a>\n\t\n"), "a tab: {:?}", text(&mut a));
    assert!(err(&mut a, "doc_set_indent", json!({ "id": id, "style": "tabs", "size": 99 })).contains("size"));
}

#[test]
fn quick_fixes_are_listed_and_one_is_applied_as_a_single_undoable_edit() {
    let mut a = Automation::new();
    let id = ok(&mut a, "doc_new", json!({ "text": "<a>\n<b></c>\n<d x=1/>\n</a>" }))["id"].as_u64().expect("id");
    let text = |a: &mut Automation| ok(a, "doc_text", json!({ "id": id }))["text"].as_str().expect("text").to_owned();
    let all = ok(&mut a, "doc_quick_fix", json!({ "id": id }));
    assert_eq!(all["count"], 2, "{all}");
    assert_eq!(all["fixes"][0]["title"], "Rename the end tag to </b>");
    assert_eq!((all["fixes"][0]["line"].as_u64(), all["fixes"][0]["column"].as_u64()), (Some(2), Some(4)));
    // Only the problems at a place.
    let at = ok(&mut a, "doc_quick_fix", json!({ "id": id, "line": 3, "column": 6 }));
    assert_eq!(at["count"], 1);
    assert_eq!(at["fixes"][0]["title"], "Put the value in quotes");
    assert_eq!(text(&mut a), "<a>\n<b></c>\n<d x=1/>\n</a>", "listing changes nothing");
    let done = ok(&mut a, "doc_quick_fix", json!({ "id": id, "apply": 0 }));
    assert_eq!(done["applied"], "Rename the end tag to </b>");
    assert_eq!(text(&mut a), "<a>\n<b></b>\n<d x=1/>\n</a>");
    ok(&mut a, "doc_undo", json!({ "id": id }));
    assert_eq!(text(&mut a), "<a>\n<b></c>\n<d x=1/>\n</a>", "one step");
    assert!(err(&mut a, "doc_quick_fix", json!({ "id": id, "apply": 9 })).contains("no fix number 9"));
    assert!(err(&mut a, "doc_quick_fix", json!({ "id": id, "line": 1 })).contains("line and column go together"));
}

#[test]
fn json_is_read_by_the_same_tools_as_xml_by_name_or_by_the_language_given() {
    let dir = tempfile::tempdir().expect("dir");
    let text = "{\n  \"a\": {\n    \"b\": [1, 2]\n  } // c\n}";
    let json_path = file(dir.path(), "x.json", text.as_bytes());
    let jsonc_path = file(dir.path(), "x.jsonc", text.as_bytes());
    let mut a = Automation::new();
    // By name: strict JSON complains about the comment, JSONC does not.
    let strict = ok(&mut a, "xml_wellformed", json!({ "path": json_path }));
    assert_eq!((strict["errors"].as_u64(), strict["problems"][0]["code"].as_str()), (Some(1), Some("json.comment-not-allowed")));
    assert_eq!(ok(&mut a, "xml_wellformed", json!({ "path": jsonc_path }))["errors"].as_u64(), Some(0));
    // By the language given for text.
    assert_eq!(ok(&mut a, "xml_wellformed", json!({ "text": text, "language": "jsonc" }))["errors"].as_u64(), Some(0));
    assert_eq!(ok(&mut a, "xml_wellformed", json!({ "text": text, "language": "json" }))["errors"].as_u64(), Some(1));
    assert!(err(&mut a, "xml_wellformed", json!({ "text": text, "language": "toml" })).contains("language"), "an unknown language is refused");
    let outline = ok(&mut a, "xml_outline", json!({ "text": text, "language": "jsonc" }));
    let names: Vec<_> = outline["items"].as_array().expect("items").iter().map(|i| (i["depth"].as_u64(), i["name"].as_str())).collect();
    assert_eq!(names, [(Some(0), Some("a")), (Some(1), Some("b"))]);
    let folds = ok(&mut a, "xml_folds", json!({ "text": text, "language": "jsonc" }));
    let kinds: Vec<_> = folds["folds"].as_array().expect("folds").iter().map(|f| (f["first_line"].as_u64(), f["kind"].as_str())).collect();
    assert_eq!(kinds, [(Some(1), Some("object")), (Some(2), Some("object"))]);
    // Without a language the text is XML, as before.
    assert_eq!(ok(&mut a, "xml_wellformed", json!({ "text": text }))["well_formed"].as_bool(), Some(false));
    // An open document is read by its name, and folds by doc_fold are those of JSON.
    let id = ok(&mut a, "doc_open", json!({ "path": jsonc_path }))["id"].as_u64().expect("an id");
    assert_eq!(ok(&mut a, "xml_wellformed", json!({ "id": id }))["errors"].as_u64(), Some(0));
    let folded = ok(&mut a, "doc_fold", json!({ "id": id, "action": "fold_all" }));
    assert_eq!(folded["changed"].as_bool(), Some(true), "{folded}");
    let ranges: Vec<_> = folded["folded"].as_array().expect("folded").iter().map(|f| (f["first_line"].as_u64(), f["last_line"].as_u64())).collect();
    assert_eq!(ranges, [(Some(1), Some(4)), (Some(2), Some(3))], "the root object and the nested one: {folded}");
}

#[test]
fn json_is_formatted_by_xml_format_in_text_in_files_and_in_documents() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "x.json", b"{\"a\":[1,2],\"b\":{}}\n");
    let mut a = Automation::new();
    let r = ok(&mut a, "xml_format", json!({ "text": "{\"a\":[1]}", "language": "json" }));
    assert_eq!((r["text"].as_str(), r["changed"].as_bool()), (Some("{\n  \"a\": [\n    1\n  ]\n}"), Some(true)));
    let r = ok(&mut a, "xml_format", json!({ "text": "{ \"a\" : [ 1 ] }", "language": "json", "minify": true }));
    assert_eq!(r["text"].as_str(), Some("{\"a\":[1]}"));
    let r = ok(&mut a, "xml_format", json!({ "path": path, "indent": 4 }));
    assert_eq!(r["text"].as_str(), Some("{\n    \"a\": [\n        1,\n        2\n    ],\n    \"b\": {}\n}\n"));
    assert_eq!(std::fs::read(&path).expect("reads"), b"{\"a\":[1,2],\"b\":{}}\n", "a file is never written");
    let id = ok(&mut a, "doc_open", json!({ "path": path }))["id"].as_u64().expect("an id");
    assert_eq!(ok(&mut a, "xml_format", json!({ "id": id }))["changed"].as_bool(), Some(true));
    assert!(ok(&mut a, "doc_text", json!({ "id": id }))["text"].as_str().is_some_and(|t| t.starts_with("{\n  \"a\": [\n    1,")));
    // Errors say where, and JSON Lines refuse indenting.
    let e = err(&mut a, "xml_format", json!({ "text": "{\"a\": }", "language": "json" }));
    assert!(e.contains("not well-formed") && e.contains("line 1"), "{e}");
    let e = err(&mut a, "xml_format", json!({ "text": "{\"a\":1}\n", "language": "jsonl" }));
    assert!(e.contains("JSON Lines"), "{e}");
    let r = ok(&mut a, "xml_format", json!({ "text": "{ \"a\" : 1 }\n\n{ \"b\" : 2 }\n", "language": "jsonl", "minify": true }));
    assert_eq!(r["text"].as_str(), Some("{\"a\":1}\n\n{\"b\":2}\n"));
}

#[test]
fn bson_files_are_read_by_their_text_and_saved_as_bytes() {
    let dir = tempfile::tempdir().expect("dir");
    let docs = vec![xmlcraft_bson::Document {
        fields: vec![("a".into(), xmlcraft_bson::Bson::Int32(7)), ("o".into(), xmlcraft_bson::Bson::Document(Default::default()))],
    }];
    let path = file(dir.path(), "x.bson", &xmlcraft_bson::write_all(&docs).expect("writes"));
    let mut a = Automation::new();
    let outline = ok(&mut a, "xml_outline", json!({ "path": path }));
    let names: Vec<_> = outline["items"].as_array().expect("items").iter().map(|i| i["name"].as_str().unwrap_or_default()).collect();
    assert_eq!(names, ["a", "o"], "a typed value is one item, not a container: {outline}");
    assert_eq!(outline["items"][0]["label"].as_str(), Some("7 ($numberInt)"));
    let id = ok(&mut a, "doc_open", json!({ "path": path }))["id"].as_u64().expect("an id");
    let text = ok(&mut a, "doc_text", json!({ "id": id }))["text"].as_str().expect("text").to_owned();
    assert!(text.contains("\"$numberInt\""), "{text}");
    ok(&mut a, "doc_set_text", json!({ "id": id, "text": text.replace("\"7\"", "\"9\"") }));
    ok(&mut a, "doc_save", json!({ "id": id }));
    let back = xmlcraft_bson::read_all(&std::fs::read(&path).expect("reads")).expect("reads");
    assert_eq!(back[0].fields[0].1, xmlcraft_bson::Bson::Int32(9));
    ok(&mut a, "doc_set_text", json!({ "id": id, "text": "{\"a\": {\"$oid\": \"zz\"}}" }));
    let e = err(&mut a, "doc_save", json!({ "id": id }));
    assert!(e.contains("Extended JSON"), "{e}");
    assert_eq!(xmlcraft_bson::read_all(&std::fs::read(&path).expect("reads")).expect("still valid")[0].fields[0].1, xmlcraft_bson::Bson::Int32(9));
    let fake = file(dir.path(), "fake.bson", b"nope");
    assert!(err(&mut a, "xml_outline", json!({ "path": fake })).contains("not a BSON file"));
    assert!(err(&mut a, "doc_open", json!({ "path": fake })).contains("not a BSON file"));
}

#[test]
fn yaml_is_read_by_the_same_tools_as_xml_and_json() {
    let mut a = Automation::new();
    let text = "name: x\nitems:\n  - id: 1\n  - id: 2\n";
    let r = ok(&mut a, "xml_wellformed", json!({ "text": text, "language": "yaml" }));
    assert_eq!((r["errors"].as_u64(), r["well_formed"].as_bool()), (Some(0), Some(true)));
    let outline = ok(&mut a, "xml_outline", json!({ "text": text, "language": "yaml" }));
    let names: Vec<_> = outline["items"].as_array().expect("items").iter().map(|i| i["name"].as_str().unwrap_or_default()).collect();
    assert_eq!(names, ["name", "items", "[0]", "id", "[1]", "id"]);
    let folds = ok(&mut a, "xml_folds", json!({ "text": text, "language": "yaml" }));
    assert_eq!(folds["folds"].as_array().map(Vec::len), Some(1), "the list of items; its entries are one line each");
    assert_eq!(
        (folds["folds"][0]["first_line"].as_u64(), folds["folds"][0]["last_line"].as_u64(), folds["folds"][0]["kind"].as_str()),
        (Some(2), Some(4), Some("array"))
    );
    let bad = ok(&mut a, "xml_wellformed", json!({ "text": "a: [1\nb: 2\n", "language": "yaml" }));
    assert_eq!(bad["problems"][0]["code"].as_str(), Some("yaml.syntax-error"));
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "x.yml", text.as_bytes());
    assert_eq!(ok(&mut a, "xml_outline", json!({ "path": path }))["count"].as_u64(), Some(6), "read by its name");
    let e = err(&mut a, "xml_format", json!({ "text": text, "language": "yaml" }));
    assert!(e.contains("YAML is not reformatted"), "{e}");
}

#[test]
fn xinclude_is_expanded_from_files_next_to_the_document_and_from_text_with_a_base() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "a.xml", br#"<a xmlns:xi="http://www.w3.org/2001/XInclude"><xi:include href="b.xml"/></a>"#);
    file(dir.path(), "b.xml", b"<b>in</b>");
    let mut a = Automation::new();
    let expanded = ok(&mut a, "xinclude_expand", json!({ "path": path }));
    assert_eq!(expanded["text"], "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"><b>in</b></a>");
    assert_eq!(expanded["includes"], 1);
    assert_eq!(expanded["edits"], json!([{ "start": 46, "end": 72, "text": "<b>in</b>" }]), "one edit, of the include's range");
    let text =
        "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"><xi:include href=\"b.txt\" parse=\"text\"><xi:fallback>no</xi:fallback></xi:include></a>";
    file(dir.path(), "b.txt", b"a < b");
    let base = format!("{}/", dir.path().display());
    let from_text = ok(&mut a, "xinclude_expand", json!({ "text": text, "base": base }));
    assert_eq!(from_text["text"], "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\">a &lt; b</a>");
    let fallback = ok(&mut a, "xinclude_expand", json!({ "text": text, "base": "/no/such/folder" }));
    assert_eq!(fallback["text"], "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\">no</a>");
    assert!(err(&mut a, "xinclude_expand", json!({ "text": "<a>" })).contains("not well-formed"));
    assert!(
        err(
            &mut a,
            "xinclude_expand",
            json!({ "text": "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"><xi:include href=\"b.xml\" xpointer=\"/b\"/></a>" })
        )
        .contains("xpointer")
    );
}

#[test]
fn xinclude_does_not_reach_a_file_outside_the_root() {
    let parent = tempfile::tempdir().expect("parent");
    let root = parent.path().join("root");
    std::fs::create_dir(&root).expect("root");
    file(parent.path(), "outside.xml", b"<secret>do not read</secret>");
    let doc = file(&root, "a.xml", br#"<a xmlns:xi="http://www.w3.org/2001/XInclude"><xi:include href="../outside.xml"/></a>"#);
    let mut a = Automation::with_root(&root).expect("root");
    let e = err(&mut a, "xinclude_expand", json!({ "path": doc }));
    assert!(e.contains("outside.xml") && !e.contains("do not read"), "{e}");
    let with_fallback = file(
        &root,
        "b.xml",
        br#"<a xmlns:xi="http://www.w3.org/2001/XInclude"><xi:include href="../outside.xml"><xi:fallback>kept out</xi:fallback></xi:include></a>"#,
    );
    let r = ok(&mut a, "xinclude_expand", json!({ "path": with_fallback }));
    assert_eq!(r["text"], "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\">kept out</a>", "{r}");
    // The same file is read when the root allows it.
    let mut open = Automation::new();
    let r = ok(&mut open, "xinclude_expand", json!({ "path": doc }));
    assert!(r["text"].as_str().is_some_and(|t| t.contains("do not read")), "{r}");
}
