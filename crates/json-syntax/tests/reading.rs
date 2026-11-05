//! Reading JSON, JSONC, JSON5 and JSON Lines: the tree, the places, the problems.

use proptest::prelude::*;
use xmlcraft_json_syntax::{Code, Data, Diagnostic, Dialect, Options, Segment, Value, parse, parse_lines};

fn opts(dialect: Dialect) -> Options {
    Options { dialect, ..Options::default() }
}

fn codes(text: &str, dialect: Dialect) -> Vec<&'static str> {
    parse(text, &opts(dialect)).diagnostics.iter().map(|d| d.code.key()).collect()
}

#[test]
fn a_document_is_read_into_values_that_know_their_places() {
    let text = "{\n  \"name\": \"x\\ny\",\n  \"n\": [1, -2.5e3, true, null],\n  \"o\": {}\n}\n";
    let read = parse(text, &Options::default());
    assert!(read.diagnostics.is_empty(), "{:?}", read.diagnostics);
    let root = read.root.expect("a value");
    assert_eq!(&text[root.span.clone()], "{\n  \"name\": \"x\\ny\",\n  \"n\": [1, -2.5e3, true, null],\n  \"o\": {}\n}");
    assert_eq!(root.get("name").map(|v| v.data.clone()), Some(Data::String("x\ny".to_owned())), "the escape is decoded");
    let n = root.get("n").expect("n");
    let Data::Array(items) = &n.data else { panic!("an array") };
    assert_eq!(items.iter().map(|v| &text[v.span.clone()]).collect::<Vec<_>>(), ["1", "-2.5e3", "true", "null"]);
    assert_eq!(root.pointer("/n/1").map(|v| v.data.clone()), Some(Data::Number("-2.5e3".to_owned())), "numbers are kept as written");
    assert_eq!(root.pointer("/n/9"), None);
    assert_eq!(root.pointer("/n/01"), None, "an index has no leading zero");
    assert_eq!(&text[root.span_of_pointer("/name").expect("a place")], "\"name\"", "a member is shown at its name");
    assert_eq!(&text[root.span_of_pointer("/n/2").expect("a place")], "true");
    let at = text.find("-2.5").expect("there") + 1;
    assert_eq!(root.path_at(at), Some(vec![Segment::Key("n".into()), Segment::Index(1)]));
    assert_eq!(root.path_at(text.find("\"o\"").expect("there") + 1), Some(vec![Segment::Key("o".into())]));
}

