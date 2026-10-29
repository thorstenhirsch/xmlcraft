//! Schemas inferred from documents: what they say, and that each one validates the documents it was learned from.

use xmlcraft_dtd::NoLoader;
use xmlcraft_infer::{EnumerationRule, Generated, InferError, Layout, Options, Output, infer};
use xmlcraft_syntax::{Severity, parse};
use xmlcraft_validate::check_against_schema;
use xmlcraft_xsd::{LoadRequest, Loaded, SchemaLoader};

const XML_XSD: &str = "http://www.w3.org/2001/xml.xsd";
const XS: &str = "http://www.w3.org/2001/XMLSchema";

/// The generated files by name, and a stand-in for `xml.xsd` (the real one is bundled with the app).
struct Files<'a>(&'a Generated);

impl SchemaLoader for Files<'_> {
    fn load(&mut self, request: &LoadRequest<'_>) -> Result<Loaded, String> {
        let wanted = request.location.unwrap_or_default();
        if wanted == XML_XSD {
            let text = format!(
                "<xs:schema xmlns:xs=\"{XS}\" targetNamespace=\"http://www.w3.org/XML/1998/namespace\"><xs:attribute name=\"lang\" type=\"xs:string\"/><xs:attribute name=\"space\" type=\"xs:string\"/></xs:schema>"
            );
            return Ok(Loaded { text, location: wanted.to_owned() });
        }
        let name = wanted.rsplit('/').next().unwrap_or(wanted);
        self.0
            .files
            .iter()
            .find(|f| f.name == name)
            .map(|f| Loaded { text: f.text.clone(), location: format!("/d/{}", f.name) })
            .ok_or_else(|| format!("{wanted}: no such file"))
    }
}

fn generate(documents: &[&str], options: Options) -> Generated {
    infer(documents, &options).unwrap_or_else(|e| panic!("inference failed: {e}"))
}

fn xsd(layout: Layout) -> Options {
    Options { output: Output::Xsd { layout }, ..Options::default() }
}

fn dtd() -> Options {
    Options { output: Output::Dtd, ..Options::default() }
}

/// The problems of `document` against the schema in the file `main` of `generated`, schema problems included.
fn xsd_problems(generated: &Generated, main: &str, document: &str) -> Vec<String> {
    let parsed = parse(document);
    let found = check_against_schema(&parsed.syntax(), parsed.diagnostics(), "/d/doc.xml", Some(main), &mut Files(generated));
    let mut out: Vec<String> =
        found.well_formedness.iter().chain(&found.validity).filter(|d| d.severity() == Severity::Error).map(|d| d.message()).collect();
    out.extend(found.schema_problems.iter().filter(|p| p.diagnostic.severity() == Severity::Error).map(|p| p.diagnostic.message()));
    if let xmlcraft_validate::SchemaOutcome::NoSchema(why) = &found.outcome {
        out.push(why.clone());
    }
    out
}

/// The problems of `document` against the DTD, which is put into its DOCTYPE.
fn dtd_problems(generated: &Generated, document: &str) -> Vec<String> {
    let dtd = &generated.files[0].text;
    // After the XML declaration, if there is one, and before the root element.
    let at = document.find("?>").map(|i| i + 2).unwrap_or(0);
    let root_start = at + document[at..].find('<').unwrap_or(0);
    let root: String = document[root_start + 1..].chars().take_while(|c| !c.is_whitespace() && *c != '>' && *c != '/').collect();
    let text = format!("{}<!DOCTYPE {root} [\n{dtd}]>\n{}", &document[..root_start], &document[root_start..]);
    let parsed = parse(&text);
    let found = xmlcraft_validate::check(&parsed.syntax(), &text, parsed.diagnostics(), "", &mut NoLoader);
    found
        .well_formedness
        .iter()
        .chain(&found.validity)
        .filter(|p| p.diagnostic.severity() == Severity::Error)
        .map(|p| p.diagnostic.message())
        .collect()
}

const LAYOUTS: [Layout; 3] = [Layout::RussianDoll, Layout::SalamiSlice, Layout::VenetianBlind];

/// `documents` validate against the schema generated from them in every layout, and against the DTD.
fn assert_all_validate(documents: &[&str]) {
    for layout in LAYOUTS {
        let generated = generate(documents, xsd(layout));
        for document in documents {
            let problems = xsd_problems(&generated, "schema.xsd", document);
            assert!(problems.is_empty(), "{layout:?}: {problems:?}\n{}\n{document}", generated.files[0].text);
        }
    }
    let generated = generate(documents, dtd());
    for document in documents {
        let problems = dtd_problems(&generated, document);
        assert!(problems.is_empty(), "DTD: {problems:?}\n{}\n{document}", generated.files[0].text);
    }
}

