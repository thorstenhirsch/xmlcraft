//! Documents generated from schemas and DTDs, and that each one is valid.

use xmlcraft_dtd::{NoLoader as NoSubsets, read_doctype};
use xmlcraft_sample::{ChoicePolicy, Fill, Options, QName, SampleError, Schema, dtd_roots, sample_from_dtd, sample_from_schema, schema_roots};
use xmlcraft_syntax::{Severity, parse};
use xmlcraft_validate::validate_against_schema;
use xmlcraft_xsd::{LoadRequest, Loaded, NoLoader, SchemaLoader, assemble, load};

const XS: &str = "http://www.w3.org/2001/XMLSchema";

fn schema_of(body: &str) -> String {
    format!("<xs:schema xmlns:xs=\"{XS}\">{body}</xs:schema>")
}

/// An assembled schema; the test fails if the schema has a problem.
fn assembled(text: &str) -> Schema {
    let documents = load(text, "mem:s.xsd", &mut NoLoader);
    let assembly = assemble(&documents);
    let problems: Vec<String> = documents.diagnostics.iter().chain(&assembly.diagnostics).map(|d| d.diagnostic.message()).collect();
    assert!(problems.is_empty(), "{problems:?}");
    assembly.schema
}

/// What is wrong with `document` as an instance of `schema`.
fn problems(schema: &Schema, document: &str) -> Vec<String> {
    let parsed = parse(document);
    let mut out: Vec<String> = parsed.diagnostics().iter().filter(|d| d.severity() == Severity::Error).map(|d| d.message()).collect();
    out.extend(validate_against_schema(&parsed.syntax(), schema).iter().filter(|d| d.severity() == Severity::Error).map(|d| d.message()));
    out
}

fn sample(schema: &Schema, root: Option<&str>, options: &Options) -> String {
    let root = root.map(|r| QName::new(None, r));
    let sample = sample_from_schema(schema, root.as_ref(), options).unwrap_or_else(|e| panic!("{e}"));
    assert!(sample.warnings.is_empty(), "{:?}\n{}", sample.warnings, sample.text);
    sample.text
}

/// The sample is valid, and is returned.
fn valid_sample(schema: &Schema, root: Option<&str>, options: &Options) -> String {
    let text = sample(schema, root, options);
    let problems = problems(schema, &text);
    assert!(problems.is_empty(), "{problems:?}\n{text}");
    text
}

#[test]
fn a_simple_schema_gives_the_document_it_describes() {
    let schema = assembled(&schema_of(
        "<xs:element name=\"order\"><xs:complexType><xs:sequence><xs:element name=\"id\" type=\"xs:int\"/><xs:element name=\"item\" type=\"xs:string\" maxOccurs=\"unbounded\"/></xs:sequence><xs:attribute name=\"currency\" type=\"xs:string\" use=\"required\"/></xs:complexType></xs:element>",
    ));
    let text = valid_sample(&schema, None, &Options::default());
    // The attribute is made first, so the element's text is the second string.
    assert_eq!(text, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<order currency=\"string\">\n  <id>1</id>\n  <item>string2</item>\n</order>\n");
}

#[test]
fn optional_content_is_made_or_left_out() {
    let schema = assembled(&schema_of(
        "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"must\" type=\"xs:string\"/><xs:element name=\"may\" type=\"xs:string\" minOccurs=\"0\"/></xs:sequence><xs:attribute name=\"a\" type=\"xs:string\"/><xs:attribute name=\"b\" type=\"xs:string\" use=\"required\"/></xs:complexType></xs:element>",
    ));
    let full = valid_sample(&schema, None, &Options::default());
    assert!(full.contains("<may>") && full.contains(" a=\"") && full.contains(" b=\""), "{full}");
    let least = valid_sample(&schema, None, &Options { optional: false, ..Options::default() });
    assert!(!least.contains("<may") && !least.contains(" a=\"") && least.contains(" b=\"") && least.contains("<must>"), "{least}");
}

