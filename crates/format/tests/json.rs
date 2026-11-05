//! Formatting JSON: only white space between tokens changes, and the result is stable.

use proptest::prelude::*;
use xmlcraft_format::json::{minify, pretty_print};
use xmlcraft_format::{BlankLines, Eol, FormatError, FormatOptions, Indent, apply};
use xmlcraft_json_syntax::Dialect;

fn options(indent: Indent) -> FormatOptions {
    FormatOptions { indent, eol: Eol::Lf, blank_lines: BlankLines::Keep }
}

fn pretty(text: &str, dialect: Dialect) -> String {
    apply(text, &pretty_print(text, dialect, &options(Indent::Spaces(2))).expect("formats"))
}

fn small(text: &str) -> String {
    apply(text, &minify(text, Dialect::Json).expect("minifies"))
}

#[test]
fn objects_and_arrays_get_a_line_per_member_and_empty_ones_stay_empty() {
    assert_eq!(
        pretty("{\"a\":1,\"b\":[1,2,{}],\"c\":[],\"d\":{\"e\":null}}", Dialect::Json),
        "{\n  \"a\": 1,\n  \"b\": [\n    1,\n    2,\n    {}\n  ],\n  \"c\": [],\n  \"d\": {\n    \"e\": null\n  }\n}"
    );
    assert_eq!(pretty("  [ ]  ", Dialect::Json), "[]  ", "leading space goes, the end of the text is left alone");
    assert_eq!(pretty("1", Dialect::Json), "1");
    let tabs = pretty_print("{\"a\":[1]}", Dialect::Json, &options(Indent::Tab)).expect("formats");
    assert_eq!(apply("{\"a\":[1]}", &tabs), "{\n\t\"a\": [\n\t\t1\n\t]\n}");
}

#[test]
fn a_formatted_text_gets_no_edits_and_what_is_not_white_space_never_changes() {
    let done = "{\n  \"a\": [\n    1,\n    \"x y\"\n  ],\n  \"b\": {}\n}\n";
    assert_eq!(pretty_print(done, Dialect::Json, &options(Indent::Spaces(2))), Ok(Vec::new()));
    // Strings keep their white space.
    assert_eq!(pretty("{\"a b\" :  \" c  d \"}", Dialect::Json), "{\n  \"a b\": \" c  d \"\n}");
}

#[test]
fn comments_stay_where_they_were_and_blank_lines_are_kept_once() {
    let text = "{ // top\n\"a\": 1, /* one */\n\n\n\"b\": 2 // two\n,\n/* alone */\n\"c\": [3, /* in */ 4]\n}";
    assert_eq!(
        pretty(text, Dialect::Jsonc),
        "{ // top\n  \"a\": 1, /* one */\n\n  \"b\": 2 // two\n  ,\n  /* alone */\n  \"c\": [\n    3, /* in */ 4\n  ]\n}"
    );
}

#[test]
fn the_line_ending_of_the_text_and_a_byte_order_mark_are_kept() {
    let crlf = pretty("{\"a\":1,\r\n\"b\":2}\r\n", Dialect::Json);
    assert_eq!(crlf, "{\r\n  \"a\": 1,\r\n  \"b\": 2\r\n}\r\n");
    let bom = pretty("\u{FEFF} {\"a\":1}", Dialect::Json);
    assert_eq!(bom, "\u{FEFF}{\n  \"a\": 1\n}");
    // A text with no line break is written with the option's ending.
    let edits = pretty_print("[1]", Dialect::Json, &FormatOptions { eol: Eol::CrLf, ..options(Indent::Spaces(2)) }).expect("formats");
    assert_eq!(apply("[1]", &edits), "[\r\n  1\r\n]");
}

#[test]
fn json5_and_unusual_values_are_formatted_like_the_rest() {
    assert_eq!(pretty("{a:'x',b:+1,c:Infinity,}", Dialect::Json5), "{\n  a: 'x',\n  b: +1,\n  c: Infinity,\n}");
}

