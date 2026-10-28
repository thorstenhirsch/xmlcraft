//! Documents validated against a schema: what is valid, and which constraint each mistake breaks.

use xmlcraft_syntax::{Limits, parse, parse_with};
use xmlcraft_validate::validate_against_schema;
use xmlcraft_xsd::{Assembly, NoLoader, XSD_NAMESPACE, assemble, load};

/// A schema with no target namespace.
fn assembled(body: &str) -> Assembly {
    let text = format!("<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\">{body}</xs:schema>");
    assembled_text(&text)
}

fn assembled_text(text: &str) -> Assembly {
    let documents = load(text, "/s/root.xsd", &mut NoLoader);
    assert!(documents.diagnostics.is_empty(), "{:?}", documents.diagnostics.iter().map(|d| d.diagnostic.message()).collect::<Vec<_>>());
    let assembly = assemble(&documents);
    assert!(assembly.diagnostics.is_empty(), "{:?}", assembly.diagnostics.iter().map(|d| d.diagnostic.message()).collect::<Vec<_>>());
    assembly
}

fn problems_in(assembly: &Assembly, xml: &str) -> Vec<(&'static str, String)> {
    let parsed = parse(xml);
    assert!(parsed.diagnostics().is_empty(), "the test document is not well-formed: {:?}", parsed.diagnostics());
    validate_against_schema(&parsed.syntax(), &assembly.schema).into_iter().map(|d| (d.code.key(), d.message())).collect()
}

/// The keys of the problems, to compare with an array (`[]` for none).
#[derive(Debug)]
struct Keys(Vec<&'static str>);

impl<const N: usize> PartialEq<[&str; N]> for Keys {
    fn eq(&self, other: &[&str; N]) -> bool {
        self.0.as_slice() == other.as_slice()
    }
}

fn keys(body: &str, xml: &str) -> Keys {
    Keys(problems_in(&assembled(body), xml).into_iter().map(|(k, _)| k).collect())
}

fn problems(body: &str, xml: &str) -> Vec<(&'static str, String)> {
    problems_in(&assembled(body), xml)
}

const ORDER: &str = "<xs:element name=\"order\"><xs:complexType><xs:sequence>\
      <xs:element name=\"id\" type=\"xs:int\"/>\
      <xs:element name=\"item\" maxOccurs=\"unbounded\"><xs:complexType><xs:simpleContent><xs:extension base=\"xs:string\"><xs:attribute name=\"qty\" type=\"xs:positiveInteger\" use=\"required\"/></xs:extension></xs:simpleContent></xs:complexType></xs:element>\
      <xs:element name=\"note\" type=\"xs:string\" minOccurs=\"0\"/>\
    </xs:sequence><xs:attribute name=\"currency\" type=\"xs:string\" default=\"EUR\"/></xs:complexType></xs:element>";

#[test]
fn a_valid_document_has_no_problems() {
    assert_eq!(keys(ORDER, "<order currency=\"USD\"><id>7</id><item qty=\"2\">pen</item><item qty=\"1\">ink</item><note>hi</note></order>"), []);
    assert_eq!(keys(ORDER, "<order><id> 7 </id><item qty=\"2\"/></order>"), [], "an int takes white space around it");
    // Comments and processing instructions are not content.
    assert_eq!(keys(ORDER, "<order><!-- c --><id>7</id><?pi x?><item qty=\"2\">a<!--b-->c</item></order>"), []);
}

#[test]
fn the_root_element_has_to_be_declared() {
    let found = problems(ORDER, "<unknown><id>7</id></unknown>");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "cvc.element-not-declared");
    assert!(found[0].1.contains("'unknown'"));
}

#[test]
fn children_come_in_the_order_the_model_says() {
    let found = problems(ORDER, "<order><item qty=\"1\"/><id>7</id></order>");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "cvc.child-not-allowed");
    assert!(found[0].1.contains("'item'") && found[0].1.contains("expected 'id'"), "{found:?}");
    // Only the first mistake of an element is said, and what is inside the element that is not allowed is not checked.
    assert_eq!(keys(ORDER, "<order><x><y/></x><id>7</id><z/></order>"), ["cvc.child-not-allowed"]);
    // After the last one that may come.
    let found = problems(ORDER, "<order><id>7</id><item qty=\"1\"/><note>a</note><note>b</note></order>");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].1.contains("'note'") && found[0].1.contains("no more elements are allowed"), "{found:?}");
}