#[test]
fn a_particle_occurs_as_often_as_asked_within_its_bounds() {
    let schema = assembled(&schema_of(
        "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"a\" type=\"xs:string\" minOccurs=\"0\" maxOccurs=\"unbounded\"/><xs:element name=\"b\" type=\"xs:string\" maxOccurs=\"2\"/><xs:element name=\"c\" type=\"xs:string\" minOccurs=\"3\" maxOccurs=\"5\"/></xs:sequence></xs:complexType></xs:element>",
    ));
    let count = |text: &str, name: &str| text.matches(&format!("<{name}>")).count();
    let one = valid_sample(&schema, None, &Options { repeat: 1, ..Options::default() });
    assert_eq!((count(&one, "a"), count(&one, "b"), count(&one, "c")), (1, 1, 3), "{one}");
    let four = valid_sample(&schema, None, &Options { repeat: 4, ..Options::default() });
    assert_eq!((count(&four, "a"), count(&four, "b"), count(&four, "c")), (4, 2, 4), "{four}");
    let none = valid_sample(&schema, None, &Options { repeat: 0, ..Options::default() });
    assert_eq!((count(&none, "a"), count(&none, "b"), count(&none, "c")), (0, 1, 3), "{none}");
}

#[test]
fn a_choice_is_its_first_branch_or_each_in_turn() {
    let schema = assembled(&schema_of(
        "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:choice minOccurs=\"3\" maxOccurs=\"3\"><xs:element name=\"a\" type=\"xs:string\"/><xs:element name=\"b\" type=\"xs:string\"/><xs:element name=\"c\" type=\"xs:string\"/></xs:choice></xs:sequence></xs:complexType></xs:element>",
    ));
    let first = valid_sample(&schema, None, &Options::default());
    assert_eq!(first.matches("<a>").count(), 3, "{first}");
    let rotating = valid_sample(&schema, None, &Options { choice: ChoicePolicy::Rotate, ..Options::default() });
    assert!(rotating.contains("<a>") && rotating.contains("<b>") && rotating.contains("<c>"), "{rotating}");
}

#[test]
fn fixed_and_default_values_are_used_and_names_can_fill_the_text() {
    let schema = assembled(&schema_of(
        "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"v\" type=\"xs:string\" fixed=\"always\"/><xs:element name=\"d\" type=\"xs:int\" default=\"42\"/><xs:element name=\"title\" type=\"xs:string\"/></xs:sequence><xs:attribute name=\"k\" type=\"xs:string\" fixed=\"x\"/><xs:attribute name=\"label\" type=\"xs:string\"/></xs:complexType></xs:element>",
    ));
    let text = valid_sample(&schema, None, &Options { fill: Fill::Named, ..Options::default() });
    assert!(
        text.contains("<v>always</v>")
            && text.contains("<d>42</d>")
            && text.contains("<title>title</title>")
            && text.contains(" k=\"x\"")
            && text.contains(" label=\"label\""),
        "{text}"
    );
}

#[test]
fn values_follow_their_types() {
    let schema = assembled(&schema_of(
        "<xs:simpleType name=\"colour\"><xs:restriction base=\"xs:string\"><xs:enumeration value=\"red\"/><xs:enumeration value=\"green\"/></xs:restriction></xs:simpleType>\
         <xs:simpleType name=\"code\"><xs:restriction base=\"xs:string\"><xs:pattern value=\"[A-Z]{2}[0-9]{3}\"/></xs:restriction></xs:simpleType>\
         <xs:simpleType name=\"pct\"><xs:restriction base=\"xs:integer\"><xs:minInclusive value=\"10\"/><xs:maxInclusive value=\"20\"/></xs:restriction></xs:simpleType>\
         <xs:simpleType name=\"ints\"><xs:list itemType=\"xs:int\"/></xs:simpleType>\
         <xs:simpleType name=\"either\"><xs:union memberTypes=\"xs:date xs:boolean\"/></xs:simpleType>\
         <xs:element name=\"r\"><xs:complexType><xs:sequence>\
           <xs:element name=\"c\" type=\"colour\" maxOccurs=\"2\"/><xs:element name=\"code\" type=\"code\"/><xs:element name=\"p\" type=\"pct\"/>\
           <xs:element name=\"l\" type=\"ints\"/><xs:element name=\"e\" type=\"either\"/><xs:element name=\"d\" type=\"xs:dateTime\"/><xs:element name=\"u\" type=\"xs:anyURI\"/>\
           <xs:element name=\"h\" type=\"xs:hexBinary\"/><xs:element name=\"b\" type=\"xs:base64Binary\"/><xs:element name=\"dec\" type=\"xs:decimal\"/><xs:element name=\"du\" type=\"xs:duration\"/>\
         </xs:sequence></xs:complexType></xs:element>",
    ));
    let text = valid_sample(&schema, None, &Options { repeat: 2, ..Options::default() });
    assert!(text.contains("<c>red</c>") && text.contains("<c>green</c>"), "an enumeration is walked through: {text}");
}

