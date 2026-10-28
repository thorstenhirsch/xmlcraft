//! Whatever the text, reading it gives diagnostics and a model and never panics; mistakes in a schema do not make the rest unreadable.

use proptest::prelude::*;
use xmlcraft_xsd::model::Item;
use xmlcraft_xsd::read;

const XS: &str = "http://www.w3.org/2001/XMLSchema";

const SCHEMA: &str = "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\" xmlns:t=\"urn:t\" targetNamespace=\"urn:t\" elementFormDefault=\"qualified\">\
<xs:import namespace=\"urn:o\" schemaLocation=\"o.xsd\"/>\
<xs:simpleType name=\"code\"><xs:restriction base=\"xs:string\"><xs:pattern value=\"[A-Z]{3}\"/><xs:enumeration value=\"ABC\"/></xs:restriction></xs:simpleType>\
<xs:simpleType name=\"codes\"><xs:list itemType=\"t:code\"/></xs:simpleType>\
<xs:simpleType name=\"either\"><xs:union memberTypes=\"t:code xs:int\"/></xs:simpleType>\
<xs:complexType name=\"item\" mixed=\"false\"><xs:sequence><xs:element name=\"id\" type=\"t:code\" minOccurs=\"0\"/>\
<xs:choice maxOccurs=\"unbounded\"><xs:element ref=\"t:note\"/><xs:any namespace=\"##other\" processContents=\"lax\"/></xs:choice></xs:sequence>\
<xs:attribute name=\"n\" type=\"xs:int\" use=\"required\"/><xs:attributeGroup ref=\"t:common\"/></xs:complexType>\
<xs:attributeGroup name=\"common\"><xs:attribute name=\"lang\" type=\"xs:language\" default=\"en\"/><xs:anyAttribute/></xs:attributeGroup>\
<xs:group name=\"g\"><xs:sequence><xs:element name=\"a\"/><xs:element name=\"b\"/></xs:sequence></xs:group>\
<xs:element name=\"note\" type=\"xs:string\" nillable=\"true\"/>\
<xs:element name=\"root\" type=\"t:item\"><xs:key name=\"k\"><xs:selector xpath=\"t:item\"/><xs:field xpath=\"@n\"/></xs:key></xs:element>\
<xs:notation name=\"gif\" public=\"image/gif\"/></xs:schema>";

#[test]
fn the_sample_is_a_clean_schema() {
    let r = read(SCHEMA, "");
    assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics.iter().map(|d| d.message()).collect::<Vec<_>>());
    assert_eq!(r.document.expect("a schema").items.iter().filter(|i| matches!(i, Item::SimpleType(_))).count(), 3);
}

fn mutated() -> impl Strategy<Value = String> {
    let alphabet = prop::sample::select(vec!['<', '>', '"', '\'', '/', '=', '&', ';', ':', ' ', 'x', '0', '\u{0}', '\u{1F600}', '[', ']', '{', '}']);
    (any::<prop::sample::Index>(), any::<prop::sample::Index>(), any::<prop::sample::Index>(), alphabet).prop_map(|(a, b, c, ch)| {
        let chars: Vec<char> = SCHEMA.chars().collect();
        let (mut from, mut to) = (a.index(chars.len()), b.index(chars.len()));
        if from > to {
            std::mem::swap(&mut from, &mut to);
        }
        let mut out: String = chars[..from].iter().collect();
        match c.index(3) {
            0 => {}                                                                // deleted
            1 => out.extend(chars[from..to].iter().chain(chars[from..to].iter())), // doubled
            _ => out.push(ch),                                                     // replaced by one character
        }
        out.extend(&chars[to..]);
        out
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(600))]

    #[test]
    fn a_damaged_schema_is_read_without_a_panic(text in mutated()) {
        let r = read(&text, "m.xsd");
        for d in &r.diagnostics {
            prop_assert!(d.span.start <= d.span.end && d.span.end <= text.len(), "{:?} in {}", d, text.len());
            prop_assert!(text.is_char_boundary(d.span.start) && text.is_char_boundary(d.span.end), "{:?}", d);
            prop_assert!(!d.message().is_empty());
        }
    }

    #[test]
    fn any_text_is_read_without_a_panic(text in "\\PC{0,200}") {
        let _ = read(&text, "");
    }

    #[test]
    fn markup_soup_is_read_without_a_panic(parts in prop::collection::vec(prop::sample::select(vec![
        "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\">", "</xs:schema>", "<xs:element name=\"e\">", "</xs:element>", "<xs:element ref=\"e\"/>",
        "<xs:complexType>", "</xs:complexType>", "<xs:sequence>", "</xs:sequence>", "<xs:simpleType name=\"s\">", "</xs:simpleType>",
        "<xs:restriction base=\"xs:string\">", "</xs:restriction>", "<xs:attribute name=\"a\"/>", "<xs:any/>", "<xs:annotation>", "</xs:annotation>",
        "text", "<!--c-->", "&amp;", "<![CDATA[x]]>", "<", ">", "\"",
    ]), 0..40)) {
        let _ = read(&parts.concat(), "");
    }
}

#[test]
fn deep_nesting_is_cut_off_and_said_so() {
    let depth = 5000;
    let text = format!("<xs:schema xmlns:xs=\"{XS}\">{}{}</xs:schema>", "<xs:sequence>".repeat(depth), "</xs:sequence>".repeat(depth));
    let r = read(&text, "");
    assert!(r.diagnostics.iter().any(|d| d.code.key() == "xsd.limit"), "{:?}", r.diagnostics.iter().map(|d| d.code.key()).collect::<Vec<_>>());
}

#[test]
fn a_wide_document_is_read() {
    let n = 20_000;
    let text = format!("<xs:schema xmlns:xs=\"{XS}\">{}</xs:schema>", (0..n).map(|i| format!("<xs:element name=\"e{i}\"/>")).collect::<String>());
    let r = read(&text, "");
    assert!(r.diagnostics.is_empty());
    assert_eq!(r.document.expect("a schema").items.len(), n);
}
