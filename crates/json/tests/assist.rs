//! Completion and hover from a schema: which names and values a place offers, and what the schema says of it.

use proptest::prelude::*;
use xmlcraft_json::{Draft, NoLoader, PropertyHint, SchemaSet};
use xmlcraft_json_syntax::{Options, Segment, Value, parse};

fn read(text: &str) -> Value {
    parse(text, &Options::default()).root.expect("a value")
}

fn set(schema: &str) -> SchemaSet {
    SchemaSet::new(read(schema), "", Draft::D2020_12, &NoLoader)
}

fn key(name: &str) -> Segment {
    Segment::Key(name.to_owned())
}

fn names(hints: &[PropertyHint]) -> Vec<&str> {
    hints.iter().map(|h| h.name.as_str()).collect()
}

const PERSON: &str = r##"{
  "$defs": {"address": {"type": "object", "description": "Where.", "properties": {"city": {"type": "string"}, "zip": {"type": "string"}}, "required": ["city"]}},
  "type": "object",
  "properties": {
    "name": {"type": "string", "description": "The full name."},
    "age": {"type": "integer", "default": 18, "deprecated": true},
    "home": {"$ref": "#/$defs/address"},
    "kind": {"enum": ["a", "b", 3, null]},
    "active": {"type": "boolean"},
    "tags": {"type": "array", "items": {"type": "string", "enum": ["x", "y"]}}
  },
  "required": ["name", "kind"]
}"##;

#[test]
fn the_names_of_an_object_come_with_what_is_required_first_and_what_is_present_left_out() {
    let s = set(PERSON);
    let hints = s.properties_at(&[], &[]);
    assert_eq!(names(&hints), ["name", "kind", "age", "home", "active", "tags"], "required first, then the schema's order");
    assert!(hints[0].required && hints[1].required && !hints[2].required);
    assert_eq!(hints[0].description.as_deref(), Some("The full name."));
    assert_eq!(hints[0].types, ["string"]);
    let age = hints.iter().find(|h| h.name == "age").expect("age");
    assert_eq!((age.default.as_deref(), age.deprecated), (Some("18"), true));
    let rest = s.properties_at(&[], &["name".to_owned(), "age".to_owned()]);
    assert_eq!(names(&rest), ["kind", "home", "active", "tags"]);
}

#[test]
fn a_reference_is_followed_into_the_members_of_a_member() {
    let s = set(PERSON);
    assert_eq!(names(&s.properties_at(&[key("home")], &[])), ["city", "zip"]);
    let docs = s.describe_at(&[key("home")]).expect("docs");
    assert_eq!(docs.description.as_deref(), Some("Where."));
    assert_eq!(docs.types, ["object"]);
}

#[test]
fn values_come_from_enums_booleans_defaults_and_array_items() {
    let s = set(PERSON);
    let texts = |path: &[Segment]| s.values_at(path).into_iter().map(|v| v.text).collect::<Vec<_>>();
    assert_eq!(texts(&[key("kind")]), ["\"a\"", "\"b\"", "3", "null"]);
    assert_eq!(texts(&[key("active")]), ["true", "false"]);
    assert_eq!(texts(&[key("age")]), ["18"]);
    assert_eq!(texts(&[key("tags"), Segment::Index(5)]), ["\"x\"", "\"y\""], "every item of the array has the schema of `items`");
    assert!(texts(&[key("nothing")]).is_empty());
}

#[test]
fn alternatives_are_all_offered_and_combined_schemas_are_merged() {
    let s = set(r#"{"allOf": [{"properties": {"a": {"type": "string"}}, "required": ["a"]}, {"properties": {"b": {"type": "number"}}}],
            "oneOf": [{"properties": {"c": {"const": "one"}}}, {"properties": {"d": {"enum": [1, 2]}}}],
            "if": {"properties": {"a": {"const": "x"}}}, "then": {"properties": {"e": {}}}, "else": {"properties": {"f": {}}}}"#);
    let hints = s.properties_at(&[], &[]);
    assert_eq!(names(&hints), ["a", "b", "c", "d", "e", "f"]);
    assert!(hints[0].required);
    assert_eq!(s.values_at(&[key("c")]).into_iter().map(|v| v.text).collect::<Vec<_>>(), ["\"one\""]);
}

