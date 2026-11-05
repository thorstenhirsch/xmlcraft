//! Reading YAML: the tree, the places, the problems.

use proptest::prelude::*;
use xmlcraft_yaml_syntax::{Code, Data, Options, Value, parse};

fn first(text: &str) -> Value {
    parse(text, &Options::default()).documents.into_iter().next().expect("a document")
}

fn codes(text: &str) -> Vec<&'static str> {
    parse(text, &Options::default()).diagnostics.iter().map(|d| d.code.key()).collect()
}

#[test]
fn a_document_is_read_into_values_that_know_their_places() {
    let text = "name: x\nitems:\n  - 1\n  - é: [true, ~]\nmulti: |\n  a\n  b\nq: 'it''s'\n";
    let read = parse(text, &Options::default());
    assert!(read.diagnostics.is_empty(), "{:?}", read.diagnostics);
    let root = &read.documents[0];
    assert_eq!(root.pointer("/name").map(|v| v.data.clone()), Some(Data::String("x".into())));
    assert_eq!(root.pointer("/items/0").map(|v| v.data.clone()), Some(Data::Number("1".into())));
    assert_eq!(root.pointer("/items/1/é/0").map(|v| v.data.clone()), Some(Data::Bool(true)));
    assert_eq!(root.pointer("/items/1/é/1").map(|v| v.data.clone()), Some(Data::Null));
    assert_eq!(root.pointer("/multi").map(|v| v.data.clone()), Some(Data::String("a\nb\n".into())));
    assert_eq!(root.pointer("/q").map(|v| v.data.clone()), Some(Data::String("it's".into())));
    let at = |pointer: &str| &text[root.span_of_pointer(pointer).expect("a place")];
    assert_eq!(at("/name"), "name", "a member is shown at its key");
    assert_eq!(at("/items/0"), "1");
    assert_eq!(at("/items/1/é"), "é", "byte offsets, not characters");
    assert_eq!(at("/items/1/é/0"), "true");
}

#[test]
fn scalars_follow_the_core_schema() {
    let v = first(
        "a: 12\nb: -1.5e3\nc: 0x1F\nd: 0o17\ne: .inf\nf: -.inf\ng: .nan\nh: Null\ni: TRUE\nj: yes\nk: '12'\nl: \"true\"\nm: !!str 12\nn: 1_000\no: 1.\np: +5\nq: 012\n",
    );
    let get = |k: &str| v.pointer(&format!("/{k}")).map(|v| v.data.clone()).expect(k);
    assert_eq!(get("a"), Data::Number("12".into()));
    assert_eq!(get("b"), Data::Number("-1.5e3".into()));
    assert_eq!(get("c"), Data::Number("31".into()));
    assert_eq!(get("d"), Data::Number("15".into()));
    assert_eq!(get("e"), Data::Number("Infinity".into()));
    assert_eq!(get("f"), Data::Number("-Infinity".into()));
    assert_eq!(get("g"), Data::Number("NaN".into()));
    assert_eq!(get("h"), Data::Null);
    assert_eq!(get("i"), Data::Bool(true));
    assert_eq!(get("j"), Data::String("yes".into()), "YAML 1.2: yes is a string");
    assert_eq!(get("k"), Data::String("12".into()));
    assert_eq!(get("l"), Data::String("true".into()));
    assert_eq!(get("m"), Data::String("12".into()));
    assert_eq!(get("n"), Data::String("1_000".into()));
    assert_eq!(get("o"), Data::Number("1.".into()));
    assert_eq!(get("p"), Data::Number("5".into()));
    assert_eq!(get("q"), Data::Number("12".into()), "012 is twelve in the core schema, written as JSON writes it");
}