#[test]
fn a_recursive_schema_is_cut_at_the_depth_and_still_valid() {
    let schema = assembled(&schema_of(
        "<xs:element name=\"node\"><xs:complexType><xs:sequence><xs:element ref=\"node\" minOccurs=\"0\" maxOccurs=\"2\"/></xs:sequence><xs:attribute name=\"n\" type=\"xs:int\"/></xs:complexType></xs:element>",
    ));
    for max_depth in [0usize, 1, 3, 5] {
        let text = valid_sample(&schema, None, &Options { repeat: 2, max_depth, ..Options::default() });
        let depth = text.lines().filter(|l| l.contains("<node")).map(|l| (l.len() - l.trim_start().len()) / 2).max().unwrap_or(0);
        assert!(depth <= max_depth, "{max_depth}: {depth}\n{text}");
    }
}

#[test]
fn a_required_branch_that_ends_the_recursion_is_the_one_taken_at_the_depth() {
    // `a` needs an `a` or a `leaf`: only the second ends.
    let schema = assembled(&schema_of(
        "<xs:element name=\"a\"><xs:complexType><xs:choice><xs:element ref=\"a\"/><xs:element name=\"leaf\" type=\"xs:string\"/></xs:choice></xs:complexType></xs:element>",
    ));
    let text = valid_sample(&schema, None, &Options { max_depth: 3, ..Options::default() });
    assert_eq!(text.matches("<a>").count() + text.matches("<a/>").count() + text.matches("<a ").count(), 4, "{text}");
}

#[test]
fn an_element_that_requires_itself_cannot_be_made() {
    let schema = assembled(&schema_of(
        "<xs:element name=\"a\"><xs:complexType><xs:sequence><xs:element ref=\"a\"/></xs:sequence></xs:complexType></xs:element>",
    ));
    assert!(matches!(sample_from_schema(&schema, None, &Options::default()), Err(SampleError::NoRoot)));
    let named = QName::new(None, "a");
    assert!(matches!(sample_from_schema(&schema, Some(&named), &Options::default()), Err(SampleError::Unsatisfiable(_))));
    let unknown = QName::new(None, "zzz");
    assert!(matches!(sample_from_schema(&schema, Some(&unknown), &Options::default()), Err(SampleError::UnknownRoot(_))));
}

#[test]
fn namespaces_get_prefixes_declared_on_the_root() {
    let schema = assembled(&format!(
        "<xs:schema xmlns:xs=\"{XS}\" xmlns:t=\"urn:t\" targetNamespace=\"urn:t\" elementFormDefault=\"qualified\"><xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"a\" type=\"xs:string\"/></xs:sequence><xs:attribute name=\"x\" type=\"xs:int\"/></xs:complexType></xs:element></xs:schema>"
    ));
    let root = QName::new(Some("urn:t"), "r");
    let text = sample_from_schema(&schema, Some(&root), &Options::default()).expect("a sample").text;
    assert!(text.contains("<ns1:r xmlns:ns1=\"urn:t\"") && text.contains("<ns1:a>string</ns1:a>") && text.contains(" x=\"1\""), "{text}");
    assert_eq!(problems(&schema, &text), Vec::<String>::new(), "{text}");
    assert_eq!(schema_roots(&schema), vec![root]);
}

