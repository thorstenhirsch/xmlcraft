//! Identity constraints in documents: `unique`, `key` and `keyref`.

use xmlcraft_syntax::parse;
use xmlcraft_validate::validate_against_schema;
use xmlcraft_xsd::{NoLoader, XSD_NAMESPACE, assemble, load};

fn problems(body: &str, xml: &str) -> Vec<(&'static str, String)> {
    let text = format!("<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\" xmlns:p=\"urn:p\">{body}</xs:schema>");
    let documents = load(&text, "/s/root.xsd", &mut NoLoader);
    let assembly = assemble(&documents);
    assert!(assembly.diagnostics.is_empty(), "{:?}", assembly.diagnostics.iter().map(|d| d.diagnostic.message()).collect::<Vec<_>>());
    let parsed = parse(xml);
    assert!(parsed.diagnostics().is_empty(), "{:?}", parsed.diagnostics());
    validate_against_schema(&parsed.syntax(), &assembly.schema).into_iter().map(|d| (d.code.key(), d.message())).collect()
}

fn keys(body: &str, xml: &str) -> Vec<&'static str> {
    problems(body, xml).into_iter().map(|(k, _)| k).collect()
}

const LIBRARY: &str = "<xs:element name=\"library\"><xs:complexType><xs:sequence>\
    <xs:element name=\"book\" maxOccurs=\"unbounded\"><xs:complexType><xs:sequence><xs:element name=\"title\" type=\"xs:string\" maxOccurs=\"2\"/></xs:sequence>\
      <xs:attribute name=\"isbn\" type=\"xs:string\"/></xs:complexType></xs:element>\
    <xs:element name=\"loan\" minOccurs=\"0\" maxOccurs=\"unbounded\"><xs:complexType><xs:attribute name=\"isbn\" type=\"xs:string\"/></xs:complexType></xs:element>\
  </xs:sequence></xs:complexType>\
  <xs:key name=\"bookKey\"><xs:selector xpath=\"book\"/><xs:field xpath=\"@isbn\"/></xs:key>\
  <xs:keyref name=\"loanRef\" refer=\"bookKey\"><xs:selector xpath=\"loan\"/><xs:field xpath=\"@isbn\"/></xs:keyref>\
  <xs:unique name=\"titleUnique\"><xs:selector xpath=\"book\"/><xs:field xpath=\"title\"/></xs:unique>\
</xs:element>";

#[test]
fn a_document_that_keeps_its_keys_is_valid() {
    let xml =
        "<library><book isbn=\"1\"><title>A</title></book><book isbn=\"2\"><title>B</title></book><loan isbn=\"2\"/><loan isbn=\"2\"/></library>";
    assert_eq!(keys(LIBRARY, xml), Vec::<&str>::new());
}

#[test]
fn a_key_value_cannot_come_twice() {
    let xml = "<library><book isbn=\"1\"><title>A</title></book><book isbn=\"1\"><title>B</title></book></library>";
    let found = problems(LIBRARY, xml);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "cvc.duplicate-key-value");
    assert!(found[0].1.contains("'bookKey'") && found[0].1.contains("(1)"), "{found:?}");
}

#[test]
fn a_keyref_needs_a_key_with_its_value() {
    let xml = "<library><book isbn=\"1\"><title>A</title></book><loan isbn=\"9\"/></library>";
    let found = problems(LIBRARY, xml);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "cvc.keyref-has-no-key");
    assert!(found[0].1.contains("'loanRef'") && found[0].1.contains("(9)"), "{found:?}");
    // A loan without the attribute is not checked.
    assert_eq!(keys(LIBRARY, "<library><book isbn=\"1\"><title>A</title></book><loan/></library>"), Vec::<&str>::new());
}

#[test]
fn unique_skips_a_row_with_a_missing_field_and_key_does_not() {
    let two_titles = "<library><book isbn=\"1\"/><book isbn=\"2\"/></library>";
    assert_eq!(
        keys(LIBRARY, two_titles),
        ["cvc.content-incomplete", "cvc.content-incomplete"],
        "title is required; the unique constraint has nothing to say"
    );
    let no_isbn = "<library><book><title>A</title></book></library>";
    let found = problems(LIBRARY, no_isbn);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "cvc.key-field-missing");
    assert!(found[0].1.contains("field 1"), "{found:?}");
}

#[test]
fn unique_values_are_those_of_an_element_too() {
    let xml = "<library><book isbn=\"1\"><title>A</title></book><book isbn=\"2\"><title>A</title></book></library>";
    let found = problems(LIBRARY, xml);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "cvc.duplicate-key-value");
    assert!(found[0].1.contains("'titleUnique'"), "{found:?}");
}

#[test]
fn a_field_selects_one_node_at_most() {
    let xml = "<library><book isbn=\"1\"><title>A</title><title>B</title></book></library>";
    let found = problems(LIBRARY, xml);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "cvc.field-selects-many");
}

