//! `xml_complete`, through the tool table.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    a.call("xml_complete", &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

fn labels(r: &Value) -> Vec<&str> {
    r["items"].as_array().expect("items").iter().filter_map(|i| i["label"].as_str()).collect()
}

#[test]
fn elements_end_tags_attributes_values_and_entities_come_from_the_document() {
    let mut a = Automation::new();
    let text = "<lib><book id=\"1\" kind=\"p\">&e;</book><book id=\"2\" kind=\"q\"/><book /></lib>";
    let offset = text.find("<book />").expect("there") + 6;
    let r = call(&mut a, json!({ "text": text, "offset": offset, "explicit": true })).expect("runs");
    assert_eq!((r["kind"].as_str(), labels(&r)), (Some("attribute"), vec!["id", "kind"]));
    assert_eq!(r["items"][0]["insert"].as_str(), Some("id=\"\""));

    let offset = text.find("kind=\"q").expect("there") + 6;
    let r = call(&mut a, json!({ "text": text, "offset": offset })).expect("runs");
    assert_eq!((r["kind"].as_str(), labels(&r)), (Some("value"), vec!["p", "q"]));

    let offset = text.find("</book>").expect("there") + 2;
    let r = call(&mut a, json!({ "text": text, "offset": offset })).expect("runs");
    assert_eq!((r["kind"].as_str(), labels(&r)), (Some("end-tag"), vec!["book", "lib"]));

    let offset = text.find("<book />").expect("there") + 1;
    let r = call(&mut a, json!({ "text": text, "offset": offset })).expect("runs");
    assert_eq!((r["kind"].as_str(), labels(&r)), (Some("element"), vec!["book", "lib"]));

    let offset = text.find("&e;").expect("there") + 1;
    let r = call(&mut a, json!({ "text": text, "offset": offset })).expect("runs");
    assert_eq!((r["kind"].as_str(), labels(&r)), (Some("entity"), vec!["&e;"]));
}

#[test]
fn an_explicit_request_opens_the_popup_where_nothing_was_typed() {
    let mut a = Automation::new();
    let text = "<lib><book id=\"1\"/><book /></lib>";
    let offset = text.find("<book />").expect("there") + 6;
    let r = call(&mut a, json!({ "text": text, "offset": offset, "explicit": true })).expect("runs");
    assert_eq!((r["kind"].as_str(), labels(&r)), (Some("attribute"), vec!["id"]));
    assert_eq!(r["items"][0]["cursor"].as_u64(), Some(4), "the caret goes between the quotes");

    let offset = text.find("</lib>").expect("there");
    let r = call(&mut a, json!({ "text": text, "offset": offset, "explicit": true })).expect("runs");
    assert_eq!((r["kind"].as_str(), labels(&r)), (Some("element"), vec!["book", "lib"]));
}

#[test]
fn nothing_is_offered_in_text_and_a_request_for_other_text_is_refused() {
    let mut a = Automation::new();
    let r = call(&mut a, json!({ "text": "plain text", "offset": 5 })).expect("runs");
    assert_eq!((r["kind"].as_str(), r["count"].as_u64()), (Some(""), Some(0)));
    assert!(call(&mut a, json!({ "text": "{}", "language": "json", "offset": 1 })).is_err(), "not XML");
    assert!(call(&mut a, json!({ "text": "<a/>", "offset": 99 })).is_err(), "outside the text");
}
