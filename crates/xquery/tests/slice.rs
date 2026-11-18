//! The first slice: plain XPath, a FLWOR, a direct constructor, errors with codes and places.

use xmlcraft_xquery::{Options, Serialization, evaluate};

fn run(query: &str, context: Option<&str>) -> Result<String, String> {
    let options = Options { serialize: Some(Serialization::Xml), ..Options::default() };
    evaluate(query, context, &options).map(|o| o.serialized.unwrap_or_default()).map_err(|e| format!("{}: {}", e.code, e.message))
}

#[test]
fn plain_xpath_runs() {
    assert_eq!(run("1 + 2", None).unwrap(), "3");
    assert_eq!(run("count(//i)", Some("<r><i/><i/></r>")).unwrap(), "2");
    assert_eq!(run("string-join(('a', 'b'), '-')", None).unwrap(), "a-b");
}

#[test]
fn a_flwor_runs() {
    let q = "for $x in (1, 2, 3, 4) let $y := $x * 10 where $x mod 2 = 0 return $y";
    assert_eq!(run(q, None).unwrap(), "20 40");
}

#[test]
fn a_direct_element_constructor_builds_a_node() {
    assert_eq!(run("<a n='{1 + 1}'>{ 'x', 'y' }<b/></a>", None).unwrap(), r#"<a n="2">x y<b/></a>"#);
}

#[test]
fn syntax_errors_have_a_code_and_a_place() {
    let e = evaluate("1 +", None, &Options::default()).unwrap_err();
    assert_eq!(e.code, "XPST0003");
    assert_eq!(e.span, Some(3..3));
}

#[test]
fn type_errors_have_their_code() {
    let e = evaluate("'a' + 1", None, &Options::default()).unwrap_err();
    assert_eq!(e.code, "XPTY0004");
}
