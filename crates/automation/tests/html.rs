//! HTML documents through the tools that read any language: the problems, the outline and the folds are those of HTML.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content};

fn call(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args).unwrap_or_else(|e| panic!("{tool} failed: {e}")).into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    }
}

const PAGE: &str = "<!DOCTYPE html>\n<html>\n<body>\n<p>one<br>two\n<ul><li>a<li>b</ul>\n<div id=main><span>x</div>\n</body>\n</html>\n";

#[test]
fn html_is_read_by_the_rules_of_html_and_the_tools_say_what_is_wrong_and_what_is_there() {
    let mut a = Automation::new();
    let r = call(&mut a, "xml_wellformed", json!({ "text": PAGE, "language": "html" }));
    let problems = r["problems"].as_array().or_else(|| r["diagnostics"].as_array()).expect("a list of problems");
    let codes: Vec<&str> = problems.iter().filter_map(|p| p["code"].as_str()).collect();
    assert_eq!(codes, ["html.unclosed-before-end-tag"], "{r}");
    // The same text read as XML has errors of XML.
    let xml = call(&mut a, "xml_wellformed", json!({ "text": PAGE }));
    let xml_problems = xml["problems"].as_array().or_else(|| xml["diagnostics"].as_array()).expect("list");
    assert!(xml_problems.iter().any(|p| p["severity"] == "error"), "{xml}");
    let outline = call(&mut a, "xml_outline", json!({ "text": PAGE, "language": "html" }));
    let names: Vec<&str> =
        outline["items"].as_array().or_else(|| outline["outline"].as_array()).expect("items").iter().filter_map(|i| i["name"].as_str()).collect();
    assert_eq!(names, ["html", "body", "p", "br", "ul", "li", "li", "div", "span"], "{outline}");
    let folds = call(&mut a, "xml_folds", json!({ "text": PAGE, "language": "html" }));
    assert!(folds.to_string().contains("element"), "{folds}");
}

#[test]
fn a_file_called_html_is_html_and_the_language_may_be_given_for_text() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("p.html");
    std::fs::write(&path, "<ul><li>a<li>b</ul>").expect("writes");
    let mut a = Automation::new();
    let opened = call(&mut a, "doc_open", json!({ "path": path.to_str().expect("utf-8") }));
    let id = opened["id"].as_u64().expect("id");
    let r = call(&mut a, "xml_wellformed", json!({ "id": id }));
    // (Read as XML a list with unclosed items would have errors: the file is read as HTML by its name.)
    assert!(!r.to_string().contains("\"error\""), "no errors: {r}");
    let e = a.call("xml_wellformed", &json!({ "text": "<a/>", "language": "htmx" })).expect_err("unknown language").to_string();
    assert!(e.contains("html"), "{e}");
}