#[test]
fn a_small_document_gives_the_schema_it_looks_like() {
    let generated =
        generate(&["<order id=\"7\"><item sku=\"a\">Tea</item><item sku=\"b\">Coffee</item><note>x</note></order>"], xsd(Layout::RussianDoll));
    assert_eq!(generated.files.len(), 1);
    assert_eq!(generated.files[0].name, "schema.xsd");
    let expected = r#"<?xml version="1.0" encoding="UTF-8"?>
<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="order">
    <xs:complexType>
      <xs:sequence>
        <xs:element name="item" maxOccurs="unbounded">
          <xs:complexType>
            <xs:simpleContent>
              <xs:extension base="xs:string">
                <xs:attribute name="sku" use="required" type="xs:string"/>
              </xs:extension>
            </xs:simpleContent>
          </xs:complexType>
        </xs:element>
        <xs:element name="note" type="xs:string"/>
      </xs:sequence>
      <xs:attribute name="id" use="required" type="xs:integer"/>
    </xs:complexType>
  </xs:element>
</xs:schema>
"#;
    assert_eq!(generated.files[0].text, expected);
}

#[test]
fn the_same_document_in_the_other_layouts() {
    let doc = "<order id=\"7\"><item>Tea</item></order>";
    let salami = &generate(&[doc], xsd(Layout::SalamiSlice)).files[0].text;
    assert!(
        salami.contains("<xs:element name=\"order\">")
            && salami.contains("<xs:element name=\"item\" type=\"xs:string\"/>")
            && salami.contains("<xs:element ref=\"item\"/>"),
        "{salami}"
    );
    let venetian = &generate(&[doc], xsd(Layout::VenetianBlind)).files[0].text;
    assert!(
        venetian.contains("<xs:complexType name=\"orderType\">") && venetian.contains("<xs:element name=\"order\" type=\"orderType\"/>"),
        "{venetian}"
    );
    assert!(venetian.contains("<xs:element name=\"item\" type=\"xs:string\"/>"), "an element with only text is a built-in type: {venetian}");
    assert_all_validate(&[doc]);
}

#[test]
fn types_are_the_narrowest_the_values_allow() {
    let docs = ["<r b=\"true\" i=\"1\" d=\"1.5\" t=\"2024-05-01\" s=\"x\"><n>10</n><n>-2</n><when>2024-05-01T10:00:00</when></r>"];
    let text = &generate(&docs, xsd(Layout::RussianDoll)).files[0].text;
    for expected in [
        "name=\"b\" use=\"required\" type=\"xs:boolean\"",
        "name=\"i\" use=\"required\" type=\"xs:integer\"",
        "name=\"d\" use=\"required\" type=\"xs:decimal\"",
        "name=\"t\" use=\"required\" type=\"xs:date\"",
        "name=\"s\" use=\"required\" type=\"xs:string\"",
        "<xs:element name=\"n\" type=\"xs:integer\" maxOccurs=\"unbounded\"/>",
        "<xs:element name=\"when\" type=\"xs:dateTime\"/>",
    ] {
        assert!(text.contains(expected), "{expected}\n{text}");
    }
    let plain = generate(&docs, Options { detect_types: false, ..xsd(Layout::RussianDoll) });
    assert!(!plain.files[0].text.contains("xs:integer") && !plain.files[0].text.contains("xs:boolean"), "{}", plain.files[0].text);
    assert_all_validate(&docs);
}

#[test]
fn a_few_values_each_seen_often_are_an_enumeration() {
    let docs = ["<r><i kind=\"a\"/><i kind=\"b\"/><i kind=\"a\"/><i kind=\"b\"/><i kind=\"c\"/></r>"];
    let none = generate(&docs, Options { enumerations: None, ..xsd(Layout::RussianDoll) });
    assert!(!none.files[0].text.contains("enumeration"));
    // c is seen once: too rare for the default rule.
    assert!(!generate(&docs, xsd(Layout::RussianDoll)).files[0].text.contains("enumeration"));
    let rule = Options { enumerations: Some(EnumerationRule { max_values: 5, min_occurrences: 1 }), ..xsd(Layout::RussianDoll) };
    let text = &generate(&docs, rule).files[0].text;
    for value in ["a", "b", "c"] {
        assert!(text.contains(&format!("<xs:enumeration value=\"{value}\"/>")), "{text}");
    }
    let dtd_text = &generate(&docs, Options { output: Output::Dtd, ..rule }).files[0].text;
    assert!(dtd_text.contains("kind (a|b|c) #REQUIRED"), "{dtd_text}");
    assert_eq!(infer(&docs, &rule).map(|g| xsd_problems(&g, "schema.xsd", docs[0])), Ok(Vec::new()));
}

