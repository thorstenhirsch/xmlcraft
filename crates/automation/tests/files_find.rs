//! `files_find`, through the tool table: a folder inside the root is searched, files outside it are not, and bad arguments are refused.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    a.call("files_find", &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("dir");
    std::fs::create_dir_all(dir.path().join("sub")).expect("dirs");
    std::fs::create_dir_all(dir.path().join(".git")).expect("dirs");
    std::fs::write(dir.path().join("a.xml"), "<a>\n  <b>Needle</b>\n</a>\n").expect("writes");
    std::fs::write(dir.path().join("sub/c.json"), "{\"k\": \"needle\"}").expect("writes");
    std::fs::write(dir.path().join(".git/x.xml"), "needle").expect("writes");
    dir
}

#[test]
fn a_folder_is_searched_with_the_editors_matcher_and_a_name_pattern() {
    let dir = project();
    let mut a = Automation::with_root(dir.path()).expect("root");
    let root = dir.path().to_str().expect("utf-8");
    let r = call(&mut a, json!({ "folder": root, "pattern": "needle", "case_sensitive": false })).expect("runs");
    assert_eq!((r["files_with_hits"].as_u64(), r["hits"].as_u64(), r["truncated"].as_bool()), (Some(2), Some(2), Some(false)), "{r}");
    assert_eq!(r["files"][0]["path"].as_str(), Some("a.xml"));
    assert_eq!(r["files"][0]["hits"][0], json!({ "line": 2, "column": 6, "text": "  <b>Needle</b>" }));
    let r = call(&mut a, json!({ "folder": root, "pattern": "needle" })).expect("runs");
    assert_eq!(r["hits"].as_u64(), Some(1), "case-sensitive by default");
    let r = call(&mut a, json!({ "folder": root, "pattern": "n.edle", "regex": true, "glob": "*.json" })).expect("runs");
    assert_eq!(r["files"][0]["path"].as_str(), Some("sub/c.json"), "{r}");
    let r = call(&mut a, json!({ "folder": root, "pattern": "needle", "case_sensitive": false, "max_hits": 1 })).expect("runs");
    assert_eq!((r["hits"].as_u64(), r["truncated"].as_bool()), (Some(1), Some(true)));
}

#[test]
fn what_cannot_be_searched_is_refused() {
    let dir = project();
    let outside = project();
    let mut a = Automation::with_root(dir.path()).expect("root");
    let root = dir.path().to_str().expect("utf-8");
    assert!(matches!(call(&mut a, json!({ "folder": root, "pattern": "" })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(call(&mut a, json!({ "folder": root, "pattern": "(", "regex": true })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(call(&mut a, json!({ "folder": root, "pattern": "x", "max_hits": 0 })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(call(&mut a, json!({ "folder": root, "pattern": "x", "regex": "yes" })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(call(&mut a, json!({ "pattern": "x" })), Err(ToolError::InvalidArgs(_))));
    assert!(call(&mut a, json!({ "folder": dir.path().join("a.xml").to_str().expect("utf-8"), "pattern": "x" })).is_err(), "a file is not a folder");
    assert!(call(&mut a, json!({ "folder": outside.path().to_str().expect("utf-8"), "pattern": "needle" })).is_err(), "outside the root");
}
