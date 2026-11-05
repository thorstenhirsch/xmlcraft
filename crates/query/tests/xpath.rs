//! XPath evaluation: results, their places in the text, errors, limits.

use proptest::prelude::*;
use xmlcraft_query::{Options, Phase, evaluate};

const DOC: &str = "<?xml version=\"1.0\"?>\n<r xmlns:p=\"urn:p\"><i n=\"1\">a</i><!--c--><i n=\"2\">b<![CDATA[&]]>c</i><p:j/><?pi data?></r>";

fn run(expression: &str) -> xmlcraft_query::Evaluated {
    evaluate(DOC, expression, &Options::default()).unwrap_or_else(|e| panic!("{expression}: {e}"))
}

#[test]
fn nodes_say_what_they_are_where_they_are_and_their_path() {
    let r = run("/r/i");
    assert_eq!(r.count, 2);
    let first = &r.items[0];
    assert_eq!((first.kind.as_str(), first.name.as_str(), first.path.as_deref()), ("element", "i", Some("/r[1]/i[1]")));
    assert_eq!(&DOC[first.span.clone().expect("a place")], "<i n=\"1\">a</i>");
    assert_eq!(first.text, "<i n=\"1\">a</i>");
    let second = &r.items[1];
    assert_eq!(second.path.as_deref(), Some("/r[1]/i[2]"));
    assert_eq!(&DOC[second.span.clone().expect("a place")], "<i n=\"2\">b<![CDATA[&]]>c</i>");

    let attribute = &run("/r/i[2]/@n").items[0];
    assert_eq!(
        (attribute.kind.as_str(), attribute.name.as_str(), attribute.path.as_deref(), attribute.text.as_str()),
        ("attribute", "n", Some("/r[1]/i[2]/@n"), "n=\"2\"")
    );
    assert_eq!(&DOC[attribute.span.clone().expect("a place")], "n=\"2\"");

    // Text, with its CDATA and references as one node, a comment, a processing instruction, a namespaced element.
    let text = &run("/r/i[2]/text()").items[0];
    assert_eq!((text.kind.as_str(), text.path.as_deref(), text.text.as_str()), ("text", Some("/r[1]/i[2]/text()[1]"), "b&c"));
    assert_eq!(&DOC[text.span.clone().expect("a place")], "b<![CDATA[&]]>c");
    let comment = &run("//comment()").items[0];
    assert_eq!((comment.kind.as_str(), &DOC[comment.span.clone().expect("a place")]), ("comment", "<!--c-->"));
    let pi = &run("//processing-instruction()").items[0];
    assert_eq!((pi.kind.as_str(), pi.name.as_str(), &DOC[pi.span.clone().expect("a place")]), ("processing-instruction", "pi", "<?pi data?>"));
    let namespaced =
        &evaluate(DOC, "/r/p:j", &Options { namespaces: vec![("p".into(), "urn:p".into())], ..Options::default() }).expect("runs").items[0];
    assert_eq!((namespaced.name.as_str(), namespaced.path.as_deref()), ("Q{urn:p}j", Some("/r[1]/Q{urn:p}j[1]")));
    assert_eq!(&DOC[namespaced.span.clone().expect("a place")], "<p:j/>");
    let document = &run("/").items[0];
    assert_eq!((document.kind.as_str(), document.path.as_deref()), ("document", Some("/")));
}

#[test]
fn values_have_types_and_xpath_representations() {
    let types: Vec<(String, String)> = run("(1, 'x', 1.5, 2e0, true(), xs:date('2020-01-02'))").items.into_iter().map(|i| (i.kind, i.text)).collect();
    assert_eq!(
        types,
        [
            ("xs:integer", "1"),
            ("xs:string", "\"x\""),
            ("xs:decimal", "1.5"),
            ("xs:double", "2"),
            ("xs:boolean", "true()"),
            ("xs:date", "xs:date(\"2020-01-02\")")
        ]
        .map(|(a, b)| (a.to_owned(), b.to_owned()))
    );
    assert_eq!(run("map{'a': 1}").items[0].kind, "map");
    assert_eq!(run("[1, 2]").items[0].kind, "array");
    assert_eq!(run("count(//i)").items[0].text, "2");
    assert_eq!(run("//i/@n/string()").items.iter().map(|i| i.text.clone()).collect::<Vec<_>>(), ["\"1\"", "\"2\""]);
    assert_eq!(run("()").count, 0);
    assert_eq!(run("for $x in 1 to 3 return $x * 2").items.len(), 3);
}