#[test]
fn an_abstract_head_is_replaced_by_a_member_of_its_group_and_an_abstract_type_by_a_derived_one() {
    let schema = assembled(&schema_of(
        "<xs:element name=\"shape\" type=\"xs:string\" abstract=\"true\"/>\
         <xs:element name=\"circle\" type=\"xs:string\" substitutionGroup=\"shape\"/>\
         <xs:complexType name=\"vehicle\" abstract=\"true\"><xs:sequence><xs:element name=\"wheels\" type=\"xs:int\"/></xs:sequence></xs:complexType>\
         <xs:complexType name=\"car\"><xs:complexContent><xs:extension base=\"vehicle\"><xs:sequence><xs:element name=\"doors\" type=\"xs:int\"/></xs:sequence></xs:extension></xs:complexContent></xs:complexType>\
         <xs:element name=\"garage\"><xs:complexType><xs:sequence><xs:element ref=\"shape\"/><xs:element name=\"v\" type=\"vehicle\"/></xs:sequence></xs:complexType></xs:element>",
    ));
    let text = valid_sample(&schema, Some("garage"), &Options::default());
    assert!(text.contains("<circle>") && !text.contains("<shape>"), "{text}");
    assert!(text.contains("xsi:type=\"car\"") && text.contains("<doors>"), "{text}");
    let roots: Vec<String> = schema_roots(&schema).iter().map(|q| q.local.clone()).collect();
    assert!(!roots.contains(&"shape".to_owned()) && roots.contains(&"circle".to_owned()), "an abstract element is no root: {roots:?}");
}

#[test]
fn ids_and_their_references_agree() {
    let schema = assembled(&schema_of(
        "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"i\" maxOccurs=\"3\"><xs:complexType><xs:attribute name=\"id\" type=\"xs:ID\" use=\"required\"/><xs:attribute name=\"ref\" type=\"xs:IDREF\"/></xs:complexType></xs:element></xs:sequence></xs:complexType></xs:element>",
    ));
    let text = valid_sample(&schema, None, &Options { repeat: 3, ..Options::default() });
    assert!(text.contains("id=\"id1\"") && text.contains("id=\"id2\"") && text.contains("ref=\"id1\""), "{text}");
}

#[test]
fn a_reference_with_no_id_is_a_warning() {
    let schema = assembled(&schema_of(
        "<xs:element name=\"r\"><xs:complexType><xs:attribute name=\"ref\" type=\"xs:IDREF\" use=\"required\"/></xs:complexType></xs:element>",
    ));
    let made = sample_from_schema(&schema, None, &Options::default()).expect("a sample");
    assert!(made.warnings.iter().any(|w| w.contains("ID")), "{:?}", made.warnings);
}

#[test]
fn a_value_that_cannot_be_found_is_a_warning_not_a_failure() {
    let schema = assembled(&schema_of(
        "<xs:simpleType name=\"never\"><xs:restriction base=\"xs:string\"><xs:pattern value=\"[a-z]{3}\"/><xs:maxLength value=\"1\"/></xs:restriction></xs:simpleType><xs:element name=\"r\" type=\"never\"/>",
    ));
    let made = sample_from_schema(&schema, None, &Options::default()).expect("a sample");
    assert!(made.warnings.iter().any(|w| w.contains("'r'")), "{:?}", made.warnings);
}

#[test]
fn the_schema_location_is_said_in_the_document() {
    let schema = assembled(&schema_of("<xs:element name=\"r\" type=\"xs:string\"/>"));
    let text = sample(&schema, None, &Options { schema_location: Some("r.xsd".into()), ..Options::default() });
    assert!(text.contains("xsi:noNamespaceSchemaLocation=\"r.xsd\"") && text.contains("xmlns:xsi="), "{text}");
}