#[test]
fn values_are_compared_in_the_value_space_of_their_types() {
    let schema = "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"e\" maxOccurs=\"unbounded\"><xs:complexType>\
        <xs:attribute name=\"n\" type=\"xs:decimal\"/><xs:attribute name=\"s\" type=\"xs:string\"/></xs:complexType></xs:element></xs:sequence></xs:complexType>\
        <xs:unique name=\"byNumber\"><xs:selector xpath=\"e\"/><xs:field xpath=\"@n\"/></xs:unique>\
        <xs:unique name=\"byString\"><xs:selector xpath=\"e\"/><xs:field xpath=\"@s\"/></xs:unique></xs:element>";
    assert_eq!(keys(schema, "<r><e n=\"1\"/><e n=\"1.0\"/></r>"), ["cvc.duplicate-key-value"], "1 and 1.0 are one number");
    assert_eq!(keys(schema, "<r><e s=\"1\"/><e s=\"1.0\"/></r>"), Vec::<&str>::new(), "and two strings");
    assert_eq!(keys(schema, "<r><e s=\"a\"/><e s=\"a\"/></r>"), ["cvc.duplicate-key-value"]);
}

#[test]
fn a_constraint_can_have_several_fields() {
    let schema = "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"e\" maxOccurs=\"unbounded\"><xs:complexType>\
        <xs:attribute name=\"a\"/><xs:attribute name=\"b\"/></xs:complexType></xs:element></xs:sequence></xs:complexType>\
        <xs:key name=\"pair\"><xs:selector xpath=\"e\"/><xs:field xpath=\"@a\"/><xs:field xpath=\"@b\"/></xs:key></xs:element>";
    assert_eq!(keys(schema, "<r><e a=\"1\" b=\"1\"/><e a=\"1\" b=\"2\"/><e a=\"2\" b=\"1\"/></r>"), Vec::<&str>::new());
    let found = problems(schema, "<r><e a=\"1\" b=\"2\"/><e a=\"1\" b=\"2\"/></r>");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].1.contains("(1, 2)"), "{found:?}");
    let found = problems(schema, "<r><e a=\"1\"/></r>");
    assert!(found[0].1.contains("field 2"), "{found:?}");
}

#[test]
fn a_constraint_holds_within_one_instance_of_its_element() {
    let schema = "<xs:element name=\"doc\"><xs:complexType><xs:sequence>\
        <xs:element name=\"section\" maxOccurs=\"unbounded\"><xs:complexType><xs:sequence><xs:element name=\"item\" maxOccurs=\"unbounded\"><xs:complexType><xs:attribute name=\"id\"/></xs:complexType></xs:element></xs:sequence>\
        </xs:complexType><xs:unique name=\"itemId\"><xs:selector xpath=\"item\"/><xs:field xpath=\"@id\"/></xs:unique></xs:element>\
        </xs:sequence></xs:complexType></xs:element>";
    assert_eq!(keys(schema, "<doc><section><item id=\"a\"/><item id=\"b\"/></section><section><item id=\"a\"/></section></doc>"), Vec::<&str>::new());
    assert_eq!(keys(schema, "<doc><section><item id=\"a\"/><item id=\"a\"/></section></doc>"), ["cvc.duplicate-key-value"]);
}

#[test]
fn a_keyref_finds_the_keys_made_below_its_element() {
    let schema = "<xs:element name=\"r\"><xs:complexType><xs:sequence>\
        <xs:element name=\"group\" maxOccurs=\"unbounded\"><xs:complexType><xs:sequence><xs:element name=\"k\" maxOccurs=\"unbounded\"><xs:complexType><xs:attribute name=\"id\"/></xs:complexType></xs:element></xs:sequence></xs:complexType>\
          <xs:key name=\"gk\"><xs:selector xpath=\"k\"/><xs:field xpath=\"@id\"/></xs:key></xs:element>\
        <xs:element name=\"use\" maxOccurs=\"unbounded\"><xs:complexType><xs:attribute name=\"ref\"/></xs:complexType></xs:element>\
        </xs:sequence></xs:complexType>\
        <xs:keyref name=\"r\" refer=\"gk\"><xs:selector xpath=\"use\"/><xs:field xpath=\"@ref\"/></xs:keyref></xs:element>";
    assert_eq!(keys(schema, "<r><group><k id=\"a\"/></group><group><k id=\"b\"/></group><use ref=\"b\"/><use ref=\"a\"/></r>"), Vec::<&str>::new());
    assert_eq!(keys(schema, "<r><group><k id=\"a\"/></group><use ref=\"z\"/></r>"), ["cvc.keyref-has-no-key"]);
    // The same value in two groups is fine: each group has its own key table, and the keyref sees both.
    assert_eq!(keys(schema, "<r><group><k id=\"a\"/></group><group><k id=\"a\"/></group><use ref=\"a\"/></r>"), Vec::<&str>::new());
}