#[test]
fn errors_have_codes_phases_and_places() {
    let e = evaluate(DOC, "//i[", &Options::default()).expect_err("syntax");
    assert_eq!((e.phase, e.code.as_str()), (Phase::Compile, "XPST0003"));
    assert!(e.span.is_some());
    let e = evaluate(DOC, "fn:nope()", &Options::default()).expect_err("unknown function");
    assert_eq!((e.phase, e.code.as_str()), (Phase::Compile, "XPST0017"));
    let e = evaluate(DOC, "1 div 0", &Options::default()).expect_err("run");
    assert_eq!((e.phase, e.code.as_str(), e.span.clone()), (Phase::Run, "FOAR0001", Some(0..7)));
    let e = evaluate("<a>", "/", &Options::default()).expect_err("document");
    assert_eq!(e.phase, Phase::Document);
    let e = evaluate(DOC, "/r/q:j", &Options::default()).expect_err("unbound prefix");
    assert_eq!(e.code, "XPST0081");
}

#[test]
fn big_results_are_cut_and_counted() {
    let o = Options { max_items: 5, max_text: 3, ..Options::default() };
    let r = evaluate(DOC, "1 to 100", &o).expect("runs");
    assert_eq!((r.items.len(), r.count), (5, 100));
    let long = evaluate(DOC, "string-join(for $i in 1 to 50 return 'ab', '')", &o).expect("runs");
    assert!(long.items[0].cut && long.items[0].text.chars().count() == 3);
    assert!(!run("'ab'").items[0].cut);
}

#[test]
fn a_document_with_a_doctype_still_has_places() {
    let doc = "<!DOCTYPE r [<!ELEMENT r ANY>]>\n<r><a>&lt;1</a></r>";
    let r = evaluate(doc, "/r/a", &Options::default()).expect("runs");
    assert_eq!(&doc[r.items[0].span.clone().expect("a place")], "<a>&lt;1</a>");
    assert_eq!(evaluate(doc, "string(/r/a)", &Options::default()).expect("runs").items[0].text, "\"<1\"");
}

proptest! {
    #[test]
    fn any_expression_on_any_text_is_an_answer_or_an_error(xml in ".{0,60}", expression in ".{0,40}") {
        let _ = evaluate(&xml, &expression, &Options::default());
    }

    #[test]
    fn every_node_of_a_document_is_at_the_place_its_text_says(depth in 1usize..4, width in 1usize..4) {
        // <e><e>text<!--c--></e>…</e>: all nodes found by `//node()` have a place, and an element's place starts with its tag.
        fn build(depth: usize, width: usize) -> String {
            if depth == 0 { return "t&amp;<!--c-->".to_owned(); }
            format!("<e a=\"1\">{}</e>", (0..width).map(|_| build(depth - 1, width)).collect::<String>())
        }
        let doc = build(depth, width);
        let r = evaluate(&doc, "//node() | //@*", &Options::default()).map_err(|e| TestCaseError::fail(e.to_string()))?;
        for item in r.items {
            let span = item.span.clone().ok_or_else(|| TestCaseError::fail(format!("no place for {item:?}")))?;
            let at = &doc[span];
            match item.kind.as_str() {
                "element" => prop_assert!(at.starts_with("<e ") && at.ends_with("</e>")),
                "attribute" => prop_assert_eq!(at, "a=\"1\""),
                "comment" => prop_assert_eq!(at, "<!--c-->"),
                "text" => prop_assert!(at.starts_with("t&amp;")),
                other => prop_assert!(false, "{other}"),
            }
        }
    }
}

#[test]
fn attributes_are_found_by_their_number_and_cdata_at_the_end_of_a_text_is_part_of_it() {
    let doc = "<e a=\"1\" b=\"2\" c=\"3\">x<![CDATA[y]]></e>";
    let r = evaluate(doc, "/e/@*", &Options::default()).expect("runs");
    let places: Vec<&str> = r.items.iter().map(|i| &doc[i.span.clone().expect("a place")]).collect();
    assert_eq!(places, ["a=\"1\"", "b=\"2\"", "c=\"3\""]);
    let c = evaluate(doc, "/e/@c", &Options::default()).expect("runs");
    assert_eq!(&doc[c.items[0].span.clone().expect("a place")], "c=\"3\"");
    let text = evaluate(doc, "/e/text()", &Options::default()).expect("runs").items.remove(0);
    assert_eq!((&doc[text.span.expect("a place")], text.text.as_str()), ("x<![CDATA[y]]>", "xy"));
    let only = "<e><![CDATA[z]]></e>";
    let text = evaluate(only, "/e/text()", &Options::default()).expect("runs").items.remove(0);
    assert_eq!(&only[text.span.expect("a place")], "<![CDATA[z]]>");
}