#[test]
fn the_content_must_be_complete_at_the_end_tag() {
    let found = problems(ORDER, "<order><id>7</id></order>");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "cvc.content-incomplete");
    assert!(found[0].1.contains("expected 'item'"), "{found:?}");
    assert_eq!(keys(ORDER, "<order/>"), ["cvc.content-incomplete"]);
    // A choice says every way on.
    let choice = "<xs:element name=\"c\"><xs:complexType><xs:choice><xs:element name=\"a\"/><xs:element name=\"b\"/></xs:choice></xs:complexType></xs:element>";
    let found = problems(choice, "<c/>");
    assert!(found[0].1.contains("one of 'a', 'b'"), "{found:?}");
}

#[test]
fn text_is_only_in_the_content_that_allows_it() {
    assert_eq!(keys(ORDER, "<order>text<id>7</id><item qty=\"1\"/></order>"), ["cvc.text-not-allowed"]);
    assert_eq!(keys(ORDER, "<order>\n  <id>7</id>\n  <item qty=\"1\"/>\n</order>"), [], "white space between elements is no text");
    assert_eq!(keys(ORDER, "<order><![CDATA[ ]]><id>7</id><item qty=\"1\"/></order>"), [], "nor in a CDATA section");
    assert_eq!(keys(ORDER, "<order><![CDATA[x]]><id>7</id><item qty=\"1\"/></order>"), ["cvc.text-not-allowed"]);
    let mixed = "<xs:element name=\"p\"><xs:complexType mixed=\"true\"><xs:sequence><xs:element name=\"b\" minOccurs=\"0\" maxOccurs=\"unbounded\"/></xs:sequence></xs:complexType></xs:element>";
    assert_eq!(keys(mixed, "<p>one <b/> two <b/></p>"), []);
    assert_eq!(keys(mixed, "<p>one <i/></p>"), ["cvc.child-not-allowed"]);
    let empty = "<xs:element name=\"e\"><xs:complexType/></xs:element>";
    assert_eq!(keys(empty, "<e/>"), []);
    assert_eq!(keys(empty, "<e> </e>"), ["cvc.text-not-allowed"], "empty means no characters at all");
    assert_eq!(keys(empty, "<e><x/></e>"), ["cvc.child-not-allowed"]);
}

#[test]
fn simple_values_are_checked_against_their_types() {
    assert_eq!(keys(ORDER, "<order><id>seven</id><item qty=\"1\"/></order>"), ["cvc.invalid-value"]);
    let found = problems(ORDER, "<order><id>7</id><item qty=\"0\"/></order>");
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].1.contains("the attribute 'qty'") && found[0].1.contains("cvc-minInclusive-valid"), "{found:?}");
    let types = "<xs:simpleType name=\"code\"><xs:restriction base=\"xs:token\"><xs:pattern value=\"[A-Z]{2}\"/></xs:restriction></xs:simpleType>\
        <xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"c\" type=\"code\" maxOccurs=\"unbounded\"/></xs:sequence></xs:complexType></xs:element>";
    assert_eq!(keys(types, "<r><c>DE</c><c>  FR </c></r>"), [], "token collapses white space before the pattern");
    let found = problems(types, "<r><c>de</c></r>");
    assert!(found[0].1.contains("cvc-pattern-valid"), "{found:?}");
    let e = "<xs:element name=\"e\"><xs:simpleType><xs:restriction base=\"xs:string\"><xs:enumeration value=\"a\"/><xs:enumeration value=\"b\"/></xs:restriction></xs:simpleType></xs:element>";
    assert_eq!(keys(e, "<e>a</e>"), []);
    assert!(problems(e, "<e>c</e>")[0].1.contains("cvc-enumeration-valid"));
    assert_eq!(keys(e, "<e/>"), ["cvc.invalid-value"], "no text is not 'a' or 'b'");
    assert_eq!(keys(e, "<e>a<x/></e>"), ["cvc.child-not-allowed"], "an element with a simple type has no elements");
}

