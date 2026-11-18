//! `xquery_eval`: queries with and without a document, variables, documents, serialization and errors.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn run(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    a.call("xquery_eval", &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

#[test]
fn a_query_over_a_document_lists_its_nodes_with_places_and_built_nodes_serialized() {
    let mut a = Automation::new();
    let doc = "<r>\n  <i n=\"1\">a</i>\n  <i n=\"2\">b</i>\n</r>";
    let q = "for $i in //i where $i/@n > 1 return ($i, <found n='{$i/@n}'/>)";
    let r = run(&mut a, json!({ "text": doc, "query": q, "serialize": "xml" })).expect("runs");
    assert_eq!((r["count"].as_u64(), r["engine"].as_str()), (Some(2), Some("XQuery 3.1")), "{r}");
    let i = &r["items"][0];
    assert_eq!((i["path"].as_str(), i["line"].as_u64(), i["column"].as_u64()), (Some("/r[1]/i[2]"), Some(3), Some(3)));
    let made = &r["items"][1];
    assert_eq!((made["kind"].as_str(), made.get("path"), made["text"].as_str()), (Some("element"), None, Some("<found n=\"2\"/>")));
    assert_eq!(r["serialized"], "<i n=\"2\">b</i><found n=\"2\"/>");
}

#[test]
fn a_query_runs_without_a_document_with_variables_and_documents() {
    let mut a = Automation::new();
    let q = "declare variable $n external; declare variable $who external; (for $i in 1 to $n return $i * 2, $who, count(doc('urn:d')//x))";
    let r =
        run(&mut a, json!({ "query": q, "variables": { "n": 3, "who": "me" }, "documents": { "urn:d": "<d><x/><x/></d>" }, "serialize": "text" }))
            .expect("runs");
    assert_eq!(r["serialized"], "2 4 6 me 2", "{r}");
    let j = run(&mut a, json!({ "query": "map { 'a': [1, true()] }", "serialize": "json" })).expect("runs");
    assert_eq!(j["serialized"], "{\"a\":[1,true]}");
}

#[test]
fn errors_have_their_codes_phases_and_places() {
    let mut a = Automation::new();
    let e = run(&mut a, json!({ "query": "for $x in 1 to 3 retrun $x" })).expect_err("syntax");
    assert!(e.to_string().contains("compile error") && e.to_string().contains("XPST0003") && e.to_string().contains("of the query"), "{e}");
    let e = run(&mut a, json!({ "query": "<a>{attribute b {1}, attribute b {2}}</a>" })).expect_err("run");
    assert!(e.to_string().contains("run error") && e.to_string().contains("XQDY0025"), "{e}");
    let e = run(&mut a, json!({ "query": "count(1 to 100000000)", "max_steps": 10 })).expect_err("budget");
    assert!(e.to_string().contains("XMLC0001"), "{e}");
    let e = run(&mut a, json!({ "query": "1", "serialize": "html" })).expect_err("args");
    assert!(matches!(e, ToolError::InvalidArgs(_)), "{e}");
    let e = run(&mut a, json!({ "query": "1", "variables": { "x": [1] } })).expect_err("args");
    assert!(matches!(e, ToolError::InvalidArgs(_)), "{e}");
}
