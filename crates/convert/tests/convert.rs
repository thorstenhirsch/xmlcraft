//! Conversions between the formats: the mappings, and that what can be converted back comes back.

use proptest::prelude::*;
use xmlcraft_convert::{ConvertOptions, Format, convert, extended_to_plain, json_text, read_value, yaml_text};
use xmlcraft_json_syntax::{Data, Options, Value, parse};

fn to(text: &str, from: Format, to: Format) -> String {
    convert(text, from, to, &ConvertOptions::default()).unwrap_or_else(|e| panic!("{e}")).text
}

fn compact(text: &str, from: Format) -> String {
    let options = ConvertOptions { indent: None, ..ConvertOptions::default() };
    convert(text, from, Format::Json, &options).unwrap_or_else(|e| panic!("{e}")).text
}

#[test]
fn xml_becomes_data_with_attributes_text_and_repeated_elements() {
    let xml = "<?xml version=\"1.0\"?>\n<lib id=\"1\" xmlns:x=\"urn:x\"><!-- c --><book x:k=\"a&amp;b\">T &lt;1&gt;</book><book>U</book><note/><mixed>a<b>1</b>c</mixed><![CDATA[ raw ]]></lib>";
    assert_eq!(
        compact(xml, Format::Xml),
        r##"{"lib":{"@id":"1","@xmlns:x":"urn:x","#text":"raw","book":[{"@x:k":"a&b","#text":"T <1>"},"U"],"note":null,"mixed":{"#text":"ac","b":"1"}}}"##
    );
    let guessing =
        ConvertOptions { xml_to_data: xmlcraft_convert::XmlToData { guess_types: true, ..Default::default() }, indent: None, ..Default::default() };
    let typed = convert("<a><n>12</n><f>1.5e3</f><b>true</b><z>null</z><s>007</s><t>+1</t></a>", Format::Xml, Format::Json, &guessing)
        .expect("converts")
        .text;
    assert_eq!(typed, r#"{"a":{"n":12,"f":1.5e3,"b":true,"z":null,"s":"007","t":"+1"}}"#);
    // Attribute values are text like any other.
    let attributes = convert(r#"<a n="3" s="x" z="007"/>"#, Format::Xml, Format::Json, &guessing).expect("converts").text;
    assert_eq!(attributes, r#"{"a":{"@n":3,"@s":"x","@z":"007"}}"#);
    assert!(
        convert("<a><b></a>", Format::Xml, Format::Json, &ConvertOptions::default())
            .expect_err("not well-formed")
            .message
            .contains("not well-formed")
    );
    assert!(convert("", Format::Xml, Format::Json, &ConvertOptions::default()).is_err());
}

#[test]
fn data_becomes_xml_with_attributes_arrays_and_names_that_are_fixed() {
    let json = r##"{"lib":{"@id":"1","#text":"hi","book":["A","B"],"empty":null,"n":5,"ok":true,"nested":{"x":"<&>"}}}"##;
    let xml = to(json, Format::Json, Format::Xml);
    assert_eq!(
        xml,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<lib id=\"1\">hi\n  <book>A</book>\n  <book>B</book>\n  <empty/>\n  <n>5</n>\n  <ok>true</ok>\n  <nested>\n    <x>&lt;&amp;&gt;</x>\n  </nested>\n</lib>\n"
    );
    // Not a single root: wrapped; a list is the items; a scalar is the text; bad names are repaired with a warning.
    assert!(to(r#"{"a":1,"b":2}"#, Format::Json, Format::Xml).contains("<root>"));
    assert!(to("[1,[2,3]]", Format::Json, Format::Xml).contains("<item>1</item>"));
    assert!(to("5", Format::Json, Format::Xml).contains("<root>5</root>"));
    let converted = convert(r#"{"1 bad":{"a b":1, "@x y":"v"}}"#, Format::Json, Format::Xml, &ConvertOptions { indent: None, ..Default::default() })
        .expect("converts");
    assert!(converted.text.contains("<_1_bad x_y=\"v\"><a_b>1</a_b></_1_bad>"), "{}", converted.text);
    assert!(converted.warnings.iter().any(|w| w.contains("'1 bad'")), "{:?}", converted.warnings);
    assert!(convert(r#"{"a":{"@x":[1]}}"#, Format::Json, Format::Xml, &ConvertOptions::default()).is_err(), "an attribute cannot be a list");
    // Control characters XML cannot hold are dropped.
    assert!(!to("{\"a\":\"x\\u0001y\"}", Format::Json, Format::Xml).contains('\u{1}'));
    // A prefix is kept where it is declared (by `@xmlns:p` on the element or one around it) and repaired where it is not.
    let declared = convert(
        r#"{"x:a":{"@xmlns:x":"urn:x","x:b":{"@x:k":"v"},"y:c":1}}"#,
        Format::Json,
        Format::Xml,
        &ConvertOptions { indent: None, ..Default::default() },
    )
    .expect("converts");
    assert!(declared.text.contains("<x:a xmlns:x=\"urn:x\"><x:b x:k=\"v\"/><y_c>1</y_c></x:a>"), "{}", declared.text);
    assert!(
        declared.warnings.iter().any(|w| w.contains("'y:c'")) && !declared.warnings.iter().any(|w| w.contains("'x:a'")),
        "{:?}",
        declared.warnings
    );
    assert!(to(r#"{"x:a":1}"#, Format::Json, Format::Xml).contains("<x_a>1</x_a>"), "undeclared: no namespace error");
    // A declaration holds for the element and what is in it, not for what comes after.
    let scoped = convert(
        r#"{"r":{"a":{"@xmlns:p":"u","p:x":1},"b":{"p:y":1}}}"#,
        Format::Json,
        Format::Xml,
        &ConvertOptions { indent: None, ..Default::default() },
    )
    .expect("converts");
    assert!(scoped.text.contains("<a xmlns:p=\"u\"><p:x>1</p:x></a><b><p_y>1</p_y></b>"), "{}", scoped.text);
    // Quotes, line breaks and tabs in an attribute are escaped so that they read back.
    let attribute = to("{\"a\":{\"@x\":\"say \\\"hi\\\"\\n\\tok\"}}", Format::Json, Format::Xml);
    assert!(attribute.contains("x=\"say &quot;hi&quot;&#10;&#9;ok\""), "{attribute}");
    assert_eq!(compact(&attribute, Format::Xml), r#"{"a":{"@x":"say \"hi\"\n\tok"}}"#);
}

#[test]
fn the_standard_mapping_is_lossless_for_json() {
    let options = ConvertOptions { standard_mapping: true, indent: None, ..Default::default() };
    let json = r##"{"a":[1,"x",null,true,{"k":"v"}],"b":{},"c":[],"e":"<&>\"","d":-2.5e3}"##;
    let xml = convert(json, Format::Json, Format::Xml, &options).expect("converts").text;
    assert!(xml.starts_with("<map xmlns=\"http://www.w3.org/2005/xpath-functions\"><array key=\"a\"><number>1</number><string>x</string><null/><boolean>true</boolean><map><string key=\"k\">v</string></map></array>"), "{xml}");
    let back = convert(&xml, Format::Xml, Format::Json, &options).expect("converts back").text;
    assert_eq!(back, json);
    assert!(
        convert("<array xmlns=\"http://www.w3.org/2005/xpath-functions\"><x/></array>", Format::Xml, Format::Json, &options)
            .expect_err("not part of the mapping")
            .message
            .contains("not part of")
    );
    assert!(
        convert("<map xmlns=\"http://www.w3.org/2005/xpath-functions\"><null/></map>", Format::Xml, Format::Json, &options)
            .expect_err("a key")
            .message
            .contains("key attribute")
    );
}

#[test]
fn json_and_yaml_convert_into_each_other_and_say_what_is_lost() {
    let json =
        r##"{"name":"x","nums":[1,2.5,null,true],"nested":{"a":{"b":[{"c":1,"d":[]},{}]}},"s":["yes","123","- a","a: b","","  pad","line\nbreak"]}"##;
    let yaml = to(json, Format::Json, Format::Yaml);
    assert!(
        yaml.starts_with("name: x\nnums:\n  - 1\n  - 2.5\n  - null\n  - true\nnested:\n  a:\n    b:\n      - c: 1\n        d: []\n      - {}\n"),
        "{yaml}"
    );
    assert!(
        yaml.contains("\"yes\"")
            && yaml.contains("\"123\"")
            && yaml.contains("\"- a\"")
            && yaml.contains("\"a: b\"")
            && yaml.contains("\"line\\nbreak\""),
        "{yaml}"
    );
    assert_eq!(compact(&yaml, Format::Yaml), compact(json, Format::Json), "and back");
    // YAML with aliases and merges is expanded; several documents are a list.
    assert_eq!(compact("a: &x [1]\nb: *x\n", Format::Yaml), r#"{"a":[1],"b":[1]}"#);
    assert_eq!(compact("a: 1\n---\nb: 2\n", Format::Yaml), r#"[{"a":1},{"b":2}]"#);
    // JSON cannot hold NaN.
    assert!(
        convert("a: .nan\n", Format::Yaml, Format::Json, &ConvertOptions::default()).expect_err("no NaN in JSON").message.contains("cannot hold")
    );
    assert_eq!(to("a: .inf\n", Format::Yaml, Format::Yaml), "a: .inf\n");
    // JSON5-ish input numbers are written as JSON writes them.
    let five = ConvertOptions { indent: None, ..Default::default() };
    assert_eq!(convert("{a: +1, b: 0x10, c: 'q'}", Format::Json, Format::Json, &five).expect("converts").text, r#"{"a":1,"b":16,"c":"q"}"#);
    // A repeated name is said once.
    let d = convert(r#"{"a":1,"a":2}"#, Format::Json, Format::Yaml, &five).expect("converts");
    assert_eq!(d.text, "a: 1\n");
    assert_eq!(d.warnings.len(), 1);
}

#[test]
fn bson_text_converts_to_plain_data_and_plain_data_to_bson_text() {
    let ext = r##"{"_id":{"$oid":"0123456789abcdef01234567"},"n":{"$numberInt":"7"},"big":{"$numberLong":"9007199254740993"},"d":{"$date":{"$numberLong":"1700000000123"}},"r":{"$regularExpression":{"pattern":"a+","options":"i"}},"u":{"$undefined":true},"x":{"$numberDouble":"NaN"},"nested":{"k":{"$minKey":1}}}"##;
    let converted = convert(ext, Format::Bson, Format::Json, &ConvertOptions { indent: None, ..Default::default() }).expect("converts");
    assert_eq!(
        converted.text,
        r##"{"_id":"0123456789abcdef01234567","n":7,"big":9007199254740993,"d":"2023-11-14T22:13:20.123Z","r":"/a+/i","u":null,"x":"NaN","nested":{"k":"$minKey"}}"##
    );
    assert!(converted.warnings.iter().any(|w| w.contains("BSON types")));
    // The other way: any JSON is Extended JSON; numbers get the type their size says.
    let bson = to(r#"{"a":1,"b":3000000000,"c":1.5,"s":"x","l":[true,null]}"#, Format::Json, Format::Bson);
    assert!(
        bson.contains("\"a\": {\"$numberInt\": \"1\"}")
            && bson.contains("\"$numberLong\": \"3000000000\"")
            && bson.contains("\"$numberDouble\": \"1.5\""),
        "{bson}"
    );
    assert_eq!(to(&bson, Format::Bson, Format::Bson), bson, "BSON to BSON changes nothing");
    assert!(convert("{\"a\": {\"$oid\": \"zz\"}}", Format::Bson, Format::Json, &ConvertOptions::default()).is_err());
    // Several documents are a list.
    let list = compact("{\"a\":{\"$numberInt\":\"1\"}}\n{\"a\":{\"$numberInt\":\"2\"}}\n", Format::Bson);
    assert_eq!(list, r#"[{"a":1},{"a":2}]"#);
    // Dates in the years the ISO form covers are text; others stay the milliseconds.
    let dates = |ms: &str| compact(&format!("{{\"d\":{{\"$date\":{{\"$numberLong\":\"{ms}\"}}}}}}"), Format::Bson);
    assert_eq!(dates("0"), r#"{"d":"1970-01-01T00:00:00.000Z"}"#);
    assert_eq!(dates("-1"), r#"{"d":-1}"#);
    assert_eq!(dates("253402300800000"), r#"{"d":253402300800000}"#);
    let _ = extended_to_plain;
}

fn arb_value() -> impl Strategy<Value = String> {
    let leaf = prop_oneof![
        Just("null".to_owned()),
        Just("true".to_owned()),
        Just("false".to_owned()),
        any::<i32>().prop_map(|n| n.to_string()),
        (-1000i32..1000).prop_map(|n| format!("{}.5", n)),
        "[a-zA-Z0-9 :#\\-\\[\\]{},&*!|>'%@`\"\\\\é\n\t]{0,10}".prop_map(|s| serde_json::to_string(&s).unwrap_or_default()),
    ];
    leaf.prop_recursive(4, 40, 5, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(|v| format!("[{}]", v.join(","))),
            prop::collection::vec(("[a-zA-Z0-9 :#\\-é]{0,6}", inner), 0..4).prop_map(|v| {
                let mut seen = std::collections::HashSet::new();
                let members: Vec<String> = v
                    .into_iter()
                    .filter(|(k, _)| seen.insert(k.clone()))
                    .map(|(k, x)| format!("{}:{}", serde_json::to_string(&k).unwrap_or_default(), x))
                    .collect();
                format!("{{{}}}", members.join(","))
            }),
        ]
    })
}

fn same(a: &Value, b: &Value) -> bool {
    match (&a.data, &b.data) {
        (Data::Array(x), Data::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q)),
        (Data::Object(x), Data::Object(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| p.key == q.key && same(&p.value, &q.value)),
        (Data::Number(x), Data::Number(y)) => x.parse::<f64>().ok() == y.parse::<f64>().ok(),
        (x, y) => x == y,
    }
}

proptest! {
    #[test]
    fn json_survives_yaml_and_the_standard_xml_mapping(json in arb_value()) {
        let original = parse(&json, &Options::default()).root.expect("a value");
        let mut warnings = Vec::new();
        // JSON -> YAML -> value
        let yaml = yaml_text(&original, &mut warnings);
        let back = read_value(&yaml, Format::Yaml, &ConvertOptions::default()).map_err(|e| TestCaseError::fail(format!("{e} in {yaml:?}")))?;
        prop_assert!(same(&original, &back), "{json} -> {yaml:?}");
        // JSON -> compact JSON -> value
        let text = json_text(&original, None, &mut warnings).map_err(|e| TestCaseError::fail(e.to_string()))?;
        let again = parse(&text, &Options::default()).root.expect("a value");
        prop_assert!(same(&original, &again));
        // JSON -> standard XML -> value
        let options = ConvertOptions { standard_mapping: true, indent: None, ..Default::default() };
        let xml = convert(&json, Format::Json, Format::Xml, &options).map_err(|e| TestCaseError::fail(e.to_string()))?.text;
        let via_xml = convert(&xml, Format::Xml, Format::Json, &options).map_err(|e| TestCaseError::fail(format!("{e} in {xml}")))?.text;
        let via_xml = parse(&via_xml, &Options::default()).root.expect("a value");
        prop_assert!(same(&original, &via_xml), "{json} -> {xml}");
        // Pretty YAML/JSON/XML of any value is also a valid conversion that reads again.
        let pretty = convert(&json, Format::Json, Format::Json, &ConvertOptions::default()).map_err(|e| TestCaseError::fail(e.to_string()))?.text;
        prop_assert!(same(&original, &parse(&pretty, &Options::default()).root.expect("a value")));
    }

    #[test]
    fn any_text_converts_or_is_refused_without_a_panic(text in ".{0,120}", from in 0usize..4, to in 0usize..4) {
        let formats = [Format::Xml, Format::Json, Format::Yaml, Format::Bson];
        let _ = convert(&text, formats[from], formats[to], &ConvertOptions::default());
        let options = ConvertOptions { standard_mapping: true, ..Default::default() };
        let _ = convert(&text, formats[from], formats[to], &options);
    }

    #[test]
    fn data_of_any_shape_becomes_xml_that_is_well_formed(json in arb_value()) {
        let xml = convert(&json, Format::Json, Format::Xml, &ConvertOptions::default());
        if let Ok(done) = xml {
            let parsed = xmlcraft_syntax::parse(&done.text);
            prop_assert!(parsed.diagnostics().iter().all(|d| d.severity() != xmlcraft_syntax::Severity::Error), "{json} -> {}", done.text);
        }
    }
}