#[test]
fn references_to_characters_and_entities_are_text() {
    let n = "<xs:element name=\"n\" type=\"xs:int\"/>";
    assert_eq!(keys(n, "<n>&#49;&#x32;</n>"), [], "12");
    assert_eq!(keys(n, "<n>&lt;</n>"), ["cvc.invalid-value"]);
    let s = "<xs:element name=\"s\" type=\"xs:string\"/>";
    assert_eq!(keys(s, "<s>a &amp; b &lt;&gt; &quot;&apos;</s>"), []);
    assert_eq!(keys(n, "<n><![CDATA[12]]></n>"), []);
    // An entity that is not known here is not checked.
    let xml = "<!DOCTYPE n [<!ENTITY e \"x\">]><n>&e;</n>";
    assert_eq!(keys(n, xml), []);
}

#[test]
fn attributes_are_declared_required_and_typed() {
    let found = problems(ORDER, "<order><id>7</id><item/></order>");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "cvc.attribute-required");
    assert!(found[0].1.contains("'qty'"));
    let found = problems(ORDER, "<order><id>7</id><item qty=\"1\" colour=\"red\"/></order>");
    assert_eq!((found.len(), found[0].0), (1, "cvc.attribute-not-allowed"), "{found:?}");
    assert_eq!(keys(ORDER, "<order currency=\"EUR\" currency2=\"x\"><id>7</id><item qty=\"1\"/></order>"), ["cvc.attribute-not-allowed"]);
    // An element with a simple type has no attributes, but those of the instance namespace.
    let s = "<xs:element name=\"s\" type=\"xs:string\"/>";
    assert_eq!(keys(s, "<s a=\"1\">x</s>"), ["cvc.attribute-not-allowed"]);
    assert_eq!(
        keys(
            s,
            "<s xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:schemaLocation=\"urn:a b.xsd\" xsi:noNamespaceSchemaLocation=\"c.xsd\">x</s>"
        ),
        []
    );
}

#[test]
fn fixed_values_of_attributes_and_elements() {
    let fixed = "<xs:element name=\"f\"><xs:complexType><xs:sequence><xs:element name=\"v\" type=\"xs:decimal\" fixed=\"1.5\"/></xs:sequence><xs:attribute name=\"k\" type=\"xs:int\" fixed=\"3\"/></xs:complexType></xs:element>";
    assert_eq!(keys(fixed, "<f k=\"3\"><v>1.50</v></f>"), [], "the same value, written another way");
    assert_eq!(keys(fixed, "<f><v/></f>"), [], "an empty element takes the fixed value");
    assert_eq!(keys(fixed, "<f k=\"03\"><v>1.5</v></f>"), []);
    let found = problems(fixed, "<f k=\"4\"><v>2</v></f>");
    assert_eq!(found.iter().map(|f| f.0).collect::<Vec<_>>(), ["cvc.fixed-value", "cvc.fixed-value"], "{found:?}");
    assert!(found[0].1.contains("'3'") && found[1].1.contains("'1.5'"));
    let default = "<xs:element name=\"d\" type=\"xs:int\" default=\"5\"/>";
    assert_eq!(keys(default, "<d/>"), []);
    assert_eq!(keys(default, "<d>9</d>"), []);
    assert_eq!(keys(default, "<d> </d>"), ["cvc.invalid-value"], "white space is text, and it is not an int");
}

#[test]
fn xsi_type_names_a_derived_type() {
    let schema = "<xs:complexType name=\"base\"><xs:sequence><xs:element name=\"a\"/></xs:sequence></xs:complexType>\
        <xs:complexType name=\"ext\"><xs:complexContent><xs:extension base=\"base\"><xs:sequence><xs:element name=\"b\"/></xs:sequence></xs:extension></xs:complexContent></xs:complexType>\
        <xs:complexType name=\"other\"><xs:sequence><xs:element name=\"z\"/></xs:sequence></xs:complexType>\
        <xs:element name=\"r\" type=\"base\"/>";
    let xsi = "xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"";
    assert_eq!(keys(schema, &format!("<r {xsi}><a/></r>")), []);
    assert_eq!(keys(schema, &format!("<r {xsi} xsi:type=\"ext\"><a/><b/></r>")), []);
    assert_eq!(keys(schema, &format!("<r {xsi} xsi:type=\"base\"><a/></r>")), [], "the type it has anyway");
    assert_eq!(keys(schema, &format!("<r {xsi}><a/><b/></r>")), ["cvc.child-not-allowed"], "b needs the extension");
    let found = problems(schema, &format!("<r {xsi} xsi:type=\"other\"><z/></r>"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].0, "cvc.xsi-type");
    assert!(found[0].1.contains("not derived"), "{found:?}");
    assert!(problems(schema, &format!("<r {xsi} xsi:type=\"nothing\"><a/></r>"))[0].1.contains("there is no type"));
    assert!(problems(schema, &format!("<r {xsi} xsi:type=\"p:ext\"><a/></r>"))[0].1.contains("prefix 'p' is not declared"));
    // The element blocks extension.
    let blocked = format!("{schema}<xs:element name=\"s\" type=\"base\" block=\"extension\"/>");
    assert_eq!(keys(&blocked, &format!("<s {xsi} xsi:type=\"ext\"><a/><b/></s>")), ["cvc.xsi-type"]);
    // A simple type derived from the one of the element.
    let simple = "<xs:element name=\"n\" type=\"xs:decimal\"/>";
    let xs = format!("xmlns:xs=\"{XSD_NAMESPACE}\"");
    assert_eq!(keys(simple, &format!("<n {xsi} {xs} xsi:type=\"xs:integer\">12</n>")), []);
    assert_eq!(keys(simple, &format!("<n {xsi} {xs} xsi:type=\"xs:integer\">1.5</n>")), ["cvc.invalid-value"]);
}

