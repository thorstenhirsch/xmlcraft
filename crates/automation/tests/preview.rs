//! `html_preview`, through the tool table and the root confinement: the page the Preview tool window shows, and the policy it is
//! served with.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, args: Value) -> Result<Value, ToolError> {
    a.call("html_preview", &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

const XSL: &str = r#"<xsl:stylesheet version="3.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:output method="html"/><xsl:template match="/"><html><head><title>Items</title></head><body><xsl:for-each select="//item"><p><xsl:value-of select="."/></p></xsl:for-each></body></html></xsl:template></xsl:stylesheet>"#;

#[test]
fn an_html_document_is_returned_as_it_is_with_the_policy() {
    let mut a = Automation::new();
    let page = "<!DOCTYPE html><title>Hi</title><p onclick=\"x()\">a<script>evil()</script>";
    let r = call(&mut a, json!({ "text": page, "language": "html" })).expect("runs");
    assert_eq!((r["shown"].as_str(), r["html"].as_str(), r["title"].as_str()), (Some("html"), Some(page), Some("Hi")), "{r}");
    assert_eq!(r["media_type"], "text/html; charset=utf-8");
    let csp = r["policy"]["content_security_policy"].as_str().expect("csp");
    assert_eq!(csp, "default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; form-action 'none'; base-uri 'none'; sandbox");
    assert_eq!(r["policy"]["headers"]["X-Content-Type-Options"], "nosniff");
    assert_eq!(r["policy"]["document_url"], "xmlcraft-preview://localhost/document");
    let opted = call(&mut a, json!({ "text": page, "language": "html", "allow_scripts": true, "allow_remote_images": true })).expect("runs");
    let csp = opted["policy"]["content_security_policy"].as_str().expect("csp");
    assert!(csp.contains("img-src data: https:") && csp.contains("script-src 'unsafe-inline'; sandbox allow-scripts"), "{csp}");
}

#[test]
fn an_xml_document_is_shown_through_the_stylesheet_it_names_from_inside_the_root() {
    let outer = tempfile::tempdir().expect("dir");
    let root = outer.path().join("root");
    std::fs::create_dir(&root).expect("creates");
    std::fs::write(root.join("s.xsl"), XSL).expect("writes");
    std::fs::write(root.join("d.xml"), "<?xml-stylesheet type=\"text/xsl\" href=\"s.xsl\"?>\n<d><item>a</item><item/></d>").expect("writes");
    std::fs::write(root.join("e.xml"), "<?xml-stylesheet type=\"text/xsl\" href=\"../out.xsl\"?>\n<d/>").expect("writes");
    std::fs::write(outer.path().join("out.xsl"), XSL).expect("writes");
    let mut a = Automation::with_root(&root).expect("root");
    let r = call(&mut a, json!({ "path": "d.xml" })).expect("runs");
    assert_eq!((r["shown"].as_str(), r["title"].as_str()), (Some("xslt"), Some("Items")), "{r}");
    let html = r["html"].as_str().expect("html");
    assert!(html.starts_with("<!DOCTYPE html>") && html.contains("<p>a</p><p></p>"), "{html}");
    // A stylesheet outside the root is not read: the document is a tree, and the answer says why.
    let e = call(&mut a, json!({ "path": "e.xml" })).expect("runs");
    assert_eq!(e["shown"], "tree", "{e}");
    assert!(e["warnings"][0].as_str().is_some_and(|w| w.contains("could not be read") && w.contains("outside the root")), "{e}");
    // A stylesheet given as text wins.
    let given = call(&mut a, json!({ "text": "<d><item>z</item></d>", "stylesheet": { "text": XSL } })).expect("runs");
    assert!(given["html"].as_str().is_some_and(|h| h.contains("<p>z</p>")), "{given}");
}

#[test]
fn plain_xml_is_a_tree_and_svg_an_image() {
    let mut a = Automation::new();
    let r = call(&mut a, json!({ "text": "<r a=\"&lt;\"><x>t</x></r>" })).expect("runs");
    assert_eq!((r["shown"].as_str(), r["title"].as_str()), (Some("tree"), Some("r")), "{r}");
    assert!(r["html"].as_str().is_some_and(|h| h.contains("&quot;&lt;&quot;") || h.contains("\"&lt;\"")), "{r}");
    let svg = call(&mut a, json!({ "text": "<svg xmlns=\"http://www.w3.org/2000/svg\"/>" })).expect("runs");
    assert_eq!((svg["shown"].as_str(), svg["media_type"].as_str()), (Some("svg"), Some("image/svg+xml; charset=utf-8")));
}

#[test]
fn what_cannot_be_previewed_is_an_error_that_says_so() {
    let mut a = Automation::new();
    let e = call(&mut a, json!({ "text": "{}", "language": "json" })).expect_err("JSON");
    assert!(e.to_string().contains("HTML and XML"), "{e}");
    let big = "x".repeat(8 * 1024 * 1024 + 1);
    let e = call(&mut a, json!({ "text": big, "language": "html" })).expect_err("too large");
    assert!(e.to_string().contains("MB"), "{e}");
    assert!(matches!(call(&mut a, json!({ "text": "<a/>", "stylesheet": "x" })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(call(&mut a, json!({ "text": "<a/>", "allow_scripts": "yes" })), Err(ToolError::InvalidArgs(_))));
    assert!(matches!(call(&mut a, json!({})), Err(ToolError::InvalidArgs(_))));
}