#[test]
fn a_text_with_a_syntax_error_is_refused_with_the_first_problem() {
    for bad in ["{\"a\" 1}", "[1,", "{\"a\": 1,}", "", "[1] 2"] {
        let refused = pretty_print(bad, Dialect::Json, &options(Indent::Spaces(2)));
        assert!(matches!(refused, Err(FormatError::NotWellFormed { .. })), "{bad:?}");
        assert!(matches!(minify(bad, Dialect::Json), Err(FormatError::NotWellFormed { .. })), "{bad:?}");
    }
    // A trailing comma is fine where the dialect allows it.
    assert!(pretty_print("[1,]", Dialect::Jsonc, &options(Indent::Spaces(2))).is_ok());
}

#[test]
fn minifying_removes_all_white_space_between_tokens_but_the_line_break_that_ends_a_line_comment() {
    assert_eq!(small("{ \"a\" : [ 1 , 2 ] ,\n \"b\" : \"x y\" }\n"), "{\"a\":[1,2],\"b\":\"x y\"}\n");
    let jsonc = "{ // c\n \"a\": 1 /* d */ }";
    assert_eq!(apply(jsonc, &minify(jsonc, Dialect::Jsonc).expect("minifies")), "{// c\n\"a\":1/* d */}");
    let crlf = "{ // c\r\n \"a\": 1 }";
    assert_eq!(apply(crlf, &minify(crlf, Dialect::Jsonc).expect("minifies")), "{// c\r\n\"a\":1}");
}

#[test]
fn nesting_beyond_the_limit_is_indented_no_further_and_the_result_stays_bounded() {
    let deep = format!("{}1{}", "[".repeat(300), "]".repeat(300));
    let out = pretty(&deep, Dialect::Json);
    let widest = out.lines().map(|l| l.len() - l.trim_start().len()).max().unwrap_or(0);
    assert_eq!(widest, 64 * 2, "64 levels of two spaces and no more");
    // A text of many short items deep inside cannot be made to grow without bound: each item would carry the whole indent.
    let items = vec!["1"; 100_000].join(",");
    let wide = format!("{}{items}{}", "[".repeat(70), "]".repeat(70));
    let result = pretty_print(&wide, Dialect::Json, &options(Indent::Spaces(2)));
    assert!(matches!(result, Err(FormatError::Refused { .. })), "{:?}", result.map(|e| e.len()));
    // The same items near the top are fine.
    let shallow = format!("[{items}]");
    assert!(pretty_print(&shallow, Dialect::Json, &options(Indent::Spaces(2))).is_ok());
}

#[test]
fn blank_lines_can_be_removed_and_a_text_that_mixes_line_endings_is_formatted_once_and_for_all() {
    let text = "{\"a\": 1,\n\n\n\"b\": 2}";
    let remove = FormatOptions { blank_lines: BlankLines::Remove, ..options(Indent::Spaces(2)) };
    assert_eq!(apply(text, &pretty_print(text, Dialect::Json, &remove).expect("formats")), "{\n  \"a\": 1,\n  \"b\": 2\n}");
    // Blank lines go, and with them the votes of the ending they were written in: the ending most of the result uses is written.
    let comment = "[ /* a\r\nb\r\nc\r\nd\r\n*/\n\n\n\n\n1]";
    let once = apply(comment, &pretty_print(comment, Dialect::Jsonc, &remove).expect("formats"));
    assert_eq!(pretty_print(&once, Dialect::Jsonc, &remove), Ok(Vec::new()), "{once:?}");
    for mixed in ["[1,\r\n2,\n3,\r\n4]", "{\"a\":\n1,\r\n\"b\":\r\n2,\r\n\"c\":3}\n", "[\r1,\r\n2\n]"] {
        let once = pretty(mixed, Dialect::Json);
        assert_eq!(pretty_print(&once, Dialect::Json, &options(Indent::Spaces(2))), Ok(Vec::new()), "{mixed:?} -> {once:?}");
    }
}