#[test]
fn abstract_elements_and_types_cannot_be_used_as_they_are() {
    let schema = "<xs:complexType name=\"shape\" abstract=\"true\"><xs:sequence><xs:element name=\"n\" minOccurs=\"0\"/></xs:sequence></xs:complexType>\
        <xs:complexType name=\"circle\"><xs:complexContent><xs:extension base=\"shape\"/></xs:complexContent></xs:complexType>\
        <xs:element name=\"s\" type=\"shape\"/><xs:element name=\"abs\" type=\"circle\" abstract=\"true\"/>";
    let xsi = "xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"";
    assert_eq!(keys(schema, "<s/>"), ["cvc.abstract-type"]);
    assert_eq!(keys(schema, &format!("<s {xsi} xsi:type=\"circle\"/>")), []);
    assert_eq!(keys(schema, &format!("<s {xsi} xsi:type=\"shape\"/>")), ["cvc.abstract-type"]);
    assert_eq!(keys(schema, "<abs/>"), ["cvc.abstract-element"]);
}

#[test]
fn xsi_nil_empties_a_nillable_element() {
    let schema = "<xs:element name=\"r\"><xs:complexType><xs:sequence>\
        <xs:element name=\"n\" type=\"xs:int\" nillable=\"true\"/><xs:element name=\"m\" type=\"xs:int\"/><xs:element name=\"f\" type=\"xs:int\" nillable=\"true\" fixed=\"4\" minOccurs=\"0\"/>\
        </xs:sequence></xs:complexType></xs:element>";
    let xsi = "xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"";
    assert_eq!(keys(schema, &format!("<r {xsi}><n xsi:nil=\"true\"/><m>1</m></r>")), [], "the empty text is not checked against int");
    assert_eq!(keys(schema, &format!("<r {xsi}><n xsi:nil=\"1\"/><m>1</m></r>")), []);
    assert_eq!(keys(schema, &format!("<r {xsi}><n xsi:nil=\"false\">3</n><m>1</m></r>")), []);
    assert_eq!(keys(schema, &format!("<r {xsi}><n xsi:nil=\"true\">3</n><m>1</m></r>")), ["cvc.nilled-element-has-content"]);
    assert_eq!(keys(schema, &format!("<r {xsi}><n xsi:nil=\"true\"><x/></n><m>1</m></r>")), ["cvc.nilled-element-has-content"]);
    assert_eq!(
        keys(schema, &format!("<r {xsi}><n xsi:nil=\"true\"/><m xsi:nil=\"true\"/></r>")),
        ["cvc.not-nillable", "cvc.invalid-value"],
        "and m then needs a value"
    );
    assert_eq!(
        keys(schema, &format!("<r {xsi}><n xsi:nil=\"yes\"/><m>1</m></r>")),
        ["cvc.bad-xsi-nil", "cvc.invalid-value"],
        "and then it is not nilled"
    );
    assert_eq!(keys(schema, &format!("<r {xsi}><n xsi:nil=\"true\"/><m>1</m><f xsi:nil=\"true\"/></r>")), ["cvc.nilled-element-is-fixed"]);
}

