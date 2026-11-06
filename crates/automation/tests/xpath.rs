//! `xpath_eval`, read through the root confinement.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn eval(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    a.call("xpath_eval", &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

#[test]
fn nodes_and_values_are_listed_with_places() {
    let mut a = Automation::new();
    let doc = "<r>\n  <i n=\"1\">a</i>\n  <i n=\"2\">b</i>\n</r>";
    let r = eval(&mut a, json!({ "text": doc, "expression": "/r/i[@n > 1]" })).expect("runs");
    assert_eq!((r["count"].as_u64(), r["truncated"].as_bool(), r["engine"].as_str()), (Some(1), Some(false), Some("XPath 3.1")), "{r}");
    let item = &r["items"][0];
    assert_eq!(
        (item["kind"].as_str(), item["path"].as_str(), item["line"].as_u64(), item["column"].as_u64()),
        (Some("element"), Some("/r[1]/i[2]"), Some(3), Some(3))
    );
    assert_eq!(item["text"], "<i n=\"2\">b</i>");
    let sum = eval(&mut a, json!({ "text": doc, "expression": "sum(//i/@n)" })).expect("runs");
    assert_eq!((sum["items"][0]["kind"].as_str(), sum["items"][0]["text"].as_str()), (Some("xs:double"), Some("3")), "{sum}");
    let capped = eval(&mut a, json!({ "text": doc, "expression": "1 to 10", "max_items": 3 })).expect("runs");
    assert_eq!((capped["count"].as_u64(), capped["truncated"].as_bool(), capped["items"].as_array().map(Vec::len)), (Some(10), Some(true), Some(3)));
}

#[test]
fn namespaces_versions_and_errors() {
    let mut a = Automation::new();
    let doc = "<r xmlns:p=\"urn:p\"><p:j/></r>";
    let r = eval(&mut a, json!({ "text": doc, "expression": "/r/p:j", "namespaces": { "p": "urn:p" }, "version": "2.0" })).expect("runs");
    assert_eq!((r["items"][0]["name"].as_str(), r["version"].as_str()), (Some("Q{urn:p}j"), Some("2.0")));
    let e = eval(&mut a, json!({ "text": doc, "expression": "/r[" })).expect_err("syntax");
    assert!(e.to_string().contains("compile error") && e.to_string().contains("XPST0003"), "{e}");
    let e = eval(&mut a, json!({ "text": doc, "expression": "1 div 0" })).expect_err("run");
    assert!(e.to_string().contains("run error") && e.to_string().contains("FOAR0001"), "{e}");
    let e = eval(&mut a, json!({ "text": "<a>", "expression": "/" })).expect_err("document");
    assert!(e.to_string().contains("document error"), "{e}");
    for bad in [
        json!({ "text": "<a/>" }),
        json!({ "text": "<a/>", "expression": "/", "version": "4.0" }),
        json!({ "text": "<a/>", "expression": "/", "namespaces": { "p": 1 } }),
        json!({ "text": "<a/>", "expression": "/", "namespaces": [] }),
        json!({ "text": "<a/>", "expression": "/", "max_items": 0 }),
    ] {
        assert!(matches!(eval(&mut a, bad.clone()), Err(ToolError::InvalidArgs(_))), "{bad}");
    }
}

#[test]
fn files_are_read_inside_the_root_only_and_only_xml() {
    let inside = tempfile::tempdir().expect("dir");
    let outside = tempfile::tempdir().expect("dir");
    std::fs::write(inside.path().join("d.xml"), "<a><b/></a>").expect("writes");
    std::fs::write(inside.path().join("d.json"), "{}").expect("writes");
    std::fs::write(outside.path().join("s.xml"), "<s/>").expect("writes");
    let mut a = Automation::with_root(inside.path()).expect("root");
    assert_eq!(eval(&mut a, json!({ "path": "d.xml", "expression": "count(//*)" })).expect("runs")["items"][0]["text"], "2");
    let e = eval(&mut a, json!({ "path": outside.path().join("s.xml").to_str().expect("utf-8"), "expression": "/" })).expect_err("outside");
    assert!(e.to_string().contains("outside the root"), "{e}");
    let e = eval(&mut a, json!({ "path": "d.json", "expression": "/" })).expect_err("not xml");
    assert!(e.to_string().contains("not XML"), "{e}");
}

#[test]
fn nodes_that_cannot_be_placed_say_so_and_the_listing_has_a_total_budget() {
    let mut a = Automation::new();
    let deep = format!("{}{}", (1..=300).map(|d| format!("<e d=\"{d}\">")).collect::<String>(), "</e>".repeat(300));
    let r = eval(&mut a, json!({ "text": deep, "expression": "//e[@d='290']" })).expect("runs");
    assert_eq!((r["items"][0]["unmapped"].as_bool(), r["items"][0]["line"].as_u64()), (Some(true), None), "{r}");
    let shallow = eval(&mut a, json!({ "text": deep, "expression": "//e[@d='2']" })).expect("runs");
    assert!(shallow["items"][0].get("unmapped").is_none() && shallow["items"][0]["line"].as_u64() == Some(1));
    // Thousands of long items are listed only up to the budget.
    let big = format!("<r>{}</r>", "<i>0123456789</i>".repeat(500_000));
    let r = eval(&mut a, json!({ "text": big, "expression": "/r/i", "max_items": 1000 })).expect("runs");
    assert_eq!((r["count"].as_u64(), r["items"].as_array().map(Vec::len)), (Some(500_000), Some(1000)), "a thousand short items fit");
    let long = format!("<r>{}</r>", "<i>0123456789</i>".repeat(1000));
    let r = eval(&mut a, json!({ "text": long, "expression": "for $i in 1 to 1000 return /r" , "max_items": 1000 })).expect("runs");
    assert!(
        r["truncated"].as_bool() == Some(true) && r["items"].as_array().map_or(0, Vec::len) < 1000,
        "{}",
        r["items"].as_array().map_or(0, Vec::len)
    );
}

#[test]
fn a_trace_says_how_many_items_each_step_of_a_path_leaves() {
    use serde_json::json;
    let mut a = Automation::new();
    let doc = "<shop><order id=\"1\"><item/><item/></order><order id=\"5\"/></shop>";
    let r = eval(&mut a, json!({ "text": doc, "expression": "/shop/order[@id < 5]/item", "trace": true })).expect("runs");
    assert_eq!(r["count"].as_u64(), Some(2));
    let steps: Vec<(String, u64)> = r["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .map(|s| (s["step"].as_str().unwrap_or_default().to_owned(), s["count"].as_u64().unwrap_or(99)))
        .collect();
    assert_eq!(steps, [("/shop".to_owned(), 1), ("/order[@id < 5]".to_owned(), 1), ("/item".to_owned(), 2)]);
    assert_eq!(r["is_path"], json!(true));
    let plain = eval(&mut a, json!({ "text": doc, "expression": "count(//item)" })).expect("runs");
    assert!(plain.get("steps").is_none(), "no trace unless asked for");
    assert!(matches!(
        eval(&mut a, json!({ "text": doc, "expression": "/shop", "trace": "yes" })),
        Err(xmlcraft_automation::ToolError::InvalidArgs(_))
    ));
}