#[test]
fn a_wildcard_is_not_filled_with_the_element_it_is_in_and_takes_the_shallowest_other() {
    let any = "<xs:any processContents=\"lax\" namespace=\"##local\"/>";
    // Only the root and one deeper element: the wildcard cannot be the root again.
    let schema = assembled(&schema_of(&format!(
        "<xs:element name=\"r\"><xs:complexType><xs:sequence>{any}</xs:sequence></xs:complexType></xs:element>\
         <xs:element name=\"deep\"><xs:complexType><xs:sequence><xs:element name=\"inner\"><xs:complexType><xs:sequence><xs:element name=\"leaf\" type=\"xs:string\"/></xs:sequence></xs:complexType></xs:element></xs:sequence></xs:complexType></xs:element>"
    )));
    let text = valid_sample(&schema, Some("r"), &Options::default());
    assert!(text.contains("<deep>") && text.matches("<r").count() == 1, "{text}");
    // With a simple element to choose, that is the one, though it is declared after the deeper one.
    let schema = assembled(&schema_of(&format!(
        "<xs:element name=\"r\"><xs:complexType><xs:sequence>{any}</xs:sequence></xs:complexType></xs:element>\
         <xs:element name=\"deep\"><xs:complexType><xs:sequence><xs:element name=\"inner\"><xs:complexType><xs:sequence><xs:element name=\"leaf\" type=\"xs:string\"/></xs:sequence></xs:complexType></xs:element></xs:sequence></xs:complexType></xs:element>\
         <xs:element name=\"simple\" type=\"xs:string\"/>"
    )));
    let text = valid_sample(&schema, Some("r"), &Options::default());
    assert!(text.contains("<simple>") && !text.contains("<deep>"), "{text}");
}

#[test]
fn a_wildcard_is_filled_with_a_declared_element() {
    let schema = assembled(&schema_of(
        "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:any processContents=\"lax\" namespace=\"##local\"/></xs:sequence></xs:complexType></xs:element><xs:element name=\"other\" type=\"xs:string\"/>",
    ));
    let text = valid_sample(&schema, Some("r"), &Options::default());
    assert!(text.contains("<r>") && (text.contains("<other>") || text.contains("<r>\n  <r")), "{text}");
}

#[test]
fn an_element_declared_in_a_named_group_is_not_a_root_and_not_what_a_wildcard_is_filled_with() {
    // `inner` is declared inside the group, so it belongs to the types that use it; it is not a global element.
    let schema = assembled(&schema_of(
        "<xs:group name=\"g\"><xs:sequence><xs:element name=\"inner\" type=\"xs:string\"/></xs:sequence></xs:group>\
         <xs:element name=\"outer\"><xs:complexType><xs:sequence><xs:group ref=\"g\"/><xs:any processContents=\"lax\" namespace=\"##local\" minOccurs=\"0\"/></xs:sequence></xs:complexType></xs:element>",
    ));
    let names: Vec<String> = schema_roots(&schema).iter().map(|q| q.local.clone()).collect();
    assert_eq!(names, ["outer"]);
    assert!(sample_from_schema(&schema, Some(&QName::new(None, "inner")), &Options::default()).is_err(), "it is not a root");
    // With no root asked for, the first element that can be made is `outer`, and the wildcard has nothing declared to take.
    let text = valid_sample(&schema, None, &Options::default());
    assert!(text.contains("<outer>") && text.matches("<inner>").count() == 1, "{text}");
}

#[test]
fn an_element_in_a_group_that_is_named_like_a_global_one_is_not_that_global_element() {
    let schema = assembled(&schema_of(
        "<xs:group name=\"g\"><xs:sequence><xs:element name=\"r\" type=\"xs:int\"/></xs:sequence></xs:group>\
         <xs:element name=\"r\"><xs:complexType><xs:sequence><xs:group ref=\"g\"/></xs:sequence></xs:complexType></xs:element>",
    ));
    assert_eq!(schema_roots(&schema).len(), 1, "one root, not two");
    let text = valid_sample(&schema, None, &Options::default());
    assert!(text.contains("<r>\n  <r>1</r>"), "{text}");
}