#[test]
fn a_member_of_a_substitution_group_takes_the_place_of_its_head() {
    let schema = "<xs:element name=\"head\" type=\"xs:string\"/><xs:element name=\"m\" type=\"xs:string\" substitutionGroup=\"head\"/>\
        <xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element ref=\"head\" maxOccurs=\"unbounded\"/></xs:sequence></xs:complexType></xs:element>";
    assert_eq!(keys(schema, "<r><head>a</head><m>b</m><head/></r>"), []);
    assert_eq!(keys(schema, "<r><other/></r>"), ["cvc.child-not-allowed"]);
    // And as the root.
    assert_eq!(keys(schema, "<m>x</m>"), []);
}

#[test]
fn a_wildcard_in_a_content_model_validates_as_it_asks() {
    let schema = "<xs:element name=\"known\" type=\"xs:int\"/>\
        <xs:element name=\"strict\"><xs:complexType><xs:sequence><xs:any processContents=\"strict\" maxOccurs=\"unbounded\"/></xs:sequence></xs:complexType></xs:element>\
        <xs:element name=\"lax\"><xs:complexType><xs:sequence><xs:any processContents=\"lax\" maxOccurs=\"unbounded\"/></xs:sequence></xs:complexType></xs:element>\
        <xs:element name=\"skip\"><xs:complexType><xs:sequence><xs:any processContents=\"skip\" maxOccurs=\"unbounded\"/></xs:sequence></xs:complexType></xs:element>";
    assert_eq!(keys(schema, "<strict><known>1</known></strict>"), []);
    assert_eq!(keys(schema, "<strict><known>x</known></strict>"), ["cvc.invalid-value"]);
    assert_eq!(keys(schema, "<strict><unknown/></strict>"), ["cvc.wildcard-element-not-declared"]);
    assert_eq!(keys(schema, "<lax><known>x</known></lax>"), ["cvc.invalid-value"], "declared, so checked");
    assert_eq!(keys(schema, "<lax><unknown><known>y</known></unknown></lax>"), ["cvc.invalid-value"], "not declared, but what is in it is");
    assert_eq!(keys(schema, "<skip><known>x</known><unknown><deeper/></unknown></skip>"), []);
}

#[test]
fn an_attribute_wildcard_validates_as_it_asks() {
    let schema = "<xs:attribute name=\"n\" type=\"xs:int\"/>\
        <xs:element name=\"strict\"><xs:complexType><xs:anyAttribute processContents=\"strict\"/></xs:complexType></xs:element>\
        <xs:element name=\"lax\"><xs:complexType><xs:anyAttribute processContents=\"lax\"/></xs:complexType></xs:element>\
        <xs:element name=\"skip\"><xs:complexType><xs:anyAttribute processContents=\"skip\"/></xs:complexType></xs:element>";
    // The attribute is in no namespace here, so it is found by its name alone.
    assert_eq!(keys(schema, "<strict n=\"1\"/>"), []);
    assert_eq!(keys(schema, "<strict n=\"x\"/>"), ["cvc.invalid-value"]);
    assert_eq!(keys(schema, "<strict m=\"x\"/>"), ["cvc.attribute-not-declared"]);
    assert_eq!(keys(schema, "<lax m=\"x\" n=\"1\"/>"), []);
    assert_eq!(keys(schema, "<lax n=\"x\"/>"), ["cvc.invalid-value"]);
    assert_eq!(keys(schema, "<skip n=\"x\" m=\"y\"/>"), []);
}

#[test]
fn ids_are_unique_and_references_find_them() {
    let schema = "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"e\" maxOccurs=\"unbounded\"><xs:complexType>\
        <xs:attribute name=\"id\" type=\"xs:ID\"/><xs:attribute name=\"ref\" type=\"xs:IDREF\"/><xs:attribute name=\"refs\" type=\"xs:IDREFS\"/>\
        </xs:complexType></xs:element></xs:sequence></xs:complexType></xs:element>";
    assert_eq!(keys(schema, "<r><e id=\"a\"/><e id=\"b\" ref=\"a\"/><e refs=\"a b  a\"/></r>"), []);
    let found = problems(schema, "<r><e id=\"a\"/><e id=\"a\"/></r>");
    assert_eq!((found.len(), found[0].0), (1, "cvc.duplicate-id"), "{found:?}");
    assert_eq!(keys(schema, "<r><e id=\"a\" ref=\"zz\"/></r>"), ["cvc.idref-has-no-id"]);
    assert_eq!(keys(schema, "<r><e id=\"a\" refs=\"a q r\"/></r>"), ["cvc.idref-has-no-id", "cvc.idref-has-no-id"]);
    assert_eq!(keys(schema, "<r><e ref=\"later\"/><e id=\"later\"/></r>"), [], "the ID may come after the reference");
    let element = "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"i\" type=\"xs:ID\" maxOccurs=\"2\"/></xs:sequence></xs:complexType></xs:element>";
    assert_eq!(keys(element, "<r><i>x</i><i>x</i></r>"), ["cvc.duplicate-id"], "an element can be an ID too");
}

