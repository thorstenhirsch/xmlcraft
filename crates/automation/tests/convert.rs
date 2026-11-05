//! `data_convert`: documents converted between XML, JSON, YAML and BSON, read through the root confinement.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn convert(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    a.call("data_convert", &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

#[test]
fn xml_text_becomes_json_and_json_becomes_yaml() {
    let mut a = Automation::new();
    let r = convert(&mut a, json!({ "text": "<order id=\"7\"><item>Tea</item></order>", "from": "xml", "to": "json" })).expect("runs");
    let parsed: Value = serde_json::from_str(r["text"].as_str().expect("text")).expect("json");
    assert_eq!(parsed, json!({ "order": { "@id": "7", "item": "Tea" } }), "{r}");
    assert_eq!((r["from"].clone(), r["to"].clone()), (json!("xml"), json!("json")));
    let y = convert(&mut a, json!({ "text": "{\"a\": [1, 2], \"b\": \"x\"}", "from": "json", "to": "yaml" })).expect("runs");
    assert_eq!(y["text"], "a:\n  - 1\n  - 2\nb: x\n", "{y}");
}

#[test]
fn the_options_change_the_result() {
    let mut a = Automation::new();
    let xml = "<a n=\"1\"><b>2</b></a>";
    let compact = convert(&mut a, json!({ "text": xml, "from": "xml", "to": "json", "indent": 0, "guess_types": true, "attribute_prefix": "_" }))
        .expect("runs");
    assert_eq!(compact["text"], "{\"a\":{\"_n\":1,\"b\":2}}", "{compact}");
    let standard = convert(&mut a, json!({ "text": "{\"a\": 1}", "from": "json", "to": "xml", "standard_mapping": true })).expect("runs");
    assert!(standard["text"].as_str().expect("text").contains("http://www.w3.org/2005/xpath-functions"), "{standard}");
    let natural = convert(&mut a, json!({ "text": "{\"a\": 1}", "from": "json", "to": "xml", "declaration": false })).expect("runs");
    assert_eq!(natural["text"].as_str().expect("text").trim(), "<a>1</a>");
}

#[test]
fn what_the_target_cannot_hold_is_a_warning_and_a_bad_text_is_an_error() {
    let mut a = Automation::new();
    let r = convert(&mut a, json!({ "text": "{\"id\": {\"$oid\": \"0123456789abcdef01234567\"}}", "from": "bson", "to": "json" })).expect("runs");
    assert!(!r["warnings"].as_array().expect("warnings").is_empty(), "{r}");
    let err = convert(&mut a, json!({ "text": "{\"a\": ", "from": "json", "to": "yaml" })).expect_err("not json");
    assert!(err.to_string().contains("json") && err.to_string().contains("yaml"), "{err}");
}

#[test]
fn the_arguments_are_checked() {
    let mut a = Automation::new();
    for args in [
        json!({ "text": "{}", "from": "json" }),
        json!({ "text": "{}", "to": "json" }),
        json!({ "text": "{}", "from": "toml", "to": "json" }),
        json!({ "text": "{}", "from": "json", "to": "xml", "indent": 99 }),
        json!({ "text": "{}", "from": "json", "to": "xml", "root": "" }),
        json!({ "text": "{}", "from": "json", "to": "xml", "declaration": "yes" }),
        json!({ "from": "json", "to": "xml" }),
        json!({ "text": "{}", "path": "x.json", "from": "json", "to": "xml" }),
    ] {
        assert!(matches!(convert(&mut a, args.clone()), Err(ToolError::InvalidArgs(_))), "{args}");
    }
}

#[test]
fn a_file_is_read_by_its_name_inside_the_root_only() {
    let inside = tempfile::tempdir().expect("dir");
    let outside = tempfile::tempdir().expect("dir");
    std::fs::write(inside.path().join("d.json"), "{\"k\": true}").expect("writes");
    std::fs::write(outside.path().join("s.json"), "{\"secret\": 1}").expect("writes");
    let mut a = Automation::with_root(inside.path()).expect("root");
    let r = convert(&mut a, json!({ "path": "d.json", "to": "xml", "declaration": false })).expect("runs");
    assert_eq!(r["from"], "json");
    assert_eq!(r["text"].as_str().expect("text").trim(), "<k>true</k>");
    // An open document is read by its name too.
    let opened = a.call("doc_open", &json!({ "path": "d.json" })).expect("opens");
    let Some(Content::Json(opened)) = opened.into_iter().next() else { panic!("a result") };
    let r = convert(&mut a, json!({ "id": opened["id"], "to": "yaml" })).expect("runs");
    assert_eq!(r["text"], "k: true\n");
    let err = convert(&mut a, json!({ "path": outside.path().join("s.json").to_str().expect("utf-8"), "to": "yaml" })).expect_err("outside the root");
    assert!(err.to_string().contains("outside") || err.to_string().contains("root"), "{err}");
}
