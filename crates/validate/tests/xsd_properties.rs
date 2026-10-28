//! Whatever the document, validating it against a schema gives diagnostics and never panics; and a document that is valid stays valid
//! when white space between its elements changes.

use proptest::prelude::*;
use xmlcraft_syntax::parse;
use xmlcraft_validate::validate_against_schema;
use xmlcraft_xsd::{Assembly, NoLoader, assemble, load};

const SCHEMA: &str = "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\" targetNamespace=\"urn:t\" xmlns:t=\"urn:t\" elementFormDefault=\"qualified\">\
<xs:simpleType name=\"code\"><xs:restriction base=\"xs:token\"><xs:pattern value=\"[A-Z]{3}\"/></xs:restriction></xs:simpleType>\
<xs:complexType name=\"item\"><xs:sequence><xs:element name=\"id\" type=\"t:code\" minOccurs=\"0\"/>\
<xs:choice maxOccurs=\"unbounded\"><xs:element ref=\"t:note\"/><xs:any namespace=\"##other\" processContents=\"lax\"/></xs:choice></xs:sequence>\
<xs:attribute name=\"n\" type=\"xs:int\" use=\"required\"/><xs:attribute name=\"ref\" type=\"xs:IDREF\"/><xs:anyAttribute processContents=\"lax\"/></xs:complexType>\
<xs:element name=\"note\" type=\"xs:string\" nillable=\"true\"/>\
<xs:element name=\"root\" type=\"t:item\"/></xs:schema>";

const DOCUMENT: &str = "<t:root xmlns:t=\"urn:t\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" n=\"4\" ref=\"x\">\
<t:id>ABC</t:id><t:note>one</t:note><t:note xsi:nil=\"true\"/><o:other xmlns:o=\"urn:o\" a=\"1\"><b/></o:other></t:root>";

fn schema() -> Assembly {
    let documents = load(SCHEMA, "/s/root.xsd", &mut NoLoader);
    let assembly = assemble(&documents);
    assert!(assembly.diagnostics.is_empty(), "{:?}", assembly.diagnostics.iter().map(|d| d.diagnostic.message()).collect::<Vec<_>>());
    assembly
}

fn keys(document: &str) -> Vec<&'static str> {
    let parsed = parse(document);
    validate_against_schema(&parsed.syntax(), &schema().schema).into_iter().map(|d| d.code.key()).collect()
}

#[test]
fn the_sample_document_is_valid_but_for_the_missing_id() {
    // The IDREF `x` has no ID: the one thing wrong with it.
    assert_eq!(keys(DOCUMENT), ["cvc.idref-has-no-id"]);
}

fn mutated() -> impl Strategy<Value = String> {
    let alphabet =
        prop::sample::select(vec!['<', '>', '"', '\'', '/', '=', '&', ';', ':', ' ', 'x', '0', '\u{0}', '\u{1F600}', '[', ']', '{', '}', '!', '?']);
    (any::<prop::sample::Index>(), any::<prop::sample::Index>(), any::<prop::sample::Index>(), alphabet).prop_map(|(a, b, c, ch)| {
        let chars: Vec<char> = DOCUMENT.chars().collect();
        let (mut from, mut to) = (a.index(chars.len()), b.index(chars.len()));
        if from > to {
            std::mem::swap(&mut from, &mut to);
        }
        let mut out: String = chars.get(..from).unwrap_or_default().iter().collect();
        match c.index(3) {
            0 => {}
            1 => out.push(ch),
            _ => {
                out.push(ch);
                out.extend(chars.get(from..to).unwrap_or_default());
            }
        }
        out.extend(chars.get(to..).unwrap_or_default());
        out
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn a_damaged_document_never_panics(text in mutated()) {
        let assembly = schema();
        let parsed = parse(&text);
        let _ = validate_against_schema(&parsed.syntax(), &assembly.schema);
    }

    #[test]
    fn any_text_never_panics(text in "\\PC{0,200}") {
        let assembly = schema();
        let parsed = parse(&text);
        let _ = validate_against_schema(&parsed.syntax(), &assembly.schema);
    }

    #[test]
    fn white_space_between_elements_changes_nothing(gap in "[ \t\r\n]{0,6}") {
        let spaced = DOCUMENT.replace("><", &format!(">{gap}<"));
        // Not inside the element-only root: the `note` elements have text, but they are not touched by `><`.
        prop_assert_eq!(keys(&spaced), keys(DOCUMENT));
    }
}
