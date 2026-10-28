//! The schemas that ship with the app are schema documents: they read without a mistake.

use xmlcraft_xsd::model::Item;
use xmlcraft_xsd::read;

#[test]
fn the_schemas_of_the_bundle_read_without_a_diagnostic() {
    let mut read_any = false;
    for path in xmlcraft_schemas::paths().filter(|p| p.ends_with(".xsd")) {
        let text = xmlcraft_schemas::get_text(path).expect("text");
        let result = read(&text, &xmlcraft_schemas::uri_of(path));
        let problems: Vec<String> = result.diagnostics.iter().map(|d| format!("{}..{}: {}", d.span.start, d.span.end, d.message())).collect();
        assert!(problems.is_empty(), "{path}: {problems:#?}");
        assert!(result.document.is_some(), "{path}");
        read_any = true;
    }
    assert!(read_any, "the bundle has schema documents");
}

#[test]
fn the_schema_for_schemas_has_its_components() {
    let text = xmlcraft_schemas::get_text("w3c/xmlschema/XMLSchema.xsd").expect("the schema for schemas");
    let doc = read(&text, "").document.expect("a schema");
    assert_eq!(doc.target_namespace.as_deref(), Some("http://www.w3.org/2001/XMLSchema"));
    let (mut simple, mut complex, mut groups, mut attribute_groups, mut elements) = (0, 0, 0, 0, 0);
    for item in &doc.items {
        match item {
            Item::SimpleType(_) => simple += 1,
            Item::ComplexType(_) => complex += 1,
            Item::Group(_) => groups += 1,
            Item::AttributeGroup(_) => attribute_groups += 1,
            Item::Element(_) => elements += 1,
            _ => {}
        }
    }
    assert!(
        simple >= 50 && complex >= 30 && groups >= 10 && attribute_groups >= 2 && elements >= 40,
        "{simple} {complex} {groups} {attribute_groups} {elements}"
    );
}