#[test]
fn namespaces_decide_which_declaration_an_element_has() {
    let text = format!(
        "<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\" targetNamespace=\"urn:t\" elementFormDefault=\"qualified\" attributeFormDefault=\"unqualified\">\
         <xs:element name=\"root\"><xs:complexType><xs:sequence><xs:element name=\"child\" type=\"xs:QName\"/></xs:sequence><xs:attribute name=\"a\" type=\"xs:int\"/></xs:complexType></xs:element></xs:schema>"
    );
    let assembly = assembled_text(&text);
    let ok = |xml: &str| Keys(problems_in(&assembly, xml).into_iter().map(|(k, _)| k).collect());
    assert_eq!(ok("<root xmlns=\"urn:t\" a=\"1\"><child>x</child></root>"), []);
    assert_eq!(ok("<t:root xmlns:t=\"urn:t\" a=\"1\"><t:child>t:y</t:child></t:root>"), []);
    assert_eq!(ok("<root a=\"1\"><child>x</child></root>"), ["cvc.element-not-declared"], "no namespace is not urn:t");
    assert_eq!(ok("<root xmlns=\"urn:t\"><t:child xmlns:t=\"urn:t\">x</t:child></root>"), []);
    assert_eq!(ok("<root xmlns=\"urn:t\"><child xmlns=\"\">x</child></root>"), ["cvc.child-not-allowed"], "the child is in no namespace");
    assert_eq!(ok("<root xmlns=\"urn:t\" xmlns:t=\"urn:t\" t:a=\"1\"><child>x</child></root>"), ["cvc.attribute-not-allowed"], "a is not qualified");
    // The prefix of a QName value is looked up where the value is.
    assert_eq!(ok("<root xmlns=\"urn:t\"><child>nope:y</child></root>"), ["cvc.invalid-value"]);
    assert_eq!(ok("<root xmlns=\"urn:t\"><child xmlns:nope=\"urn:n\">nope:y</child></root>"), []);
}

#[test]
fn an_all_group_takes_its_elements_in_any_order() {
    let schema = "<xs:element name=\"r\"><xs:complexType><xs:all><xs:element name=\"a\"/><xs:element name=\"b\"/><xs:element name=\"c\" minOccurs=\"0\"/></xs:all></xs:complexType></xs:element>";
    assert_eq!(keys(schema, "<r><b/><a/></r>"), []);
    assert_eq!(keys(schema, "<r><c/><b/><a/></r>"), []);
    assert_eq!(keys(schema, "<r><a/></r>"), ["cvc.content-incomplete"]);
    assert_eq!(keys(schema, "<r><a/><a/><b/></r>"), ["cvc.child-not-allowed"]);
}

#[test]
fn a_document_nested_deeper_than_the_limit_is_said_so_once() {
    let schema =
        "<xs:element name=\"n\"><xs:complexType><xs:sequence><xs:element ref=\"n\" minOccurs=\"0\"/></xs:sequence></xs:complexType></xs:element>";
    let depth = xmlcraft_validate::XSD_MAX_OPEN + 50;
    let xml = format!("{}{}", "<n>".repeat(depth), "</n>".repeat(depth));
    let parsed = parse_with(&xml, &Limits { max_depth: depth + 10, ..Limits::default() });
    let assembly = assembled(schema);
    let found = validate_against_schema(&parsed.syntax(), &assembly.schema);
    assert_eq!(found.iter().filter(|d| d.code.key() == "cvc.limit").count(), 1);
}

#[test]
fn a_document_that_is_not_there_has_nothing_wrong_with_it() {
    let assembly = assembled(ORDER);
    for xml in ["", "<!-- nothing -->", "<?xml version=\"1.0\"?>"] {
        let parsed = parse(xml);
        assert!(validate_against_schema(&parsed.syntax(), &assembly.schema).is_empty(), "{xml:?}");
    }
}

