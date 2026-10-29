//! A document checked against the schema it names, or one it is told to use.

use xmlcraft_syntax::parse;
use xmlcraft_validate::{NoSchema, SchemaHint, SchemaOutcome, check_against_schema, load_hinted, load_schema, root_namespace, schema_hints};
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
    assert!(
        matches!(&found.outcome, SchemaOutcome::NoSchema(why) if why.starts_with("the schema 'gone.xsd' could not be loaded: ")),
        "it says which schema it was: {found:?}"
    );
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

/// Loads nothing, and remembers where each request was said to be from.
struct Bases(Vec<String>);

impl SchemaLoader for Bases {
    fn load(&mut self, request: &LoadRequest<'_>) -> Result<Loaded, String> {
        self.0.push(request.base.to_owned());
        Err("not here".to_owned())
    }
}

#[test]
fn a_schema_is_looked_for_from_the_place_of_the_document() {
    let parsed = parse("<r/>");
    let mut bases = Bases(Vec::new());
    check_against_schema(&parsed.syntax(), parsed.diagnostics(), "/d/doc.xml", Some("r.xsd"), &mut bases);
    assert_eq!(bases.0, ["/d/doc.xml"]);
    // Several are imported by a schema that is made for it, in the place of the document too.
    let parsed = parse(&format!("<r {XSI} xsi:schemaLocation=\"urn:a a.xsd urn:b b.xsd\"/>"));
    let mut bases = Bases(Vec::new());
    check_against_schema(&parsed.syntax(), parsed.diagnostics(), "/d/doc.xml", None, &mut bases);
    assert_eq!(bases.0, ["/d/doc.xml", "/d/doc.xml"]);
}

#[test]
fn locations_with_markup_characters_survive_the_schema_that_imports_several() {
    let xml = format!("<r {XSI} xsi:schemaLocation='urn:a a&amp;b.xsd urn:b c&lt;d.xsd urn:e f\"g.xsd'/>");
    let empty = |ns: &str| format!("<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\" targetNamespace=\"{ns}\"/>");
    let files = vec![("/d/a&b.xsd", empty("urn:a")), ("/d/c<d.xsd", empty("urn:b")), ("/d/f\"g.xsd", empty("urn:e"))];
    let found = check(&xml, None, files);
    assert!(found.schema_problems.is_empty(), "{:?}", found.schema_problems.iter().map(|p| p.diagnostic.message()).collect::<Vec<_>>());
    for read in ["/d/a&b.xsd", "/d/c<d.xsd", "/d/f\"g.xsd"] {
        assert!(found.sources.iter().any(|s| s == read), "{read} was read: {:?}", found.sources);
    }
}

#[test]
fn a_document_a_schema_cannot_load_is_a_problem_of_the_schema() {
    let schema = format!(
        "<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\"><xs:include schemaLocation=\"gone.xsd\"/><xs:element name=\"r\" type=\"xs:string\"/></xs:schema>"
    );
    let found = check("<r>x</r>", Some("r.xsd"), vec![("/d/r.xsd", schema)]);
    assert_eq!(found.outcome, SchemaOutcome::Validated);
    assert!(found.validity.is_empty());
    let codes: Vec<&str> = found.schema_problems.iter().map(|p| p.diagnostic.code.key()).collect();
    assert_eq!(codes, ["xsd.document-not-loaded"]);
}

#[test]
fn the_hints_of_the_root_element() {
    let parsed = parse(&format!("<r {XSI} xsi:schemaLocation=\"urn:a a.xsd  urn:b\nb.xsd urn:odd\" xsi:noNamespaceSchemaLocation=\"c.xsd\"/>"));
    let hints = schema_hints(&parsed.syntax());
    let shown: Vec<(Option<&str>, &str)> = hints.iter().map(|h| (h.namespace.as_deref(), h.location.as_str())).collect();
    assert_eq!(shown, [(Some("urn:a"), "a.xsd"), (Some("urn:b"), "b.xsd"), (None, "c.xsd")], "an odd word is ignored");
    // References in the value are read.
    let escaped = parse(&format!("<r {XSI} xsi:noNamespaceSchemaLocation=\"a&amp;b.xsd\"/>"));
    assert_eq!(schema_hints(&escaped.syntax())[0].location, "a&b.xsd");
    // Another prefix for the same namespace; not on the root, or in no namespace: no hint.
    let other =
        parse("<r xmlns:i=\"http://www.w3.org/2001/XMLSchema-instance\" i:noNamespaceSchemaLocation=\"d.xsd\"><c schemaLocation=\"x y\"/></r>");
    assert_eq!(schema_hints(&other.syntax()).len(), 1);
    let none = parse("<r schemaLocation=\"urn:a a.xsd\"/>");
    assert!(schema_hints(&none.syntax()).is_empty());
    assert!(schema_hints(&parse("").syntax()).is_empty());
}

