//! A document checked against the schema it names, or one it is told to use.

use xmlcraft_syntax::parse;
use xmlcraft_validate::{SchemaOutcome, check_against_schema, schema_hints};
use xmlcraft_xsd::{LoadRequest, Loaded, SchemaLoader, XSD_NAMESPACE};

/// Schema documents by location (the end of the path they are asked for with).
struct Map(Vec<(&'static str, String)>);

impl SchemaLoader for Map {
    fn load(&mut self, request: &LoadRequest<'_>) -> Result<Loaded, String> {
        let wanted = request.location.unwrap_or_default();
        self.0
            .iter()
            .find(|(l, _)| l.ends_with(wanted))
            .map(|(l, t)| Loaded { text: t.clone(), location: (*l).to_owned() })
            .ok_or_else(|| format!("{wanted}: no such file"))
    }
}

fn plain_schema() -> String {
    format!(
        "<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\"><xs:element name=\"r\"><xs:complexType><xs:sequence><xs:element name=\"n\" type=\"xs:int\"/></xs:sequence></xs:complexType></xs:element></xs:schema>"
    )
}

fn namespaced_schema() -> String {
    format!(
        "<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\" targetNamespace=\"urn:t\" elementFormDefault=\"qualified\"><xs:element name=\"r\" type=\"xs:int\"/></xs:schema>"
    )
}

const XSI: &str = "xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"";

fn check(xml: &str, explicit: Option<&str>, files: Vec<(&'static str, String)>) -> xmlcraft_validate::SchemaCheck {
    let parsed = parse(xml);
    check_against_schema(&parsed.syntax(), parsed.diagnostics(), "/d/doc.xml", explicit, &mut Map(files))
}

#[test]
fn the_schema_a_document_names_is_found_and_used() {
    let xml = format!("<r {XSI} xsi:noNamespaceSchemaLocation=\"r.xsd\"><n>7</n></r>");
    let found = check(&xml, None, vec![("/d/r.xsd", plain_schema())]);
    assert_eq!(found.outcome, SchemaOutcome::Validated);
    assert!(found.validity.is_empty() && found.schema_problems.is_empty(), "{found:?}");
    assert_eq!(found.sources, ["/d/r.xsd"]);
    let bad = format!("<r {XSI} xsi:noNamespaceSchemaLocation=\"r.xsd\"><n>seven</n></r>");
    let found = check(&bad, None, vec![("/d/r.xsd", plain_schema())]);
    assert_eq!(found.validity.iter().map(|d| d.code.key()).collect::<Vec<_>>(), ["cvc.invalid-value"]);
}

#[test]
fn schema_location_pairs_name_schemas_by_namespace() {
    let xml = format!("<r xmlns=\"urn:t\" {XSI} xsi:schemaLocation=\"urn:t t.xsd\">5</r>");
    let found = check(&xml, None, vec![("/d/t.xsd", namespaced_schema())]);
    assert_eq!(found.outcome, SchemaOutcome::Validated);
    assert!(found.validity.is_empty(), "{found:?}");
    let wrong = format!("<r xmlns=\"urn:t\" {XSI} xsi:schemaLocation=\"urn:t t.xsd\">five</r>");
    assert_eq!(check(&wrong, None, vec![("/d/t.xsd", namespaced_schema())]).validity.len(), 1);
}

#[test]
fn several_schemas_are_used_together() {
    let other = format!(
        "<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\" targetNamespace=\"urn:o\" elementFormDefault=\"qualified\"><xs:element name=\"o\" type=\"xs:boolean\"/></xs:schema>"
    );
    let xml = format!("<r xmlns=\"urn:t\" {XSI} xsi:schemaLocation=\"urn:t t.xsd urn:o o.xsd\">5</r>");
    let found = check(&xml, None, vec![("/d/t.xsd", namespaced_schema()), ("/d/o.xsd", other.clone())]);
    assert!(found.validity.is_empty() && found.schema_problems.is_empty(), "{found:?}");
    assert_eq!(found.sources.len(), 3, "the document that imports both, and the two: {:?}", found.sources);
    let o = format!("<o xmlns=\"urn:o\" {XSI} xsi:schemaLocation=\"urn:t t.xsd urn:o o.xsd\">maybe</o>");
    assert_eq!(check(&o, None, vec![("/d/t.xsd", namespaced_schema()), ("/d/o.xsd", other)]).validity.len(), 1);
}

#[test]
fn a_schema_can_be_given_and_wins_over_the_document() {
    let xml = format!("<r {XSI} xsi:noNamespaceSchemaLocation=\"wrong.xsd\"><n>1</n></r>");
    let found = check(&xml, Some("r.xsd"), vec![("/d/r.xsd", plain_schema())]);
    assert_eq!(found.outcome, SchemaOutcome::Validated);
    assert!(found.validity.is_empty(), "{found:?}");
}

#[test]
fn without_a_schema_or_with_one_that_is_missing_nothing_is_validated_and_it_is_said() {
    let found = check("<r><n>1</n></r>", None, vec![]);
    assert!(matches!(&found.outcome, SchemaOutcome::NoSchema(why) if why.contains("names no schema")), "{found:?}");
    assert!(found.validity.is_empty());
    let xml = format!("<r {XSI} xsi:noNamespaceSchemaLocation=\"gone.xsd\"><n>1</n></r>");
    let found = check(&xml, None, vec![]);
    assert!(matches!(&found.outcome, SchemaOutcome::NoSchema(why) if why.contains("gone.xsd") && why.contains("no such file")), "{found:?}");
}

#[test]
fn a_document_that_is_not_well_formed_is_not_validated() {
    let found = check("<r><n>1</r>", Some("r.xsd"), vec![("/d/r.xsd", plain_schema())]);
    assert_eq!(found.outcome, SchemaOutcome::NotWellFormed);
    assert!(!found.well_formedness.is_empty() && found.validity.is_empty());
}

#[test]
fn the_problems_of_the_schema_are_reported_with_the_document_they_are_in() {
    let schema = format!("<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\"><xs:element name=\"r\" type=\"nothing\"/></xs:schema>");
    let found = check("<r/>", Some("r.xsd"), vec![("/d/r.xsd", schema)]);
    assert_eq!(found.outcome, SchemaOutcome::Validated);
    assert_eq!(found.schema_problems.len(), 1);
    assert_eq!(found.sources[found.schema_problems[0].source], "/d/r.xsd");
    assert_eq!(found.schema_errors(), 1);
}

#[test]
fn the_hints_of_the_root_element() {
    let parsed = parse(&format!("<r {XSI} xsi:schemaLocation=\"urn:a a.xsd  urn:b\nb.xsd urn:odd\" xsi:noNamespaceSchemaLocation=\"c.xsd\"/>"));
    let hints = schema_hints(&parsed.syntax());
    let shown: Vec<(Option<&str>, &str)> = hints.iter().map(|h| (h.namespace.as_deref(), h.location.as_str())).collect();
    assert_eq!(shown, [(Some("urn:a"), "a.xsd"), (Some("urn:b"), "b.xsd"), (None, "c.xsd")], "an odd word is ignored");
    // Another prefix for the same namespace; not on the root, or in no namespace: no hint.
    let other =
        parse("<r xmlns:i=\"http://www.w3.org/2001/XMLSchema-instance\" i:noNamespaceSchemaLocation=\"d.xsd\"><c schemaLocation=\"x y\"/></r>");
    assert_eq!(schema_hints(&other.syntax()).len(), 1);
    let none = parse("<r schemaLocation=\"urn:a a.xsd\"/>");
    assert!(schema_hints(&none.syntax()).is_empty());
    assert!(schema_hints(&parse("").syntax()).is_empty());
}