#[test]
fn a_namespace_declaration_is_in_scope_for_its_element_only() {
    let schema = "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"a\" type=\"xs:QName\"/><xs:element name=\"b\" type=\"xs:QName\"/></xs:sequence></xs:complexType></xs:element>";
    assert_eq!(keys(schema, "<r><a xmlns:p=\"urn:n\">p:x</a><b xmlns:p=\"urn:m\">p:y</b></r>"), []);
    assert_eq!(keys(schema, "<r><a xmlns:p=\"urn:n\">p:x</a><b>p:y</b></r>"), ["cvc.invalid-value"], "p is not declared for b");
    assert_eq!(keys(schema, "<r xmlns:p=\"urn:n\"><a>p:x</a><b>p:y</b></r>"), [], "declared on the parent, it holds for the children");
}

#[test]
fn the_value_of_an_attribute_has_its_references_replaced() {
    let schema = "<xs:element name=\"r\"><xs:complexType><xs:attribute name=\"s\" type=\"xs:string\" fixed=\"a&amp;b&lt;c\"/><xs:attribute name=\"n\" type=\"xs:int\"/></xs:complexType></xs:element>";
    assert_eq!(keys(schema, "<r s=\"a&amp;b&lt;c\"/>"), []);
    assert_eq!(keys(schema, "<r s=\"a&#38;b&#x3C;c\"/>"), [], "the same by character references");
    assert_eq!(keys(schema, "<r s=\"a&amp;b\"/>"), ["cvc.fixed-value"]);
    assert_eq!(keys(schema, "<r n=\"&#49;&#50;\"/>"), []);
    assert_eq!(keys(schema, "<!DOCTYPE r [<!ENTITY e \"1\">]><r n=\"&e;\"/>"), ["cvc.invalid-value"], "an entity in a value is not replaced here");
}

#[test]
fn a_long_list_of_what_could_come_is_cut_short() {
    let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
    let choice: String = names.iter().map(|n| format!("<xs:element name=\"{n}\"/>")).collect();
    let schema = format!("<xs:element name=\"r\"><xs:complexType><xs:choice>{choice}</xs:choice></xs:complexType></xs:element>");
    let found = problems(&schema, "<r/>");
    assert!(found[0].1.contains("one of 'a', 'b', 'c', 'd', 'e', 'f' or 2 more"), "{found:?}");
    let two = problems(
        "<xs:element name=\"r\"><xs:complexType><xs:choice><xs:element name=\"a\"/><xs:element name=\"b\"/></xs:choice></xs:complexType></xs:element>",
        "<r><c/></r>",
    );
    assert!(two[0].1.contains("expected one of 'a', 'b'") && !two[0].1.contains("more"), "{two:?}");
}

#[test]
fn what_a_wildcard_lets_in_is_said_in_words() {
    let any = "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:any/><xs:any namespace=\"##other\"/><xs:any namespace=\"urn:a ##local\"/></xs:sequence></xs:complexType></xs:element>";
    let found = problems(any, "<r/>");
    assert!(found.iter().any(|p| p.1.contains("'any element'")), "{found:?}");
    let other = problems(any, "<r><x/></r>");
    assert!(other.iter().any(|p| p.1.contains("'any element from another namespace'")), "{other:?}");
    let list = problems(any, "<r><x/><y xmlns=\"urn:z\"/></r>");
    assert!(list.iter().any(|p| p.1.contains("'any element from urn:a or no namespace'")), "{list:?}");
}

#[test]
fn nil_is_written_true_false_1_or_0_and_empties_any_kind_of_element() {
    let schema = "<xs:element name=\"r\" nillable=\"true\"><xs:complexType><xs:sequence><xs:element name=\"must\"/></xs:sequence></xs:complexType></xs:element>";
    let xsi = "xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"";
    assert_eq!(keys(schema, &format!("<r {xsi} xsi:nil=\"true\"/>")), [], "a nilled element needs none of its content");
    assert_eq!(keys(schema, &format!("<r {xsi} xsi:nil=\"1\"/>")), []);
    assert_eq!(keys(schema, &format!("<r {xsi} xsi:nil=\"0\"><must/></r>")), [], "0 is false");
    assert_eq!(keys(schema, &format!("<r {xsi} xsi:nil=\"false\"/>")), ["cvc.content-incomplete"]);
    assert_eq!(keys(schema, &format!("<r {xsi} xsi:nil=\"true\"><must/></r>")), ["cvc.nilled-element-has-content"]);
}