#[test]
fn escapes_and_surrogate_pairs_are_decoded_and_bad_ones_are_reported() {
    let read = parse(r#""aé😀\"\\\/""#, &Options::default());
    assert!(read.diagnostics.is_empty());
    assert_eq!(read.root.map(|v| v.data), Some(Data::String("aé😀\"\\/".to_owned())));
    assert_eq!(codes(r#""\x""#, Dialect::Json), ["json.invalid-escape"]);
    assert_eq!(codes(r#""\u12""#, Dialect::Json), ["json.invalid-escape"]);
    assert_eq!(codes("\"a\u{1}b\"", Dialect::Json), ["json.control-character"]);
    // A lone surrogate is a replacement character, not a crash.
    let lone = parse(r#""\ud83d""#, &Options::default());
    assert_eq!(lone.root.map(|v| v.data), Some(Data::String("\u{FFFD}".to_owned())));
    // JSON5 allows more escapes.
    assert_eq!(codes(r#"'\x41\v\0\q'"#, Dialect::Json5), Vec::<&str>::new());
    assert_eq!(parse(r#"'\x41'"#, &opts(Dialect::Json5)).root.map(|v| v.data), Some(Data::String("A".to_owned())));
}

#[test]
fn numbers_follow_the_grammar_of_the_dialect() {
    for ok in ["0", "-0", "12", "-1.5", "1e5", "1E+5", "0.5e-3"] {
        assert!(codes(ok, Dialect::Json).is_empty(), "{ok}");
    }
    for bad in ["01", "+1", ".5", "1.", "1e", "0x10", "--1", "1.2.3", "NaN", "Infinity"] {
        assert_eq!(codes(bad, Dialect::Json), ["json.invalid-number"], "{bad}");
    }
    for ok in ["+1", ".5", "5.", "0x1F", "Infinity", "-Infinity", "NaN"] {
        assert!(codes(ok, Dialect::Json5).is_empty(), "{ok}");
    }
    assert_eq!(codes("012", Dialect::Json5), ["json.invalid-number"], "no leading zeros in JSON5 either");
}

#[test]
fn what_json_does_not_allow_is_reported_and_the_relatives_allow_it() {
    let with_comment = "{ // note\n \"a\": 1 /* x */ }";
    assert_eq!(codes(with_comment, Dialect::Json), ["json.comment-not-allowed"], "reported once");
    assert!(codes(with_comment, Dialect::Jsonc).is_empty());
    assert!(codes(with_comment, Dialect::Json5).is_empty());
    assert_eq!(codes("[1, 2,]", Dialect::Json), ["json.trailing-comma"]);
    assert_eq!(codes("{\"a\": 1,}", Dialect::Json), ["json.trailing-comma"]);
    assert!(codes("[1, 2,]", Dialect::Jsonc).is_empty());
    assert!(codes("{\"a\": 1,}", Dialect::Json5).is_empty());
    assert_eq!(codes("{'a': 'b'}", Dialect::Json), ["json.single-quotes", "json.single-quotes"]);
    assert!(codes("{'a': 'b'}", Dialect::Json5).is_empty());
    assert_eq!(codes("{a: 1}", Dialect::Json), ["json.unquoted-key"]);
    assert!(codes("{a: 1, $b: 2, _c: 3}", Dialect::Json5).is_empty());
}

#[test]
fn broken_text_gives_a_tree_as_far_as_it_goes_and_problems_where_it_breaks() {
    assert_eq!(codes("{\"a\" 1}", Dialect::Json), ["json.expected-colon"]);
    assert_eq!(codes("{\"a\": 1 \"b\": 2}", Dialect::Json), ["json.expected-comma"]);
    assert_eq!(codes("[1 2]", Dialect::Json), ["json.expected-comma"]);
    assert_eq!(codes("{\"a\": }", Dialect::Json), ["json.expected-value"]);
    assert_eq!(codes("[1, , 2]", Dialect::Json), ["json.expected-value"]);
    assert_eq!(codes("{\"a\": 1", Dialect::Json), ["json.unclosed-object"]);
    assert_eq!(codes("[1, [2", Dialect::Json), ["json.unclosed-array", "json.unclosed-array"]);
    assert_eq!(codes("\"abc", Dialect::Json), ["json.unterminated-string"]);
    assert_eq!(codes("{\"a\": 1} 2", Dialect::Json), ["json.trailing-content"]);
    assert_eq!(codes("", Dialect::Json), ["json.empty-document"]);
    assert_eq!(codes("  \n ", Dialect::Json), ["json.empty-document"]);
    assert_eq!(codes("}", Dialect::Json), ["json.unexpected-token"]);
    assert_eq!(codes("{1: 2}", Dialect::Json), ["json.unquoted-key"]);
    // The part that was readable is there.
    let read = parse("{\"a\": 1, \"b\": [2, 3", &Options::default());
    let root = read.root.expect("a value");
    assert_eq!(root.pointer("/a").map(|v| v.data.clone()), Some(Data::Number("1".to_owned())));
    assert_eq!(root.pointer("/b/1").map(|v| v.data.clone()), Some(Data::Number("3".to_owned())));
    // A repeated name is a warning, not an error.
    let dup = parse("{\"a\": 1, \"a\": 2}", &Options::default());
    assert_eq!(
        dup.diagnostics.iter().map(|d: &Diagnostic| (d.code.key(), d.severity())).collect::<Vec<_>>(),
        [("json.duplicate-key", xmlcraft_json_syntax::Severity::Warning)]
    );
    assert_eq!(&"{\"a\": 1, \"a\": 2}"[dup.diagnostics[0].span.clone()], "\"a\"", "at the second name");
}

#[test]
fn nesting_and_the_number_of_problems_are_bounded() {
    let deep = format!("{}{}", "[".repeat(5000), "]".repeat(5000));
    let read = parse(&deep, &Options::default());
    assert!(read.diagnostics.iter().any(|d| d.code == Code::JsonTooDeep));
    assert!(read.root.is_some());
    let many = "[".to_owned() + &"1 ".repeat(5000) + "]";
    let read = parse(&many, &Options { max_diagnostics: 50, ..Options::default() });
    assert_eq!(read.diagnostics.len(), 51, "50 and the one that says there are more");
    assert_eq!(read.diagnostics.last().map(|d| d.code), Some(Code::TooManyDiagnostics));
}

#[test]
fn json_lines_are_read_line_by_line_at_their_places_in_the_text() {
    let text = "{\"a\": 1}\n\n[1,\n{\"b\": 2}\r\n";
    let lines = parse_lines(text, &Options::default());
    assert_eq!(lines.len(), 3, "the blank line is nothing");
    assert_eq!(&text[lines[0].0.clone()], "{\"a\": 1}");
    assert!(lines[0].1.diagnostics.is_empty());
    assert_eq!(&text[lines[2].0.clone()], "{\"b\": 2}");
    let root = lines[2].1.root.as_ref().expect("a value");
    assert_eq!(&text[root.span_of_pointer("/b").expect("a place")], "\"b\"", "places are those of the whole text");
    assert_eq!(lines[1].1.diagnostics.iter().map(|d| d.code.key()).collect::<Vec<_>>(), ["json.unclosed-array"]);
    assert_eq!(&text[lines[1].1.diagnostics[0].span.clone()], "[", "and so are the problems'");
}

/// A tree back into serde_json's, to compare what was read with what was written.
fn plain(value: &Value) -> serde_json::Value {
    match &value.data {
        Data::Null => serde_json::Value::Null,
        Data::Bool(b) => (*b).into(),
        Data::Number(n) => serde_json::from_str(n).unwrap_or(serde_json::Value::Null),
        Data::String(s) => s.clone().into(),
        Data::Array(items) => items.iter().map(plain).collect::<Vec<_>>().into(),
        Data::Object(members) => {
            let mut map = serde_json::Map::new();
            for m in members {
                map.insert(m.key.clone(), plain(&m.value));
            }
            serde_json::Value::Object(map)
        }
    }
}

fn spans_nest(value: &Value, text: &str) -> bool {
    let inside = |outer: &std::ops::Range<usize>, inner: &std::ops::Range<usize>| outer.start <= inner.start && inner.end <= outer.end;
    let ok = value.span.start <= value.span.end && text.get(value.span.clone()).is_some();
    ok && match &value.data {
        Data::Array(items) => items.iter().all(|v| inside(&value.span, &v.span) && spans_nest(v, text)),
        Data::Object(members) => members.iter().all(|m| {
            inside(&value.span, &m.key_span)
                && inside(&value.span, &m.value.span)
                && text.get(m.key_span.clone()).is_some()
                && spans_nest(&m.value, text)
        }),
        _ => true,
    }
}

fn arb_json() -> impl Strategy<Value = serde_json::Value> {
    let leaf = prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::from),
        any::<i64>().prop_map(serde_json::Value::from),
        (-1_000_000i32..1_000_000)
            .prop_map(|n| serde_json::Number::from_f64(f64::from(n) / 8.0).map_or(serde_json::Value::Null, serde_json::Value::Number)),
        ".{0,12}".prop_map(serde_json::Value::from),
    ];
    leaf.prop_recursive(4, 40, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..5).prop_map(serde_json::Value::from),
            prop::collection::hash_map("[a-z\"\\\\é ]{0,6}", inner, 0..5).prop_map(|m| serde_json::Value::Object(m.into_iter().collect())),
        ]
    })
}

proptest! {
    #[test]
    fn what_serde_json_writes_is_read_back_without_a_problem_and_equal(value in arb_json(), pretty in any::<bool>()) {
        let text = if pretty { serde_json::to_string_pretty(&value) } else { serde_json::to_string(&value) }.expect("writes");
        let read = parse(&text, &Options::default());
        prop_assert!(read.diagnostics.is_empty(), "{:?} in {text}", read.diagnostics);
        let root = read.root.expect("a value");
        prop_assert_eq!(plain(&root), value);
        prop_assert!(spans_nest(&root, &text));
    }

    #[test]
    fn any_text_is_read_without_a_panic_with_places_inside_the_text(text in ".{0,200}", dialect in 0u8..3) {
        let dialect = [Dialect::Json, Dialect::Jsonc, Dialect::Json5][usize::from(dialect)];
        let read = parse(&text, &Options { dialect, max_depth: 8, ..Options::default() });
        for d in &read.diagnostics {
            prop_assert!(d.span.start <= d.span.end && d.span.end <= text.len(), "{d:?}");
        }
        if let Some(root) = &read.root {
            prop_assert!(spans_nest(root, &text));
        }
        let _ = parse_lines(&text, &Options::default());
    }

    #[test]
    fn text_made_of_json_pieces_is_read_without_a_panic(pieces in prop::collection::vec(prop_oneof![Just("{"), Just("}"), Just("["), Just("]"), Just(","), Just(":"), Just("\""), Just("'"), Just("1"), Just("true"), Just("/*"), Just("*/"), Just("//"), Just("\\"), Just("\n"), Just(" "), Just("é")], 0..60)) {
        let text: String = pieces.concat();
        for dialect in [Dialect::Json, Dialect::Jsonc, Dialect::Json5] {
            let read = parse(&text, &Options { dialect, ..Options::default() });
            for d in &read.diagnostics {
                prop_assert!(d.span.end <= text.len());
            }
            if let Some(root) = &read.root {
                prop_assert!(spans_nest(root, &text), "{text:?}");
            }
        }
    }
}