#[test]
fn a_json_lines_line_with_a_unicode_space_is_not_blank() {
    use xmlcraft_format::json::minify_lines;
    assert!(minify_lines("{\"a\": 1}\n\u{a0}\n", Dialect::Json).is_err(), "a no-break space is no white space in JSON");
    assert!(minify_lines("{\"a\": 1}\n\u{a0}\n", Dialect::Json5).is_ok());
    assert!(minify_lines("{\"a\": 1}\n \t \n", Dialect::Json).is_ok());
}

fn arb_json() -> impl Strategy<Value = serde_json::Value> {
    let leaf = prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::from),
        any::<i64>().prop_map(serde_json::Value::from),
        ".{0,10}".prop_map(serde_json::Value::from),
    ];
    leaf.prop_recursive(4, 40, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..5).prop_map(serde_json::Value::from),
            prop::collection::btree_map("[a-z \"\\\\é]{0,6}", inner, 0..5).prop_map(|m| serde_json::Value::Object(m.into_iter().collect())),
        ]
    })
}

proptest! {
    #[test]
    fn pretty_printing_gives_what_serde_json_prints_and_minifying_what_it_prints_compactly(value in arb_json()) {
        let compact = serde_json::to_string(&value).expect("writes");
        let pretty_text = serde_json::to_string_pretty(&value).expect("writes");
        prop_assert_eq!(pretty(&compact, Dialect::Json), pretty_text.clone());
        prop_assert_eq!(small(&pretty_text), compact);
        // Stable: nothing left to do on the result.
        prop_assert_eq!(pretty_print(&pretty_text, Dialect::Json, &options(Indent::Spaces(2))), Ok(Vec::new()));
        prop_assert_eq!(minify(&serde_json::to_string(&value).expect("writes"), Dialect::Json), Ok(Vec::new()));
    }

    #[test]
    fn only_white_space_changes_and_the_result_is_stable_for_any_valid_text(
        pieces in prop::collection::vec(prop_oneof![Just("{"), Just("}"), Just("["), Just("]"), Just(","), Just(":"), Just("\"a\""), Just("1"), Just("true"), Just("/*c*/"), Just("//l\n"), Just(" "), Just("\n"), Just("\r\n"), Just("\t")], 0..40),
        dialect in 0u8..2,
    ) {
        let text: String = pieces.concat();
        let dialect = [Dialect::Jsonc, Dialect::Json5][usize::from(dialect)];
        let Ok(edits) = pretty_print(&text, dialect, &options(Indent::Spaces(2))) else { return Ok(()) };
        let result = apply(&text, &edits);
        // The tokens (every string, number, name and comment with its own white space) are the same, in the same order.
        let contents = |t: &str| xmlcraft_json_syntax::tokens(t, dialect, usize::MAX).0.iter().map(|tok| t[tok.span.clone()].to_owned()).collect::<Vec<_>>();
        prop_assert_eq!(contents(&result), contents(&text), "only the white space between tokens moved");
        let again = pretty_print(&result, dialect, &options(Indent::Spaces(2)));
        prop_assert!(again.as_ref().is_ok_and(Vec::is_empty), "{text:?} -> {result:?} -> {again:?}");
        let small = apply(&text, &minify(&text, dialect).expect("minifies"));
        prop_assert_eq!(contents(&small), contents(&text));
        prop_assert_eq!(minify(&small, dialect), Ok(Vec::new()));
    }
}

#[test]
fn json_lines_are_minified_line_by_line_with_the_edits_at_their_places() {
    use xmlcraft_format::json::minify_lines;
    let text = "{ \"a\" : 1 }\n\n  [ 1 , 2 ]\r\n{\"b\":  2}";
    let edits = minify_lines(text, Dialect::Json).expect("minifies");
    assert_eq!(apply(text, &edits), "{\"a\":1}\n\n[1,2]\r\n{\"b\":2}");
    let broken = "{\"a\": 1}\n{\"b\": }\n";
    let Err(FormatError::NotWellFormed { offset, .. }) = minify_lines(broken, Dialect::Json) else { panic!("a broken line is refused") };
    assert_eq!(&broken[offset..offset + 1], "}", "the problem is where it is in the whole text");
}
