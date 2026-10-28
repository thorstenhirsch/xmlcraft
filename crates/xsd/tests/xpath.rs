//! The expressions of identity constraints: what parses, and which nodes a path reaches.

use xmlcraft_xsd::model::QName;
use xmlcraft_xsd::xpath::{NameTest, parse_field, parse_selector};

fn ns() -> Vec<(Option<String>, String)> {
    vec![(Some("p".to_owned()), "urn:p".to_owned()), (None, "urn:default".to_owned())]
}

fn name(local: &str) -> QName {
    QName::new(None, local)
}

fn pname(local: &str) -> QName {
    QName::new(Some("urn:p"), local)
}

fn selects(selector: &str, chain: &[QName]) -> bool {
    let path = parse_selector(selector, &ns()).unwrap_or_else(|e| panic!("{selector}: {e}"));
    let refs: Vec<&QName> = chain.iter().collect();
    path.alternatives.iter().any(|a| a.matches_elements(&refs))
}

#[test]
fn selectors_follow_child_steps_from_the_context() {
    assert!(selects("a", &[name("a")]));
    assert!(!selects("a", &[name("b")]));
    assert!(!selects("a", &[name("a"), name("a")]), "a child, not a descendant");
    assert!(selects("a/b", &[name("a"), name("b")]));
    assert!(!selects("a/b", &[name("b")]));
    assert!(selects("./a/./b", &[name("a"), name("b")]), "a dot is the node itself");
    assert!(selects(".", &[]), "the context itself");
    assert!(!selects(".", &[name("a")]));
    assert!(selects("*", &[name("anything")]) && selects("a/*", &[name("a"), pname("x")]));
    assert!(selects("child::a/child::b", &[name("a"), name("b")]));
}

#[test]
fn a_leading_dot_slash_slash_reaches_any_depth() {
    assert!(selects(".//a", &[name("a")]));
    assert!(selects(".//a", &[name("x"), name("y"), name("a")]));
    assert!(!selects(".//a", &[name("a"), name("y")]));
    assert!(selects(".//a/b", &[name("q"), name("a"), name("b")]));
    assert!(!selects(".//a/b", &[name("q"), name("b")]));
    assert!(selects(".//.", &[]) && selects(".//.", &[name("a"), name("b")]), "descendant-or-self");
}

#[test]
fn alternatives_and_namespaces() {
    assert!(selects("a | b", &[name("b")]));
    assert!(selects("p:a", &[pname("a")]));
    assert!(!selects("p:a", &[name("a")]), "the prefix puts it in a namespace");
    assert!(!selects("a", &[pname("a")]), "no prefix is no namespace, whatever the default is");
    assert!(selects("p:*", &[pname("whatever")]) && !selects("p:*", &[name("whatever")]));
}

#[test]
fn fields_may_end_in_an_attribute() {
    let field = parse_field("a/@id | @key | .//@p:x", &ns()).expect("a field");
    assert_eq!(field.alternatives.len(), 3);
    assert_eq!(field.alternatives[0].steps, [NameTest::Name(name("a"))]);
    assert_eq!(field.alternatives[0].attribute, Some(NameTest::Name(name("id"))));
    assert!(field.alternatives[1].steps.is_empty() && field.alternatives[1].attribute.is_some());
    assert!(field.alternatives[2].descendant);
    assert_eq!(field.alternatives[2].attribute, Some(NameTest::Name(pname("x"))));
    assert_eq!(parse_field("attribute::id", &ns()).expect("a field").alternatives[0].attribute, Some(NameTest::Name(name("id"))));
    assert_eq!(parse_field("@*", &ns()).expect("a field").alternatives[0].attribute, Some(NameTest::Any));
}

#[test]
fn what_is_not_in_the_subset_is_refused() {
    for bad in ["", "a/", "/a", "a//b", "a b", "a[1]", "@id", "a/@id/b", "q:a", "a|", "|a", "..", "a/..", "1a", "a:b:c", "text()", "a@b"] {
        assert!(parse_selector(bad, &ns()).is_err(), "selector '{bad}' should not parse");
    }
    for bad in ["", "a/@", "a//b", "@a/@b", "q:a", "a[@x]", "@@a", "a/", "@a|"] {
        assert!(parse_field(bad, &ns()).is_err(), "field '{bad}' should not parse");
    }
    let message = parse_selector("q:a", &ns()).expect_err("a prefix that is not declared");
    assert!(message.contains("'q'"), "{message}");
}