#[test]
fn optional_and_repeated_children_and_attributes() {
    let docs = ["<r><a/><b/><b/></r>", "<r><a/><c x=\"1\"/></r>", "<r><a/><b/><c/></r>"];
    let generated = generate(&docs, xsd(Layout::RussianDoll));
    let text = &generated.files[0].text;
    assert!(text.contains("<xs:element name=\"b\" minOccurs=\"0\" maxOccurs=\"unbounded\">"), "{text}");
    assert!(text.contains("<xs:attribute name=\"x\" type=\"xs:integer\"/>"), "an attribute some elements lack is optional: {text}");
    let bounded = generate(&docs, Options { unbounded: false, ..xsd(Layout::RussianDoll) });
    assert!(bounded.files[0].text.contains("<xs:element name=\"b\" minOccurs=\"0\" maxOccurs=\"2\">"), "{}", bounded.files[0].text);
    assert_all_validate(&docs);
}

#[test]
fn names_that_never_occur_together_are_a_choice() {
    let docs = ["<r><a/><x/></r>", "<r><b/><x/></r>"];
    let text = &generate(&docs, xsd(Layout::RussianDoll)).files[0].text;
    assert!(text.contains("<xs:choice>"), "{text}");
    let dtd_text = &generate(&docs, dtd()).files[0].text;
    assert!(dtd_text.contains("<!ELEMENT r ((a|b), x)>"), "{dtd_text}");
    assert_all_validate(&docs);
}

#[test]
fn text_with_child_elements_is_mixed_content() {
    let docs = ["<p>Some <b>bold</b> and <i>italic</i> text.</p>", "<p>Plain</p>"];
    let text = &generate(&docs, xsd(Layout::RussianDoll)).files[0].text;
    assert!(text.contains("mixed=\"true\""), "{text}");
    let dtd_text = &generate(&docs, dtd()).files[0].text;
    assert!(dtd_text.contains("<!ELEMENT p (#PCDATA|b|i)*>"), "{dtd_text}");
    assert_all_validate(&docs);
}

#[test]
fn an_element_inside_itself_is_a_global_declaration_that_is_referred_to() {
    let docs = ["<doc><section><title>A</title><section><title>B</title></section></section></doc>"];
    let text = &generate(&docs, xsd(Layout::RussianDoll)).files[0].text;
    assert!(text.contains("<xs:element ref=\"section\""), "{text}");
    assert_eq!(text.matches("<xs:element name=\"section\"").count(), 1, "declared once, at the top: {text}");
    assert_all_validate(&docs);
}

#[test]
fn an_element_with_several_parents_is_declared_once() {
    let docs = ["<r><a><item><name>x</name></item></a><b><item><name>y</name></item></b></r>"];
    let text = &generate(&docs, xsd(Layout::RussianDoll)).files[0].text;
    assert_eq!(text.matches("<xs:element name=\"item\"").count(), 1, "{text}");
    assert_all_validate(&docs);
}

#[test]
fn several_documents_are_learned_together() {
    let docs = ["<r><a/></r>", "<r><a/><b/></r>", "<r><a/><a/><b/><b/></r>"];
    let text = &generate(&docs, xsd(Layout::RussianDoll)).files[0].text;
    assert!(
        text.contains("<xs:element name=\"a\" maxOccurs=\"unbounded\">")
            && text.contains("<xs:element name=\"b\" minOccurs=\"0\" maxOccurs=\"unbounded\">"),
        "{text}"
    );
    assert_all_validate(&docs);
}

#[test]
fn a_default_namespace_is_the_target_namespace() {
    let docs = ["<r xmlns=\"urn:t\"><a>1</a></r>"];
    for layout in LAYOUTS {
        let generated = generate(&docs, xsd(layout));
        assert_eq!(generated.files.len(), 1);
        assert_eq!(generated.files[0].namespace.as_deref(), Some("urn:t"));
        let text = &generated.files[0].text;
        assert!(text.contains("targetNamespace=\"urn:t\"") && text.contains("elementFormDefault=\"qualified\""), "{text}");
        assert_eq!(xsd_problems(&generated, "schema.xsd", docs[0]), Vec::<String>::new(), "{layout:?}\n{text}");
    }
}