#[test]
fn aliases_become_copies_placed_at_the_alias_and_merge_keys_are_applied() {
    let text = "base: &b {x: 1, y: 2}\ncopy: *b\nmerged:\n  <<: *b\n  y: 9\n  z: 3\n";
    let read = parse(text, &Options::default());
    assert!(read.diagnostics.is_empty(), "{:?}", read.diagnostics);
    let root = &read.documents[0];
    assert_eq!(root.pointer("/copy/x").map(|v| v.data.clone()), Some(Data::Number("1".into())));
    assert_eq!(&text[root.span_of_pointer("/copy/x").expect("a place")], "*b", "every place of the copy is the alias");
    let merged = root.pointer("/merged").expect("merged");
    let Data::Object(members) = &merged.data else { panic!("an object") };
    let shown: Vec<_> = members.iter().map(|m| (m.key.as_str(), m.value.data.clone())).collect();
    assert_eq!(shown, [("y", Data::Number("9".into())), ("z", Data::Number("3".into())), ("x", Data::Number("1".into()))], "own members win");
    // A list of mappings, and a merge of something else.
    let v = first("a: &a {p: 1}\nb: &b {q: 2}\nc:\n  <<: [*a, *b]\n");
    assert!(v.pointer("/c/p").is_some() && v.pointer("/c/q").is_some());
    assert_eq!(codes("a:\n  <<: 5\n"), ["yaml.bad-merge"]);
}

#[test]
fn what_cannot_be_read_is_reported_and_the_readable_part_stays() {
    assert_eq!(codes("a: *nope\n"), ["yaml.syntax-error"], "the parser knows the anchors");
    assert_eq!(codes("a: 1\na: 2\n"), ["yaml.duplicate-key"]);
    let bad = "ok: 1\nlist: [1, 2\nnext: 3\n";
    let read = parse(bad, &Options::default());
    assert_eq!(read.diagnostics.iter().map(|d| d.code.key()).collect::<Vec<_>>(), ["yaml.syntax-error"]);
    let root = read.documents.first().expect("the part that was read");
    assert_eq!(root.pointer("/ok").map(|v| v.data.clone()), Some(Data::Number("1".into())));
    assert!(read.diagnostics[0].span.start <= bad.len());
    assert_eq!(codes("a:\n\t- x\n"), ["yaml.syntax-error"], "a tab cannot indent");
    assert!(codes("").is_empty() && parse("", &Options::default()).documents.is_empty(), "an empty stream is valid and has no document");
}

#[test]
fn places_are_right_whatever_the_line_breaks_are() {
    for text in ["a: 1\nb: 2\nc: é\n", "a: 1\r\nb: 2\r\nc: é\r\n", "a: 1\rb: 2\rc: é\r"] {
        let read = parse(text, &Options::default());
        let root = &read.documents[0];
        for (key, shown) in [("a", "a"), ("b", "b"), ("c", "c")] {
            assert_eq!(&text[root.span_of_pointer(&format!("/{key}")).expect("a place")], shown, "{text:?}");
        }
    }
    // Far into a long text, with characters of several bytes before it.
    let long = format!("{}k: v\n", "é: 1\n".repeat(3000));
    let read = parse(&long, &Options::default());
    let root = &read.documents[read.documents.len() - 1];
    assert_eq!(&long[root.span_of_pointer("/k").expect("a place")], "k");
}

#[test]
fn an_alias_to_a_value_dropped_for_being_too_deep_is_not_called_unknown() {
    let text = "a: [[[[&deep x]]]]\nb: *deep\n";
    let read = parse(text, &Options { max_depth: 2, ..Options::default() });
    assert!(read.diagnostics.iter().any(|d| d.code == Code::YamlTooDeep));
    assert!(read.diagnostics.iter().all(|d| !d.message().contains("not available")), "{:?}", read.diagnostics);
}

#[test]
fn a_stream_may_have_several_documents() {
    let read = parse("a: 1\n---\nb: 2\n...\n---\n- 3\n", &Options::default());
    assert_eq!(read.documents.len(), 3);
    assert!(read.documents[2].pointer("/0").is_some());
}