#[test]
fn what_is_skipped_by_a_wildcard_is_skipped_all_the_way_down() {
    let schema = "<xs:element name=\"known\" type=\"xs:int\"/>\
        <xs:element name=\"skip\"><xs:complexType><xs:sequence><xs:any processContents=\"skip\"/></xs:sequence></xs:complexType></xs:element>\
        <xs:element name=\"lax\"><xs:complexType><xs:sequence><xs:any processContents=\"lax\"/></xs:sequence></xs:complexType></xs:element>";
    assert_eq!(keys(schema, "<skip><wrap><known>x</known></wrap></skip>"), []);
    assert_eq!(keys(schema, "<lax><wrap><known>x</known></wrap></lax>"), ["cvc.invalid-value"]);
}

#[test]
fn only_the_first_mistake_in_an_element_is_said() {
    let empty = "<xs:element name=\"e\"><xs:complexType/></xs:element>";
    assert_eq!(keys(empty, "<e><x/><y/><z/></e>"), ["cvc.child-not-allowed"]);
    assert_eq!(keys(empty, "<e>a<x/></e>"), ["cvc.text-not-allowed"], "the text comes first, and then the rest is not said");
    let simple = "<xs:element name=\"s\"><xs:simpleType><xs:restriction base=\"xs:string\"><xs:enumeration value=\"a\"/></xs:restriction></xs:simpleType></xs:element>";
    assert_eq!(keys(simple, "<s>zzz<x/><y/></s>"), ["cvc.child-not-allowed"], "the text is not checked when the element has children as well");
}

#[test]
fn an_attribute_wildcard_lets_in_the_namespaces_it_names() {
    let schema =
        "<xs:element name=\"r\"><xs:complexType><xs:anyAttribute namespace=\"urn:a\" processContents=\"skip\"/></xs:complexType></xs:element>";
    assert_eq!(keys(schema, "<r xmlns:a=\"urn:a\" a:x=\"1\"/>"), []);
    assert_eq!(keys(schema, "<r xmlns:b=\"urn:b\" b:x=\"1\"/>"), ["cvc.attribute-not-allowed"]);
    assert_eq!(keys(schema, "<r x=\"1\"/>"), ["cvc.attribute-not-allowed"], "no namespace is not urn:a");
}

#[test]
fn what_is_mandatory_is_what_is_said_when_content_is_missing() {
    let schema = "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"a\" minOccurs=\"0\"/><xs:element name=\"b\"/></xs:sequence></xs:complexType></xs:element>";
    let found = problems(schema, "<r/>");
    assert!(found[0].1.contains("expected 'b'") && !found[0].1.contains("'a'"), "{found:?}");
    // Where the child is wrong, everything that could come is said.
    let found = problems(schema, "<r><c/></r>");
    assert!(found[0].1.contains("expected 'b'") || found[0].1.contains("one of"), "{found:?}");
}

#[test]
fn an_element_with_no_namespace_in_a_namespace_that_says_so() {
    let text = format!(
        "<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\" targetNamespace=\"urn:t\"><xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"c\" type=\"xs:int\"/></xs:sequence></xs:complexType></xs:element></xs:schema>"
    );
    let assembly = assembled_text(&text);
    let ok = |xml: &str| Keys(problems_in(&assembly, xml).into_iter().map(|(k, _)| k).collect());
    assert_eq!(ok("<r xmlns=\"urn:t\"><c xmlns=\"\">1</c></r>"), [], "local elements are not qualified, so a child in no namespace is right");
    assert_eq!(ok("<r xmlns=\"urn:t\"><c>1</c></r>"), ["cvc.child-not-allowed"], "and one in the default namespace is not");
}

#[test]
fn a_reference_to_an_entity_in_a_value_is_kept_as_written() {
    let schema =
        "<xs:element name=\"r\"><xs:complexType><xs:attribute name=\"s\" type=\"xs:string\" fixed=\"a&amp;e;b\"/></xs:complexType></xs:element>";
    assert_eq!(keys(schema, "<!DOCTYPE r [<!ENTITY e \"1\">]><r s=\"a&e;b\"/>"), []);
}