#[test]
fn a_wildcard_with_nothing_declared_is_left_out_when_it_may_be_and_given_a_placeholder_when_it_may_not() {
    let root = |any: &str| {
        assembled(&schema_of(&format!(
            "<xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"a\" type=\"xs:string\"/>{any}</xs:sequence></xs:complexType></xs:element>"
        )))
    };
    // May be left out: no element, no warning.
    let optional = root("<xs:any namespace=\"##other\" processContents=\"lax\" minOccurs=\"0\"/>");
    let text = valid_sample(&optional, None, &Options::default());
    assert!(!text.contains("extension"), "{text}");
    // Required, and lax or skip: an element of no declaration, in a namespace the wildcard allows, which is valid since nothing checks it.
    for (constraint, namespace) in [
        ("##other", Some("urn:example:extension")),
        ("##any", Some("urn:example:extension")),
        ("urn:listed ##local", Some("urn:listed")),
        ("##local", None),
    ] {
        for process in ["lax", "skip"] {
            let schema = root(&format!("<xs:any namespace=\"{constraint}\" processContents=\"{process}\"/>"));
            let text = valid_sample(&schema, None, &Options::default());
            match namespace {
                Some(ns) => assert!(text.contains(ns) && text.contains("extension"), "{constraint} {process}: {text}"),
                None => assert!(text.contains("<extension/>"), "{constraint} {process}: {text}"),
            }
        }
    }
    // Several required: one each.
    let two = root("<xs:any namespace=\"##other\" processContents=\"lax\" minOccurs=\"2\" maxOccurs=\"2\"/>");
    assert_eq!(valid_sample(&two, None, &Options::default()).matches("extension/>").count(), 2);
    // Strict checks what it lets in, and nothing is declared: it cannot be satisfied, and says so.
    let strict = root("<xs:any namespace=\"##other\" processContents=\"strict\"/>");
    let made = sample_from_schema(&strict, None, &Options::default()).expect("a document is made");
    assert!(made.warnings.iter().any(|w| w.contains("wildcard")), "{:?}", made.warnings);
    // ...but not when it may be left out.
    let strict_optional = root("<xs:any namespace=\"##other\" processContents=\"strict\" minOccurs=\"0\"/>");
    assert!(sample_from_schema(&strict_optional, None, &Options::default()).expect("made").warnings.is_empty());
}

/// The schemas that ship with the app, and the loader that finds the ones they include and import among them.
struct Bundle;

impl SchemaLoader for Bundle {
    fn load(&mut self, request: &LoadRequest<'_>) -> Result<Loaded, String> {
        let wanted = request.location.unwrap_or_default();
        let file = wanted.rsplit('/').next().unwrap_or(wanted);
        let path = xmlcraft_schemas::paths().find(|p| p.ends_with(&format!("/{file}"))).ok_or_else(|| format!("{wanted} is not in the bundle"))?;
        Ok(Loaded { text: xmlcraft_schemas::get_text(path).ok_or("no text")?, location: xmlcraft_schemas::uri_of(path) })
    }
}

/// Files of the bundle that are parts of another schema, which includes them and so gets every element of them tested; on their own they
/// refer to elements their siblings declare.
const PARTS: &[&str] = &[
    "w3c/mathml3/mathml3-common.xsd",
    "w3c/mathml3/mathml3-content.xsd",
    "w3c/mathml3/mathml3-presentation.xsd",
    "w3c/mathml3/mathml3-strict-content.xsd",
];

/// Elements of the bundle that no sample is made of, and why: a value this generator cannot write. The test fails when one of them
/// starts to work, so that the list does not outlive its reasons.
const EXCEPTIONS: &[(&str, Option<&str>, &str)] = &[(
    "w3c/soap12/soap-envelope.xsd",
    Some("Fault"),
    "its code is an enumeration of QNames (`env:Receiver`): the prefix would have to be declared where the value is written",
)];

fn excepted(path: &str, root: &str) -> bool {
    EXCEPTIONS.iter().any(|(file, element, _)| path.ends_with(file) && element.is_none_or(|e| e == root))
}

