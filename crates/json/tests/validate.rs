//! Validation: what each keyword says, where a violation is reported, and that nothing runs away.

use proptest::prelude::*;
use xmlcraft_json::{Draft, Loader, NoLoader, SchemaSet, Violation};
use xmlcraft_json_syntax::{Options, Value, parse};

fn read(text: &str) -> Value {
    parse(text, &Options::default()).root.expect("a value")
}

fn check_with(schema: &str, draft: Draft, instance: &str, loader: &dyn Loader) -> Vec<Violation> {
    SchemaSet::new(read(schema), "", draft, loader).validate(&read(instance)).violations
}

/// `(keyword, where in the instance)` of each violation.
fn check(schema: &str, instance: &str) -> Vec<(&'static str, String)> {
    check_with(schema, Draft::D2020_12, instance, &NoLoader).into_iter().map(|v| (v.keyword, v.instance)).collect()
}

#[test]
fn types_enums_and_constants() {
    assert_eq!(check(r#"{"type":"string"}"#, r#""x""#), []);
    assert_eq!(check(r#"{"type":"string"}"#, "1"), [("type", String::new())]);
    assert_eq!(check(r#"{"type":["string","null"]}"#, "null"), []);
    assert_eq!(check(r#"{"type":"integer"}"#, "1.0"), [], "1.0 is an integer since draft 6");
    assert_eq!(check_with(r#"{"type":"integer"}"#, Draft::D04, "1.0", &NoLoader).len(), 1, "and was not in draft 4");
    assert_eq!(check(r#"{"enum":[1,"a",{"x":[1,2]}]}"#, r#"{"x":[1.0,2]}"#), [], "equal by value: 1.0 is 1");
    assert_eq!(check(r#"{"enum":[1,"a"]}"#, "2"), [("enum", String::new())]);
    assert_eq!(check(r#"{"const":{"a":1,"b":2}}"#, r#"{"b":2,"a":1}"#), [], "the order of members does not matter");
    assert_eq!(check(r#"{"const":false}"#, "0"), [("const", String::new())], "false is not 0");
}

#[test]
fn numbers_are_compared_exactly() {
    let s = r#"{"minimum":0.1,"maximum":10,"multipleOf":0.0001}"#;
    assert_eq!(check(s, "0.0075"), [("minimum", String::new())]);
    assert_eq!(check(r#"{"multipleOf":0.0001}"#, "0.0075"), []);
    assert_eq!(check(s, "10.0001"), [("maximum", String::new())]);
    assert_eq!(check(r#"{"exclusiveMinimum":1,"exclusiveMaximum":3}"#, "3"), [("exclusiveMaximum", String::new())]);
    assert_eq!(check(r#"{"minimum":12345678901234567890}"#, "12345678901234567891"), [], "bigger than a 64-bit number, still exact");
    assert_eq!(check(r#"{"maximum":12345678901234567890}"#, "12345678901234567891"), [("maximum", String::new())]);
    assert_eq!(check(r#"{"multipleOf":2}"#, "12345678901234567891"), [("multipleOf", String::new())], "odd, though a float could not tell");
    assert_eq!(check(r#"{"multipleOf":2}"#, "12345678901234567890"), []);
    // Draft 4: exclusiveMinimum is a flag.
    let d4 = check_with(r#"{"minimum":1,"exclusiveMinimum":true}"#, Draft::D04, "1", &NoLoader);
    assert_eq!(d4.len(), 1);
    assert!(check_with(r#"{"minimum":1,"exclusiveMinimum":true}"#, Draft::D04, "1.5", &NoLoader).is_empty());
}

#[test]
fn equal_numbers_are_equal_however_far_they_are_spelled_out() {
    let long = "100000000000000000000000000000000000000000"; // 42 digits: not held exactly
    assert_eq!(check(&format!(r#"{{"enum":[{long}]}}"#), "1e41"), []);
    assert_eq!(check(&format!(r#"{{"const":{long}}}"#), "1.0E41"), []);
    assert_eq!(check(r#"{"uniqueItems":true}"#, &format!("[1e41, {long}]")), [("uniqueItems", String::new())]);
    assert_eq!(check(r#"{"uniqueItems":true}"#, &format!("[1e41, 1e42, {long}0]")), [("uniqueItems", String::new())], "the third is the second");
    assert_eq!(check(r#"{"uniqueItems":true}"#, "[0, -0, 0.0]"), [("uniqueItems", String::new())]);
    assert_eq!(check(r#"{"uniqueItems":true}"#, "[0, -0e99999999999]"), [("uniqueItems", String::new())], "minus zero out of range is still zero");
    assert_eq!(check(r#"{"uniqueItems":true}"#, "[1, 2, 3]"), []);
}

#[test]
fn a_false_schema_is_reported_at_the_schema_and_the_cap_does_not_rewrite_what_was_kept() {
    let many = format!("[{}]", "1,".repeat(2000).trim_end_matches(','));
    let outcome = SchemaSet::new(read(r#"{"items":false}"#), "", Draft::D2020_12, &NoLoader).validate(&read(&many));
    assert_eq!(outcome.violations.len(), xmlcraft_json::MAX_VIOLATIONS);
    assert!(outcome.truncated);
    assert!(outcome.violations.iter().all(|v| v.keyword == "false" && v.schema == "/items"), "{:?}", outcome.violations.last());
    // When the cap is reached by one keyword and a false schema comes after, the last kept violation is not touched.
    let ones = format!("[{}]", "1,".repeat(1500).trim_end_matches(','));
    let outcome = SchemaSet::new(read(r#"{"items":{"type":"string"},"contains":false}"#), "", Draft::D2020_12, &NoLoader).validate(&read(&ones));
    assert_eq!(outcome.violations.len(), xmlcraft_json::MAX_VIOLATIONS);
    assert_eq!(outcome.violations.last().map(|v| (v.keyword, v.schema.as_str())), Some(("type", "/items/type")));
}

#[test]
fn a_dynamic_reference_that_lands_in_another_resource_reads_its_relative_references_from_there() {
    let files = |uri: &str| match uri {
        "http://b.test/lib" => {
            Some(r##"{"$id":"http://b.test/lib","$dynamicAnchor":"node","type":"object","properties":{"child":{"$dynamicRef":"#node"}}}"##.to_owned())
        }
        "http://a.test/num.json" => Some(r#"{"type":"integer"}"#.to_owned()),
        "http://b.test/num.json" => Some(r#"{"type":"string"}"#.to_owned()),
        _ => None,
    };
    let root = r##"{"$id":"http://a.test/root","$defs":{"n":{"$dynamicAnchor":"node","properties":{"num":{"$ref":"num.json"}}}},"$ref":"http://b.test/lib"}"##;
    // The child is checked as the outermost `node` (in a.test/root), so its `num` is an integer from a.test, not a string from b.test.
    assert!(check_with(root, Draft::D2020_12, r#"{"child":{"num":5}}"#, &files).is_empty());
    assert_eq!(check_with(root, Draft::D2020_12, r#"{"child":{"num":"x"}}"#, &files).len(), 1);
}

#[test]
fn strings_count_characters_not_bytes_and_patterns_search() {
    assert_eq!(check(r#"{"maxLength":2}"#, r#""日本""#), []);
    assert_eq!(check(r#"{"maxLength":2}"#, r#""😀😀😀""#), [("maxLength", String::new())]);
    assert_eq!(check(r#"{"minLength":2}"#, r#""x""#), [("minLength", String::new())]);
    assert_eq!(check(r#"{"pattern":"^a.c$"}"#, r#""abc""#), []);
    assert_eq!(check(r#"{"pattern":"b"}"#, r#""abc""#), [], "a pattern is not anchored");
    assert_eq!(check(r#"{"pattern":"^b"}"#, r#""abc""#), [("pattern", String::new())]);
    assert_eq!(check(r#"{"pattern":"(["}"#, r#""abc""#), [], "a pattern that is no pattern is left alone");
}

#[test]
fn arrays_are_checked_item_by_item_and_violations_say_which() {
    assert_eq!(check(r#"{"items":{"type":"integer"}}"#, r#"[1,"a",3,"b"]"#), [("type", "/1".into()), ("type", "/3".into())]);
    assert_eq!(
        check(r#"{"prefixItems":[{"type":"string"},{"type":"integer"}],"items":false}"#, r#"["a",1,true]"#),
        [("false", "/2".into())],
        "items after the prefix"
    );
    assert_eq!(check(r#"{"minItems":2,"maxItems":3}"#, "[1]"), [("minItems", String::new())]);
    assert_eq!(check(r#"{"uniqueItems":true}"#, r#"[1,2,1.0]"#), [("uniqueItems", String::new())]);
    assert_eq!(check(r#"{"uniqueItems":true}"#, r#"[{"a":1,"b":2},{"b":2,"a":1}]"#), [("uniqueItems", String::new())]);
    assert_eq!(check(r#"{"uniqueItems":true}"#, r#"[1,true,"1"]"#), []);
    assert_eq!(check(r#"{"contains":{"const":3},"minContains":2}"#, "[3,1]"), [("contains", String::new())]);
    assert_eq!(check(r#"{"contains":{"const":3},"maxContains":1}"#, "[3,3]"), [("maxContains", String::new())]);
    // Draft 7's array form of items, with additionalItems.
    let v = check_with(r#"{"items":[{"type":"string"}],"additionalItems":{"type":"integer"}}"#, Draft::D07, r#"["a",1,"b"]"#, &NoLoader);
    assert_eq!(v.iter().map(|v| (v.keyword, v.instance.as_str())).collect::<Vec<_>>(), [("type", "/2")]);
}

#[test]
fn objects_and_the_places_of_their_violations() {
    let s = r#"{"properties":{"a":{"type":"integer"},"b":{"type":"string"}},"required":["a","c"],"additionalProperties":false}"#;
    assert_eq!(check(s, r#"{"a":"x","d":1}"#), [("required", String::new()), ("type", "/a".into()), ("additionalProperties", "/d".into())]);
    let patterns = r#"{"patternProperties":{"^x_":{"type":"integer"}},"additionalProperties":{"type":"string"}}"#;
    assert_eq!(check(patterns, r#"{"x_a":1,"x_b":"no","c":"yes","d":2}"#), [("type", "/x_b".into()), ("type", "/d".into())]);
    assert_eq!(check(r#"{"propertyNames":{"maxLength":2}}"#, r#"{"ab":1,"abc":2}"#), [("maxLength", "/abc".into())]);
    assert_eq!(check(r#"{"minProperties":2}"#, "{}"), [("minProperties", String::new())]);
    assert_eq!(check(r#"{"dependentRequired":{"a":["b"]}}"#, r#"{"a":1}"#), [("dependentRequired", String::new())]);
    assert_eq!(check(r#"{"dependentSchemas":{"a":{"required":["b"]}}}"#, r#"{"a":1}"#), [("required", String::new())]);
    // Keys with a slash or a tilde are escaped in pointers.
    let escaped = r#"{"properties":{"a/b":{"type":"integer"},"c~d":{"type":"integer"}}}"#;
    assert_eq!(check(escaped, r#"{"a/b":"x","c~d":"y"}"#), [("type", "/a~1b".into()), ("type", "/c~0d".into())]);
}

#[test]
fn applicators_and_conditionals() {
    assert_eq!(check(r#"{"allOf":[{"type":"integer"},{"minimum":5}]}"#, "3"), [("minimum", String::new())]);
    assert_eq!(check(r#"{"anyOf":[{"type":"string"},{"minimum":5}]}"#, "3"), [("anyOf", String::new())], "the branches' own problems are not shown");
    assert_eq!(check(r#"{"oneOf":[{"type":"integer"},{"minimum":5}]}"#, "7"), [("oneOf", String::new())], "two match");
    assert_eq!(check(r#"{"oneOf":[{"type":"string"},{"minimum":5}]}"#, "7"), []);
    assert_eq!(check(r#"{"not":{"type":"string"}}"#, r#""x""#), [("not", String::new())]);
    let cond = r#"{"if":{"properties":{"k":{"const":1}},"required":["k"]},"then":{"required":["a"]},"else":{"required":["b"]}}"#;
    assert_eq!(check(cond, r#"{"k":1}"#), [("required", String::new())]);
    assert_eq!(check(cond, r#"{"k":2,"b":1}"#), []);
    assert_eq!(check(r#"true"#, "1"), []);
    assert_eq!(check(r#"false"#, "1"), [("false", String::new())]);
}

#[test]
fn references_are_followed_within_a_document_and_to_other_documents() {
    let s = r##"{"$defs":{"n":{"type":"integer"},"list":{"type":"array","items":{"$ref":"#/$defs/n"}}},"$ref":"#/$defs/list"}"##;
    assert_eq!(check(s, r#"[1,"x"]"#), [("type", "/1".into())]);
    let recursive = r##"{"type":"object","properties":{"next":{"$ref":"#"}}}"##;
    assert_eq!(check(recursive, r#"{"next":{"next":{"next":1}}}"#), [("type", "/next/next/next".into())]);
    let other = |uri: &str| (uri == "http://x.test/n.json").then(|| r#"{"type":"integer"}"#.to_owned());
    let v = check_with(r#"{"properties":{"a":{"$ref":"http://x.test/n.json"}}}"#, Draft::D2020_12, r#"{"a":"x"}"#, &other);
    assert_eq!(v.iter().map(|v| (v.keyword, v.instance.as_str())).collect::<Vec<_>>(), [("type", "/a")]);
    // Relative to the $id; anchors and a reference that is not found.
    let ids = r##"{"$id":"http://x.test/root","properties":{"a":{"$ref":"n.json"}}}"##;
    assert_eq!(check_with(ids, Draft::D2020_12, r#"{"a":"x"}"#, &other).len(), 1);
    let anchor = r##"{"$defs":{"n":{"$anchor":"num","type":"integer"}},"$ref":"#num"}"##;
    assert_eq!(check(anchor, r#""x""#), [("type", String::new())]);
    assert_eq!(check(r##"{"$ref":"#/nowhere"}"##, "1"), [("$ref", String::new())]);
    // Up to draft 7 a $ref hides its siblings.
    assert!(
        check_with(r##"{"$ref":"#/definitions/a","minimum":100,"definitions":{"a":{"type":"integer"}}}"##, Draft::D07, "5", &NoLoader).is_empty()
    );
    assert_eq!(check(r##"{"$ref":"#/$defs/a","minimum":100,"$defs":{"a":{"type":"integer"}}}"##, "5"), [("minimum", String::new())]);
}

#[test]
fn what_nothing_else_evaluated_is_unevaluated() {
    let s = r#"{"properties":{"a":true},"allOf":[{"properties":{"b":true}}],"unevaluatedProperties":false}"#;
    assert_eq!(check(s, r#"{"a":1,"b":2,"c":3}"#), [("unevaluatedProperties", "/c".into())]);
    let items = r#"{"prefixItems":[true],"contains":{"const":9},"unevaluatedItems":false}"#;
    assert_eq!(check(items, "[1,9,5]"), [("unevaluatedItems", "/2".into())]);
    assert_eq!(check(r#"{"if":{"properties":{"a":true},"required":["a"]},"unevaluatedProperties":false}"#, r#"{"a":1}"#), []);
    // What the one branch of oneOf (and the branches of anyOf) evaluated counts; what a branch that did not match evaluated does not.
    let one = r#"{"oneOf":[{"properties":{"a":true},"required":["a"]},{"properties":{"b":true},"required":["b"]}],"unevaluatedProperties":false}"#;
    assert_eq!(check(one, r#"{"a":1}"#), []);
    assert_eq!(
        check(one, r#"{"a":1,"b":2}"#),
        [("oneOf", String::new()), ("unevaluatedProperties", "/a".into()), ("unevaluatedProperties", "/b".into())]
    );
    let any = r#"{"anyOf":[{"properties":{"a":true},"required":["a"]},{"properties":{"c":true},"required":["c"]}],"unevaluatedProperties":false}"#;
    assert_eq!(check(any, r#"{"a":1,"b":2}"#), [("unevaluatedProperties", "/b".into())]);
}

#[test]
fn violations_stop_at_the_cap_and_cycles_and_deep_schemas_end() {
    let many = format!("[{}]", "1,".repeat(3000).trim_end_matches(','));
    let outcome = SchemaSet::new(read(r#"{"items":{"type":"string"}}"#), "", Draft::D2020_12, &NoLoader).validate(&read(&many));
    assert_eq!(outcome.violations.len(), xmlcraft_json::MAX_VIOLATIONS);
    assert!(outcome.truncated && !outcome.is_valid());
    // A schema that refers to itself with no progress ends.
    let looping = SchemaSet::new(read(r##"{"$ref":"#"}"##), "", Draft::D2020_12, &NoLoader).validate(&read("1"));
    assert!(looping.truncated);
    let mutual =
        SchemaSet::new(read(r##"{"$defs":{"a":{"$ref":"#/$defs/b"},"b":{"$ref":"#/$defs/a"}},"$ref":"#/$defs/a"}"##), "", Draft::D2020_12, &NoLoader);
    assert!(mutual.validate(&read("1")).truncated);
    // A deeply nested instance against a recursive schema ends.
    let deep = format!("{}1{}", "[".repeat(2000), "]".repeat(2000));
    let outcome = SchemaSet::new(read(r##"{"items":{"$ref":"#"}}"##), "", Draft::D2020_12, &NoLoader).validate(&read(&deep));
    assert!(outcome.truncated, "the instance is deeper than the schema is followed: the result says it was cut off");
}

#[test]
fn the_draft_comes_from_the_schema() {
    let set = SchemaSet::new(read(r#"{"$schema":"http://json-schema.org/draft-07/schema#","type":"integer"}"#), "", Draft::D2020_12, &NoLoader);
    assert_eq!(set.draft(), Draft::D07);
    assert_eq!(SchemaSet::new(read("{}"), "", Draft::D2019_09, &NoLoader).draft(), Draft::D2019_09);
}

#[test]
fn a_violation_points_into_the_text_of_the_instance() {
    let text = "{\n  \"a\": [1, \"x\"],\n  \"b\": 2\n}";
    let instance = read(text);
    let outcome = SchemaSet::new(read(r#"{"properties":{"a":{"items":{"type":"integer"}}}}"#), "", Draft::D2020_12, &NoLoader).validate(&instance);
    let at = instance.span_of_pointer(&outcome.violations[0].instance).expect("a place");
    assert_eq!(&text[at], "\"x\"");
    assert_eq!(outcome.violations[0].schema, "/properties/a/items/type");
}

fn arb_json(depth: u32) -> BoxedStrategy<String> {
    let leaf = prop_oneof![
        Just("null".to_owned()),
        Just("true".to_owned()),
        Just("1".to_owned()),
        Just("-2.5".to_owned()),
        Just("\"ab\"".to_owned()),
        Just("\"\"".to_owned())
    ];
    if depth == 0 {
        return leaf.boxed();
    }
    let inner = arb_json(depth - 1);
    prop_oneof![
        leaf,
        prop::collection::vec(inner.clone(), 0..4).prop_map(|v| format!("[{}]", v.join(","))),
        prop::collection::vec(("[a-c]", inner), 0..4)
            .prop_map(|v| format!("{{{}}}", v.iter().map(|(k, x)| format!("\"{k}\":{x}")).collect::<Vec<_>>().join(","))),
    ]
    .boxed()
}

fn arb_schema(depth: u32) -> BoxedStrategy<String> {
    let keyword = |k: &'static str, v: BoxedStrategy<String>| v.prop_map(move |v| format!("\"{k}\":{v}"));
    let sub = if depth == 0 { Just("true".to_owned()).boxed() } else { arb_schema(depth - 1) };
    let list = prop::collection::vec(sub.clone(), 0..3).prop_map(|v| format!("[{}]", v.join(",")));
    prop_oneof![
        Just("true".to_owned()),
        Just("false".to_owned()),
        prop::collection::vec(
            prop_oneof![
                keyword(
                    "type",
                    prop_oneof![
                        Just("\"string\"".to_owned()),
                        Just("[\"integer\",\"null\"]".to_owned()),
                        Just("\"array\"".to_owned()),
                        Just("\"object\"".to_owned())
                    ]
                    .boxed()
                ),
                keyword("items", sub.clone()),
                keyword("prefixItems", list.clone().boxed()),
                keyword("properties", sub.clone().prop_map(|s| format!("{{\"a\":{s},\"b\":true}}")).boxed()),
                keyword("additionalProperties", sub.clone()),
                keyword("allOf", list.clone().boxed()),
                keyword("anyOf", list.clone().boxed()),
                keyword("oneOf", list.boxed()),
                keyword("not", sub.clone()),
                keyword("contains", sub.clone()),
                keyword("unevaluatedProperties", sub.clone()),
                keyword("unevaluatedItems", sub.clone()),
                keyword("if", sub.clone()),
                keyword("then", sub),
                keyword("minimum", Just("0".to_owned()).boxed()),
                keyword("maxLength", Just("1".to_owned()).boxed()),
                keyword("required", Just("[\"a\"]".to_owned()).boxed()),
                keyword("$ref", Just("\"#\"".to_owned()).boxed()),
                keyword("uniqueItems", Just("true".to_owned()).boxed()),
            ],
            0..4
        )
        .prop_map(|kv| format!("{{{}}}", kv.join(","))),
    ]
    .boxed()
}

proptest! {
    #[test]
    fn any_schema_against_any_instance_ends_without_a_panic_and_in_the_same_way_twice(schema in arb_schema(3), instance in arb_json(3), draft in 0usize..5) {
        let draft = [Draft::D04, Draft::D06, Draft::D07, Draft::D2019_09, Draft::D2020_12][draft];
        let a = check_with(&schema, draft, &instance, &NoLoader);
        let b = check_with(&schema, draft, &instance, &NoLoader);
        prop_assert_eq!(&a, &b);
        prop_assert!(a.len() <= xmlcraft_json::MAX_VIOLATIONS);
        // Every violation points at a place that exists in the instance.
        let value = read(&instance);
        for v in &a {
            prop_assert!(value.span_of_pointer(&v.instance).is_some(), "{} in {instance}", v.instance);
        }
    }

    #[test]
    fn the_empty_schema_and_true_accept_everything_and_false_rejects_everything(instance in arb_json(3)) {
        prop_assert_eq!(check_with("{}", Draft::D2020_12, &instance, &NoLoader).len(), 0);
        prop_assert_eq!(check_with("true", Draft::D2020_12, &instance, &NoLoader).len(), 0);
        prop_assert_eq!(check_with("false", Draft::D2020_12, &instance, &NoLoader).len(), 1);
    }
}