#[test]
fn a_schema_is_loaded_for_a_document_without_checking_it() {
    let xml = format!("<r {XSI} xsi:noNamespaceSchemaLocation=\"r.xsd\"><n>seven</n></r>");
    let parsed = parse(&xml);
    let loaded = load_schema(&parsed.syntax(), "/d/doc.xml", None, &mut Map(vec![("/d/r.xsd", plain_schema())])).expect("a schema");
    // The document is not checked (it has a bad value): what comes back is the schema, where it was read from, and what was asked for.
    assert_eq!(loaded.sources, ["/d/r.xsd"]);
    assert_eq!(loaded.hints, [SchemaHint { namespace: None, location: "r.xsd".to_owned() }]);
    assert!(loaded.problems.is_empty() && loaded.assembly.schema.element_by_name(&xmlcraft_xsd::model::QName::new(None, "r")).is_some());
    // An explicit schema takes the place of the ones the document names.
    let explicit =
        load_schema(&parsed.syntax(), "/d/doc.xml", Some("other.xsd"), &mut Map(vec![("/d/other.xsd", plain_schema())])).expect("a schema");
    assert_eq!(explicit.sources, ["/d/other.xsd"]);
}

#[test]
fn when_there_is_no_schema_to_load_it_says_why_and_which_were_asked_for() {
    let parsed = parse("<r/>");
    let none = load_schema(&parsed.syntax(), "/d/doc.xml", None, &mut Map(Vec::new())).expect_err("nothing named");
    assert_eq!(none.hints, []);
    assert!(none.why.contains("names no schema"), "{}", none.why);
    let xml = format!("<r {XSI} xsi:noNamespaceSchemaLocation=\"gone.xsd\"/>");
    let parsed = parse(&xml);
    let NoSchema { why, hints } = load_schema(&parsed.syntax(), "/d/doc.xml", None, &mut Map(Vec::new())).expect_err("not there");
    assert!(why.contains("gone.xsd") && why.contains("could not be loaded"), "{why}");
    assert_eq!(hints.len(), 1);
    // Several schemas are loaded through one that imports them: every file read is a source.
    let several = format!("<r xmlns=\"urn:t\" {XSI} xsi:schemaLocation=\"urn:t t.xsd urn:u u.xsd\"/>");
    let parsed = parse(&several);
    let u = format!("<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\" targetNamespace=\"urn:u\"/>");
    let loaded = load_schema(&parsed.syntax(), "/d/doc.xml", None, &mut Map(vec![("/d/t.xsd", namespaced_schema()), ("/d/u.xsd", u)])).expect("both");
    assert!(loaded.sources.iter().any(|s| s == "/d/t.xsd") && loaded.sources.iter().any(|s| s == "/d/u.xsd"), "{:?}", loaded.sources);
    assert_eq!(loaded.hints.len(), 2);
}

#[test]
fn hints_alone_load_the_same_schema_as_the_document_that_names_them() {
    let xml = format!("<r {XSI} xsi:noNamespaceSchemaLocation=\"r.xsd\"/>");
    let parsed = parse(&xml);
    let files = || Map(vec![("/d/r.xsd", plain_schema())]);
    let from_document = load_schema(&parsed.syntax(), "/d/doc.xml", None, &mut files()).expect("a schema");
    let from_hints = load_hinted(schema_hints(&parsed.syntax()), "/d/doc.xml", &mut files()).expect("a schema");
    assert_eq!((&from_hints.sources, &from_hints.hints), (&from_document.sources, &from_document.hints));
    assert_eq!(load_hinted(Vec::new(), "/d/doc.xml", &mut files()).expect_err("none").hints, []);
}

#[test]
fn the_namespace_of_the_root_element_is_the_one_it_declares_for_its_name() {
    let ns = |xml: &str| root_namespace(&parse(xml).syntax());
    assert_eq!(ns("<r xmlns=\"urn:a\"/>").as_deref(), Some("urn:a"), "the default namespace");
    assert_eq!(ns("<p:r xmlns:p=\"urn:b\" xmlns=\"urn:a\"/>").as_deref(), Some("urn:b"), "the prefix of the name wins");
    assert_eq!(ns("<r xmlns:p=\"urn:b\"/>"), None, "a prefix that names nothing in the name is no namespace");
    assert_eq!(ns("<p:r/>"), None, "an undeclared prefix");
    assert_eq!(ns("<r/>"), None);
    assert_eq!(ns("<r xmlns=\"\"/>"), None, "an undeclared default");
    assert_eq!(ns("<r xmlns='urn:a&amp;b'/>").as_deref(), Some("urn:a&b"), "references are replaced, either quote works");
    assert_eq!(ns(""), None);
    assert_eq!(ns("<!-- no root -->"), None);
    // Only what the root declares counts: a declaration further in does not reach it.
    assert_eq!(ns("<r><c xmlns=\"urn:a\"/></r>"), None);
}