#[test]
fn pattern_properties_additional_properties_and_tuples() {
    let s = set(r#"{"patternProperties": {"^x-": {"description": "An extension.", "type": "string"}},
            "additionalProperties": {"description": "Anything else.", "type": "number"},
            "properties": {"t": {"prefixItems": [{"const": 1}, {"const": 2}], "items": {"const": 9}},
                           "old": {"items": [{"const": "first"}], "additionalItems": {"const": "more"}}}}"#);
    assert_eq!(s.describe_at(&[key("x-foo")]).and_then(|d| d.description).as_deref(), Some("An extension."));
    assert_eq!(s.describe_at(&[key("other")]).and_then(|d| d.description).as_deref(), Some("Anything else."));
    let value = |path: &[Segment]| s.values_at(path).into_iter().map(|v| v.text).collect::<Vec<_>>();
    assert_eq!(value(&[key("t"), Segment::Index(1)]), ["2"]);
    assert_eq!(value(&[key("t"), Segment::Index(2)]), ["9"]);
    assert_eq!(value(&[key("old"), Segment::Index(0)]), ["\"first\""]);
    assert_eq!(value(&[key("old"), Segment::Index(1)]), ["\"more\""]);
}

#[test]
fn a_schema_that_refers_to_itself_ends() {
    let s = set(r##"{"$ref": "#", "properties": {"a": {"$ref": "#"}}, "allOf": [{"$ref": "#"}]}"##);
    assert_eq!(names(&s.properties_at(&[], &[])), ["a"]);
    let deep: Vec<Segment> = (0..50).map(|_| key("a")).collect();
    assert_eq!(names(&s.properties_at(&deep, &[])), ["a"]);
}

#[test]
fn a_value_that_cannot_be_written_as_json_is_not_offered_and_strings_are_escaped() {
    let s = set(r#"{"enum": ["a\"b\\c\n", 1e3, -0.5]}"#);
    let texts: Vec<String> = s.values_at(&[]).into_iter().map(|v| v.text).collect();
    assert_eq!(texts, [r#""a\"b\\c\n""#, "1e3", "-0.5"]);
    let json5 = SchemaSet::new(
        parse(r#"{"enum": [0xFF, Infinity, 2]}"#, &Options { dialect: xmlcraft_json_syntax::Dialect::Json5, ..Options::default() }).root.expect("v"),
        "",
        Draft::D2020_12,
        &NoLoader,
    );
    assert_eq!(json5.values_at(&[]).into_iter().map(|v| v.text).collect::<Vec<_>>(), ["2"]);
}

#[test]
fn a_schema_that_is_not_an_object_offers_nothing() {
    for schema in ["true", "[]", "3", "{\"properties\": 5, \"required\": \"x\", \"enum\": 1}"] {
        let s = set(schema);
        assert!(s.properties_at(&[key("a")], &[]).is_empty());
        assert!(s.values_at(&[Segment::Index(0)]).is_empty());
        let _ = s.describe_at(&[]);
    }
}

proptest! {
    #[test]
    fn any_path_into_any_small_schema_ends_without_panicking(
        path in proptest::collection::vec(prop_oneof!["[a-c]".prop_map(Segment::Key), (0usize..4).prop_map(Segment::Index)], 0..8),
        ref_to in "(#|#/properties/a|#/items|#/nowhere)",
    ) {
        let schema = format!(r##"{{"properties": {{"a": {{"$ref": "{ref_to}", "items": {{"$ref": "#"}}}}, "b": {{"allOf": [{{"$ref": "#/properties/a"}}]}}}}, "items": {{"$ref": "{ref_to}"}}, "additionalProperties": {{"$ref": "#"}}}}"##);
        let s = set(&schema);
        let _ = s.properties_at(&path, &[]);
        let _ = s.values_at(&path);
        let _ = s.describe_at(&path);
    }
}