#[test]
fn a_schema_per_namespace_that_import_each_other() {
    let docs = ["<p:r xmlns:p=\"urn:p\" xmlns:q=\"urn:q\"><q:a q:flag=\"1\"/><p:b/></p:r>"];
    for layout in LAYOUTS {
        let generated = generate(&docs, xsd(layout));
        assert_eq!(generated.files.len(), 2, "{layout:?}");
        let names: Vec<&str> = generated.files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names.len(), names.iter().collect::<std::collections::BTreeSet<_>>().len(), "file names are different: {names:?}");
        let p = generated.files.iter().find(|f| f.namespace.as_deref() == Some("urn:p")).expect("a schema for p");
        assert!(p.text.contains("<xs:import namespace=\"urn:q\""), "{}", p.text);
        assert_eq!(
            xsd_problems(&generated, &p.name, docs[0]),
            Vec::<String>::new(),
            "{layout:?}\n{}\n{}",
            generated.files[0].text,
            generated.files[1].text
        );
    }
}

#[test]
fn the_attributes_of_the_xml_namespace_are_referred_to() {
    let docs = ["<r xml:lang=\"en\"><a xml:space=\"preserve\"> x </a></r>"];
    let generated = generate(&docs, xsd(Layout::RussianDoll));
    let text = &generated.files[0].text;
    assert!(
        text.contains("<xs:import namespace=\"http://www.w3.org/XML/1998/namespace\"") && text.contains("<xs:attribute ref=\"xml:lang\""),
        "{text}"
    );
    assert_eq!(xsd_problems(&generated, "schema.xsd", docs[0]), Vec::<String>::new(), "{text}");
}

#[test]
fn schema_instance_attributes_are_left_out_of_an_xsd_and_declared_in_a_dtd() {
    let docs = ["<r xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:noNamespaceSchemaLocation=\"r.xsd\"/>"];
    let text = &generate(&docs, xsd(Layout::RussianDoll)).files[0].text;
    assert!(!text.contains("xsi") && !text.contains("noNamespaceSchemaLocation"), "{text}");
    let dtd_text = &generate(&docs, dtd()).files[0].text;
    assert!(dtd_text.contains("xmlns:xsi CDATA") && dtd_text.contains("xsi:noNamespaceSchemaLocation CDATA"), "{dtd_text}");
    assert_all_validate(&docs);
}

#[test]
fn a_dtd_for_a_document_with_prefixes_uses_the_names_as_written() {
    let docs = ["<p:r xmlns:p=\"urn:p\"><p:a/></p:r>", "<p:r xmlns:p=\"urn:p\"/>"];
    let dtd_text = &generate(&docs, dtd()).files[0].text;
    assert!(dtd_text.contains("<!ELEMENT p:r (p:a?)>") && dtd_text.contains("xmlns:p CDATA #REQUIRED"), "{dtd_text}");
    assert_all_validate(&docs);
}

#[test]
fn xml_space_is_declared_the_way_a_valid_document_must() {
    let dtd_text = &generate(&["<r xml:space=\"preserve\"/>"], dtd()).files[0].text;
    assert!(dtd_text.contains("xml:space (default|preserve) #REQUIRED"), "{dtd_text}");
}

#[test]
fn a_dtd_lists_the_roots_first() {
    let dtd_text = &generate(&["<z><a/></z>"], dtd()).files[0].text;
    assert!(dtd_text.starts_with("<!ELEMENT z (a)>"), "{dtd_text}");
    assert!(dtd_text.contains("<!ELEMENT a EMPTY>"));
}

#[test]
fn what_cannot_be_read_is_said() {
    assert_eq!(infer(&[], &Options::default()), Err(InferError::NoDocuments));
    let err = infer(&["<a/>", "<a><b></a>"], &Options::default()).expect_err("the second is broken");
    assert!(matches!(err, InferError::NotWellFormed { document: 1, .. }), "{err:?}");
    assert!(err.to_string().starts_with("Document 2 is not well-formed"), "{err}");
    assert!(matches!(infer(&[""], &Options::default()), Err(InferError::NotWellFormed { .. } | InferError::NoRoot)));
    let unbound = infer(&["<p:a/>"], &Options::default()).expect_err("the prefix is not declared");
    assert!(matches!(unbound, InferError::NotWellFormed { .. }), "{unbound:?}");
}