#[test]
fn selectors_reach_below_and_alternatives_join() {
    let schema = "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:any processContents=\"lax\" maxOccurs=\"unbounded\"/></xs:sequence></xs:complexType>\
        <xs:unique name=\"deep\"><xs:selector xpath=\".//a | b\"/><xs:field xpath=\"@id\"/></xs:unique></xs:element>";
    assert_eq!(keys(schema, "<r><x><y><a id=\"1\"/></y></x><b id=\"2\"/></r>"), Vec::<&str>::new());
    assert_eq!(keys(schema, "<r><x><y><a id=\"1\"/></y></x><b id=\"1\"/></r>"), ["cvc.duplicate-key-value"]);
    assert_eq!(keys(schema, "<r><x><b id=\"1\"/></x><b id=\"1\"/></r>"), Vec::<&str>::new(), "b is a child of the context only");
}

#[test]
fn a_field_that_selects_an_element_with_a_complex_type_is_refused() {
    let schema = "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"e\" maxOccurs=\"unbounded\"><xs:complexType><xs:sequence><xs:element name=\"c\"><xs:complexType><xs:sequence><xs:element name=\"d\" minOccurs=\"0\"/></xs:sequence></xs:complexType></xs:element></xs:sequence></xs:complexType></xs:element></xs:sequence></xs:complexType>\
        <xs:unique name=\"u\"><xs:selector xpath=\"e\"/><xs:field xpath=\"c\"/></xs:unique></xs:element>";
    assert_eq!(keys(schema, "<r><e><c/></e></r>"), ["cvc.field-not-simple"]);
}

#[test]
fn a_nilled_field_has_no_value() {
    let schema = "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"e\" maxOccurs=\"unbounded\"><xs:complexType><xs:sequence><xs:element name=\"v\" type=\"xs:int\" nillable=\"true\"/></xs:sequence></xs:complexType></xs:element></xs:sequence></xs:complexType>\
        <xs:key name=\"k\"><xs:selector xpath=\"e\"/><xs:field xpath=\"v\"/></xs:key></xs:element>";
    let xsi = "xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"";
    assert_eq!(keys(schema, &format!("<r {xsi}><e><v>1</v></e><e><v>2</v></e></r>")), Vec::<&str>::new());
    assert_eq!(keys(schema, &format!("<r {xsi}><e><v xsi:nil=\"true\"/></e></r>")), ["cvc.key-field-missing"]);
}

#[test]
fn attributes_in_a_namespace_are_found_by_their_prefix() {
    let schema = "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"e\" maxOccurs=\"unbounded\"><xs:complexType><xs:anyAttribute processContents=\"skip\"/></xs:complexType></xs:element></xs:sequence></xs:complexType>\
        <xs:unique name=\"u\"><xs:selector xpath=\"e\"/><xs:field xpath=\"@p:id\"/></xs:unique></xs:element>";
    assert_eq!(keys(schema, "<r xmlns:q=\"urn:p\"><e q:id=\"1\"/><e q:id=\"1\"/></r>"), ["cvc.duplicate-key-value"], "q is p's namespace");
    assert_eq!(keys(schema, "<r xmlns:q=\"urn:other\"><e q:id=\"1\"/><e q:id=\"1\"/></r>"), Vec::<&str>::new());
    assert_eq!(keys(schema, "<r><e id=\"1\"/><e id=\"1\"/></r>"), Vec::<&str>::new(), "an attribute with no prefix is in no namespace");
}

#[test]
fn a_constraint_whose_expression_is_wrong_is_left_alone() {
    // The schema reports the expression; a document is still validated, without that constraint.
    let text = format!(
        "<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\"><xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"e\" maxOccurs=\"unbounded\"/></xs:sequence></xs:complexType>\
         <xs:unique name=\"u\"><xs:selector xpath=\"e[1]\"/><xs:field xpath=\"@id\"/></xs:unique></xs:element></xs:schema>"
    );
    let assembly = assemble(&load(&text, "/s/root.xsd", &mut NoLoader));
    assert_eq!(assembly.diagnostics.len(), 1);
    let parsed = parse("<r><e/><e/></r>");
    assert!(validate_against_schema(&parsed.syntax(), &assembly.schema).is_empty());
}

#[test]
fn a_string_and_a_number_with_the_same_digits_are_two_values() {
    let schema = "<xs:element name=\"r\"><xs:complexType><xs:choice maxOccurs=\"unbounded\">\
        <xs:element name=\"x\"><xs:complexType><xs:attribute name=\"n\" type=\"xs:decimal\"/></xs:complexType></xs:element>\
        <xs:element name=\"y\"><xs:complexType><xs:attribute name=\"s\" type=\"xs:string\"/></xs:complexType></xs:element></xs:choice></xs:complexType>\
        <xs:unique name=\"u\"><xs:selector xpath=\"x | y\"/><xs:field xpath=\"@n | @s\"/></xs:unique></xs:element>";
    assert_eq!(keys(schema, "<r><x n=\"1\"/><y s=\"1\"/></r>"), Vec::<&str>::new());
    assert_eq!(keys(schema, "<r><x n=\"1\"/><x n=\"1\"/></r>"), ["cvc.duplicate-key-value"]);
}