#[test]
fn aliases_cannot_blow_up_and_nesting_and_problems_are_bounded() {
    // The billion laughs: each level has ten of the one before.
    let mut text = String::from("a: &a0 [x, x, x, x, x, x, x, x, x, x]\n");
    for i in 1..12 {
        let prev = format!("*a{}", i - 1);
        text.push_str(&format!("a{i}: &a{i} [{}]\n", vec![prev; 10].join(", ")));
    }
    let read = parse(&text, &Options { max_nodes: 5_000, ..Options::default() });
    // The values the text spells out do not use the budget: one alias of one value fits in a budget of one.
    let small = parse("a: &x 1\nb: *x\n", &Options { max_nodes: 1, ..Options::default() });
    assert!(small.diagnostics.is_empty(), "{:?}", small.diagnostics);
    assert!(read.diagnostics.iter().any(|d| d.code == Code::YamlAliasTooLarge), "{:?}", read.diagnostics);
    fn size(v: &Value) -> usize {
        1 + match &v.data {
            Data::Array(items) => items.iter().map(size).sum(),
            Data::Object(ms) => ms.iter().map(|m| size(&m.value)).sum(),
            _ => 0,
        }
    }
    assert!(read.documents.iter().map(size).sum::<usize>() < 60_000);
    let deep = format!("{}1{}", "[".repeat(100), "]".repeat(100));
    let read = parse(&deep, &Options { max_depth: 50, ..Options::default() });
    assert!(read.diagnostics.iter().any(|d| d.code == Code::YamlTooDeep));
    let many = "a: 1\n".repeat(5000);
    let read = parse(&many, &Options { max_diagnostics: 20, ..Options::default() });
    assert_eq!(read.diagnostics.len(), 21, "the cap and the one that says there are more");
    assert_eq!(read.diagnostics.last().map(|d| d.code), Some(Code::TooManyDiagnostics));
}

fn spans_inside(v: &Value, len: usize) -> bool {
    v.span.start <= v.span.end
        && v.span.end <= len
        && match &v.data {
            Data::Array(items) => items.iter().all(|i| spans_inside(i, len)),
            Data::Object(ms) => ms.iter().all(|m| m.key_span.end <= len && m.key_span.start <= m.key_span.end && spans_inside(&m.value, len)),
            _ => true,
        }
}

proptest! {
    #[test]
    fn any_text_is_read_without_a_panic_with_places_inside_the_text(text in ".{0,200}") {
        let read = parse(&text, &Options { max_depth: 16, max_nodes: 10_000, ..Options::default() });
        for d in &read.diagnostics {
            prop_assert!(d.span.start <= d.span.end && d.span.end <= text.len(), "{d:?}");
            prop_assert!(text.is_char_boundary(d.span.start) && text.is_char_boundary(d.span.end), "{d:?} in {text:?}");
        }
        for doc in &read.documents {
            prop_assert!(spans_inside(doc, text.len()), "{text:?}");
        }
    }

    #[test]
    fn text_made_of_yaml_pieces_is_read_without_a_panic(pieces in prop::collection::vec(prop_oneof![Just("a"), Just(": "), Just("- "), Just("\n"), Just("  "), Just("["), Just("]"), Just("{"), Just("}"), Just(","), Just("&x "), Just("*x"), Just("|\n"), Just(">\n"), Just("'"), Just("\""), Just("#"), Just("---\n"), Just("é"), Just("!!str "), Just("? "), Just("<<: "), Just("\t")], 0..50)) {
        let text: String = pieces.concat();
        let read = parse(&text, &Options { max_depth: 16, max_nodes: 10_000, ..Options::default() });
        for d in &read.diagnostics {
            prop_assert!(d.span.end <= text.len() && text.is_char_boundary(d.span.start) && text.is_char_boundary(d.span.end), "{d:?} in {text:?}");
        }
        for doc in &read.documents {
            prop_assert!(spans_inside(doc, text.len()), "{text:?}");
        }
    }
}