#[test]
fn more_child_names_than_an_order_is_learned_for_are_a_free_choice() {
    let children: String = (0..80).map(|i| format!("<c{i}/>")).collect();
    let doc = format!("<r>{children}</r>");
    assert_all_validate(&[&doc]);
    let dtd_text = &generate(&[&doc], dtd()).files[0].text;
    assert!(dtd_text.contains("<!ELEMENT r ((c0|c1|"), "{dtd_text}");
}

mod properties {
    use proptest::prelude::*;

    use super::*;

    #[derive(Clone, Debug)]
    enum Node {
        Element(El),
        Text(&'static str),
    }

    #[derive(Clone, Debug)]
    struct El {
        name: &'static str,
        attributes: Vec<(&'static str, &'static str)>,
        children: Vec<Node>,
    }

    fn element(depth: u32) -> BoxedStrategy<El> {
        let name = prop::sample::select(vec!["a", "b", "c", "d"]);
        let attributes = prop::collection::btree_map(
            prop::sample::select(vec!["x", "y", "z"]),
            prop::sample::select(vec!["1", "2", "x", "true", "2024-01-02", "", "a b", "1.5"]),
            0..3,
        )
        .prop_map(|m| m.into_iter().collect::<Vec<_>>());
        let text = prop::sample::select(vec!["t", "42", "", " ", "2024-01-02", "a &amp; b"]);
        let children: BoxedStrategy<Vec<Node>> = if depth == 0 {
            prop::collection::vec(text.clone().prop_map(Node::Text), 0..2).boxed()
        } else {
            prop::collection::vec(prop_oneof![3 => element(depth - 1).prop_map(Node::Element), 1 => text.prop_map(Node::Text)], 0..5).boxed()
        };
        (name, attributes, children).prop_map(|(name, attributes, children)| El { name, attributes, children }).boxed()
    }

    fn write(el: &El, out: &mut String) {
        out.push('<');
        out.push_str(el.name);
        for (k, v) in &el.attributes {
            out.push_str(&format!(" {k}=\"{v}\""));
        }
        out.push('>');
        for child in &el.children {
            match child {
                Node::Element(e) => write(e, out),
                Node::Text(t) => out.push_str(t),
            }
        }
        out.push_str(&format!("</{}>", el.name));
    }

    fn document() -> impl Strategy<Value = String> {
        element(3).prop_map(|e| {
            let mut s = String::new();
            write(&e, &mut s);
            s
        })
    }

    fn option_sets() -> Vec<Options> {
        let aggressive = Some(EnumerationRule { max_values: 6, min_occurrences: 1 });
        vec![
            Options::default(),
            Options { detect_types: false, enumerations: None, unbounded: false, ..Options::default() },
            Options { enumerations: aggressive, ..Options::default() },
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 96, ..ProptestConfig::default() })]

        /// A schema inferred from some documents validates every one of them.
        #[test]
        fn an_inferred_xsd_validates_its_sources(docs in prop::collection::vec(document(), 1..4)) {
            let refs: Vec<&str> = docs.iter().map(String::as_str).collect();
            for base in option_sets() {
                for layout in LAYOUTS {
                    let generated = infer(&refs, &Options { output: Output::Xsd { layout }, ..base }).expect("well-formed documents");
                    for doc in &refs {
                        let problems = xsd_problems(&generated, "schema.xsd", doc);
                        prop_assert!(problems.is_empty(), "{:?} {:?}: {:?}\n{}\n{}", layout, base, problems, generated.files[0].text, doc);
                    }
                }
            }
        }

        /// The same for the DTD.
        #[test]
        fn an_inferred_dtd_validates_its_sources(docs in prop::collection::vec(document(), 1..4)) {
            let refs: Vec<&str> = docs.iter().map(String::as_str).collect();
            for base in option_sets() {
                let generated = infer(&refs, &Options { output: Output::Dtd, ..base }).expect("well-formed documents");
                for doc in &refs {
                    let problems = dtd_problems(&generated, doc);
                    prop_assert!(problems.is_empty(), "{:?}: {:?}\n{}\n{}", base, problems, generated.files[0].text, doc);
                }
            }
        }

        /// Whatever the input, inference gives an answer and a schema that is well-formed XML.
        #[test]
        fn what_is_written_is_well_formed(docs in prop::collection::vec(document(), 1..3)) {
            let refs: Vec<&str> = docs.iter().map(String::as_str).collect();
            for layout in LAYOUTS {
                let generated = infer(&refs, &xsd(layout)).expect("well-formed documents");
                for file in &generated.files {
                    let parsed = parse(&file.text);
                    prop_assert!(parsed.diagnostics().iter().all(|d| d.severity() != Severity::Error), "{}", file.text);
                }
            }
        }
    }
}