#[test]
fn most_of_the_global_elements_of_the_bundled_schemas_give_a_valid_document() {
    let (mut made, mut valid) = (0usize, 0usize);
    let mut failures: Vec<String> = Vec::new();
    let mut excepted_that_work: Vec<String> = Vec::new();
    for path in xmlcraft_schemas::paths().filter(|p| p.ends_with(".xsd") && !PARTS.iter().any(|part| p.ends_with(part))) {
        let text = xmlcraft_schemas::get_text(path).expect("text");
        let documents = load(&text, &xmlcraft_schemas::uri_of(path), &mut Bundle);
        let assembly = assemble(&documents);
        for root in schema_roots(&assembly.schema) {
            let works = match sample_from_schema(&assembly.schema, Some(&root), &Options::default()) {
                Ok(sample) => {
                    let found = problems(&assembly.schema, &sample.text);
                    if found.is_empty() && sample.warnings.is_empty() {
                        Ok(())
                    } else {
                        Err(format!("{path} {root}: {:?} {:?}", found.first(), sample.warnings.first()))
                    }
                }
                Err(e) => Err(format!("{path} {root}: {e}")),
            };
            if excepted(path, &root.local) {
                if works.is_ok() {
                    excepted_that_work.push(format!("{path} {root}"));
                }
                continue;
            }
            made += 1;
            match works {
                Ok(()) => valid += 1,
                Err(why) => failures.push(why),
            }
        }
    }
    eprintln!("bundled global elements: {valid} of {made} give a valid document without a warning\n{}", failures.join("\n"));
    assert!(made > 100, "the bundle has schemas with elements: {made}");
    // The bar of the plan is 95%; every global element of the bundle gives one today, and none is allowed to stop. An exception that has
    // to be accepted goes on the list above, with why, and not into a lower number.
    assert_eq!(valid, made, "{valid} of {made}:\n{}", failures.join("\n"));
    assert!(excepted_that_work.is_empty(), "these work now, so they are no exceptions: {excepted_that_work:?}");
}

// --- DTD ---------------------------------------------------------------------------------------------------------------------

/// The DTD in `declarations` as the DTD of a document, and the document that `text` is (which starts with the XML declaration).
fn dtd_of(declarations: &str) -> xmlcraft_dtd::Dtd {
    let text = format!("<!DOCTYPE r [\n{declarations}\n]><r/>");
    let parsed = parse(&text);
    let (read, _) = read_doctype(&parsed.syntax(), &text, "", &mut NoSubsets).expect("a DOCTYPE");
    read.dtd
}

/// `sample` with a DOCTYPE that holds `declarations`, checked against them.
fn dtd_problems(declarations: &str, root: &str, sample: &str) -> Vec<String> {
    let at = sample.find("?>").map(|i| i + 3).unwrap_or(0);
    let text = format!("{}<!DOCTYPE {root} [\n{declarations}\n]>\n{}", &sample[..at], &sample[at..]);
    let parsed = parse(&text);
    let found = xmlcraft_validate::check(&parsed.syntax(), &text, parsed.diagnostics(), "", &mut NoSubsets);
    found
        .well_formedness
        .iter()
        .chain(&found.validity)
        .filter(|p| p.diagnostic.severity() == Severity::Error)
        .map(|p| p.diagnostic.message())
        .collect()
}

fn valid_dtd_sample(declarations: &str, root: Option<&str>, options: &Options) -> String {
    let dtd = dtd_of(declarations);
    let made = sample_from_dtd(&dtd, root, options).unwrap_or_else(|e| panic!("{e}"));
    let root_name = root.map(str::to_owned).unwrap_or_else(|| dtd_roots(&dtd).first().cloned().unwrap_or_default());
    let found = dtd_problems(declarations, &root_name, &made.text);
    assert!(found.is_empty() && made.warnings.is_empty(), "{found:?} {:?}\n{}", made.warnings, made.text);
    made.text
}

const BOOKS: &str = "<!ELEMENT library (book+)>\n<!ELEMENT book (title, author+, (isbn | issn)?, note*)>\n<!ATTLIST book id ID #REQUIRED lang (en|de|fr) 'en' status CDATA #IMPLIED kind (a|b) #REQUIRED>\n<!ELEMENT title (#PCDATA)>\n<!ELEMENT author (#PCDATA)>\n<!ELEMENT isbn (#PCDATA)>\n<!ELEMENT issn (#PCDATA)>\n<!ELEMENT note (#PCDATA | em)*>\n<!ELEMENT em EMPTY>";

