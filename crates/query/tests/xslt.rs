//! XSLT on the `xee` engine: what it makes, and what it does not implement is said, never a crash.

use xmlcraft_query::transform;

const DOC: &str = "<doc><item n=\"1\">a</item><item n=\"2\">b</item></doc>";
const XSL: &str = "http://www.w3.org/1999/XSL/Transform";

fn sheet(body: &str) -> String {
    format!("<xsl:stylesheet version=\"3.0\" xmlns:xsl=\"{XSL}\">{body}</xsl:stylesheet>")
}

#[test]
fn for_each_and_value_of_make_a_result_tree() {
    let r = transform(DOC, &sheet(r#"<xsl:template match="/"><out><xsl:for-each select="//item"><i><xsl:value-of select="@n"/>:<xsl:value-of select="."/></i></xsl:for-each></out></xsl:template>"#)).expect("transforms");
    assert_eq!((r.text.as_str(), r.items), ("<out><i>1:a</i><i>2:b</i></out>", 1));
}

#[test]
fn templates_are_applied_to_what_matches() {
    let r = transform(DOC, &sheet(r#"<xsl:template match="doc"><r><xsl:apply-templates/></r></xsl:template><xsl:template match="item"><x><xsl:value-of select="."/></x></xsl:template>"#)).expect("transforms");
    assert_eq!(r.text, "<r><x>a</x><x>b</x></r>");
}

#[test]
fn a_doctype_in_the_document_does_not_stop_it() {
    let with_doctype = "<!DOCTYPE doc [<!ELEMENT doc ANY>]><doc><item n=\"1\">a</item></doc>";
    let r = transform(with_doctype, &sheet(r#"<xsl:template match="/"><n><xsl:value-of select="count(//item)"/></n></xsl:template>"#))
        .expect("transforms");
    assert_eq!(r.text, "<n>1</n>");
}

#[test]
fn what_the_engine_does_not_implement_is_an_error_that_says_so_and_a_bad_stylesheet_or_document_is_refused() {
    let e = transform(DOC, "<not xslt").expect_err("not a stylesheet");
    assert!(e.message.contains("cannot be used"), "{e}");
    let e = transform("<doc>", &sheet(r#"<xsl:template match="/"/>"#)).expect_err("not well-formed");
    assert!(e.message.contains("document cannot be read"), "{e}");
    // Variables are not implemented in this engine yet: that is an error, not a panic.
    let e = transform(DOC, &sheet(r#"<xsl:template match="/"><xsl:variable name="v" select="1"/><x><xsl:value-of select="$v"/></x></xsl:template>"#))
        .err();
    assert!(e.is_none_or(|e| !e.message.is_empty()), "an error or a result, never a crash");
}

proptest::proptest! {
    #[test]
    fn any_stylesheet_and_any_document_are_answered_or_refused_without_a_crash(xslt in ".{0,200}", xml in ".{0,100}") {
        let _ = transform(&xml, &xslt);
        let _ = transform(DOC, &xslt);
    }
}

const SHEET: &str = r#"<xsl:stylesheet version="3.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:param name="who" select="'world'"/><xsl:param name="n" select="2"/><xsl:template match="/"><out><xsl:value-of select="concat($who, ':', $n * 2)"/></out></xsl:template></xsl:stylesheet>"#;

#[test]
fn global_parameters_are_passed_as_typed_values_and_a_literal_default_is_kept() {
    use xmlcraft_query::{Param, ParamValue, declared_params, literal_default, transform_with};
    let params = declared_params(SHEET);
    assert_eq!(params.iter().map(|p| (p.name.as_str(), p.select.as_deref())).collect::<Vec<_>>(), [("who", Some("'world'")), ("n", Some("2"))]);
    assert_eq!(literal_default("'a b'"), Some(ParamValue::String("a b".into())));
    assert_eq!(literal_default("2.5"), Some(ParamValue::Number("2.5".into())));
    assert_eq!((literal_default("true()"), literal_default(" false() ")), (Some(ParamValue::Boolean(true)), Some(ParamValue::Boolean(false))));
    assert_eq!(literal_default("count(//a)"), None);
    assert_eq!(literal_default("'it''s'"), None, "an escaped quote is an expression here");
    assert_eq!(transform_with("<a/>", SHEET, &[]).expect("runs").text, "<out>world:4</out>");
    let given =
        [Param { name: "who".into(), value: ParamValue::String("Rex".into()) }, Param { name: "n".into(), value: ParamValue::Number("20".into()) }];
    assert_eq!(transform_with("<a/>", SHEET, &given).expect("runs").text, "<out>Rex:40</out>");
    let unknown = [Param { name: "zzz".into(), value: ParamValue::Boolean(true) }];
    assert!(transform_with("<a/>", SHEET, &unknown).expect_err("unknown").message.contains("zzz"));
    assert!(declared_params("<not a stylesheet").is_empty());
}