#[test]
fn a_dtd_gives_a_valid_document() {
    let text = valid_dtd_sample(BOOKS, None, &Options::default());
    assert!(text.contains("<library>") && text.contains("<book ") && text.contains("<title>text</title>"), "{text}");
    let least = valid_dtd_sample(BOOKS, None, &Options { optional: false, ..Options::default() });
    assert!(!least.contains("<note") && !least.contains("status="), "{least}");
    let rotating = valid_dtd_sample(BOOKS, None, &Options { choice: ChoicePolicy::Rotate, repeat: 3, ..Options::default() });
    assert_eq!(rotating.matches("<book ").count(), 3, "{rotating}");
    assert!(rotating.contains("kind=\"a\"") && rotating.contains("kind=\"b\""), "an enumerated attribute takes its values in turn: {rotating}");
    assert!(text.contains("<em/>"), "mixed content has its elements: {text}");
    let named = valid_dtd_sample(BOOKS, None, &Options { fill: Fill::Named, ..Options::default() });
    assert!(named.contains("<title>title</title>") && named.contains("status=\"status\""), "{named}");
}

#[test]
fn a_dtd_with_every_attribute_type_gives_a_valid_document() {
    let declarations = "<!ELEMENT r (a+)>\n<!ELEMENT a EMPTY>\n<!ATTLIST a i ID #REQUIRED r IDREF #IMPLIED rs IDREFS #IMPLIED t NMTOKEN #IMPLIED ts NMTOKENS #IMPLIED f CDATA #FIXED 'fixed &amp; sure' e (x|y|z) #IMPLIED>";
    let text = valid_dtd_sample(declarations, None, &Options { repeat: 3, choice: ChoicePolicy::Rotate, ..Options::default() });
    assert!(text.contains("f=\"fixed &amp; sure\"") && text.contains("i=\"id3\""), "{text}");
}

#[test]
fn a_recursive_dtd_is_cut_at_the_depth() {
    let declarations = "<!ELEMENT section (title, section*)>\n<!ELEMENT title (#PCDATA)>";
    for max_depth in [1usize, 2, 4] {
        let text = valid_dtd_sample(declarations, Some("section"), &Options { repeat: 2, max_depth, ..Options::default() });
        let depth = text.lines().filter(|l| l.contains("<section")).map(|l| (l.len() - l.trim_start().len()) / 2).max().unwrap_or(0);
        assert!(depth <= max_depth, "{max_depth}: {depth}\n{text}");
    }
}

#[test]
fn a_dtd_element_that_requires_itself_cannot_be_made() {
    let dtd = dtd_of("<!ELEMENT a (a)>");
    assert_eq!(sample_from_dtd(&dtd, None, &Options::default()), Err(SampleError::NoRoot));
    assert!(matches!(sample_from_dtd(&dtd, Some("a"), &Options::default()), Err(SampleError::Unsatisfiable(_))));
    assert!(matches!(sample_from_dtd(&dtd, Some("b"), &Options::default()), Err(SampleError::UnknownRoot(_))));
}

#[test]
fn a_dtd_with_prefixed_names_declares_them_as_attributes_and_the_sample_is_valid() {
    let declarations = "<!ELEMENT p:r (p:a)>\n<!ATTLIST p:r xmlns:p CDATA #FIXED 'urn:p'>\n<!ELEMENT p:a EMPTY>";
    let text = valid_dtd_sample(declarations, Some("p:r"), &Options::default());
    assert!(text.contains("<p:r xmlns:p=\"urn:p\">") && text.contains("<p:a/>"), "{text}");
}

#[test]
fn the_roots_of_a_dtd_are_its_elements_in_order() {
    let dtd = dtd_of(BOOKS);
    assert_eq!(dtd_roots(&dtd)[..3], ["library", "book", "title"]);
}
