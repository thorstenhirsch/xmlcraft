//! Re-indenting YAML: only the indentation of block structure, the indentation of block scalars and the white space at the ends of
//! lines change, and the parsed document stays the same.

use proptest::prelude::*;
use xmlcraft_format::yaml::pretty_print;
use xmlcraft_format::{Eol, FormatError, FormatOptions, Indent, IndentSequences, apply};
use xmlcraft_yaml_syntax::{Data, Options, Severity, parse};

fn options(unit: usize, style: IndentSequences) -> FormatOptions {
    FormatOptions { indent: Indent::Spaces(unit), eol: Eol::Lf, indent_sequences: style, ..FormatOptions::default() }
}

fn fmt_with(text: &str, unit: usize, style: IndentSequences) -> String {
    let edits = pretty_print(text, &options(unit, style)).unwrap_or_else(|e| panic!("{text:?} is refused: {e}"));
    apply(text, &edits)
}

fn fmt(text: &str) -> String {
    fmt_with(text, 2, IndentSequences::Keep)
}

/// The documents of a text as plain data (no places).
fn values(text: &str) -> String {
    let read = parse(text, &Options::default());
    assert!(read.diagnostics.iter().all(|d| d.severity() != Severity::Error), "{text:?}: {:?}", read.diagnostics);
    format!("{:?}", read.documents.iter().map(|v| strip(&v.data)).collect::<Vec<_>>())
}

fn strip(data: &Data) -> String {
    match data {
        Data::Array(items) => format!("[{}]", items.iter().map(|v| strip(&v.data)).collect::<Vec<_>>().join(",")),
        Data::Object(members) => {
            format!("{{{}}}", members.iter().map(|m| format!("{:?}:{}", m.key, strip(&m.value.data))).collect::<Vec<_>>().join(","))
        }
        other => format!("{other:?}"),
    }
}

/// Everything that is not white space, in order.
fn skeleton(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// The result is the same document, differs from the text only in white space, and formatting it again changes nothing.
fn check(text: &str, unit: usize, style: IndentSequences) -> String {
    let out = fmt_with(text, unit, style);
    assert_eq!(values(&out), values(text), "the value changed: {text:?} -> {out:?}");
    assert_eq!(skeleton(&out), skeleton(text), "more than white space changed: {text:?} -> {out:?}");
    let again = fmt_with(&out, unit, style);
    assert_eq!(again, out, "not idempotent for {text:?}");
    out
}

#[test]
fn nested_mappings_and_sequences_take_the_indent_unit() {
    let text = "a:\n    b:\n        c: 1\n        d: 2\n    e: 3\nf:\n  - x\n  - y: 1\n    z: 2\n";
    assert_eq!(check(text, 2, IndentSequences::Keep), "a:\n  b:\n    c: 1\n    d: 2\n  e: 3\nf:\n  - x\n  - y: 1\n    z: 2\n");
    assert_eq!(check(text, 4, IndentSequences::Keep), "a:\n    b:\n        c: 1\n        d: 2\n    e: 3\nf:\n    - x\n    - y: 1\n      z: 2\n");
}

#[test]
fn a_formatted_text_gets_no_edits() {
    let done = "a:\n  b: 1\n  c:\n    - x\n    - y\nd: # note\n  - 1\n";
    assert_eq!(pretty_print(done, &FormatOptions::default()), Ok(Vec::new()));
}

#[test]
fn sequences_under_a_key_keep_their_style_unless_asked() {
    let text = "a:\n- x\n- y\nb:\n    - z\n";
    assert_eq!(check(text, 2, IndentSequences::Keep), "a:\n- x\n- y\nb:\n  - z\n");
    assert_eq!(check(text, 2, IndentSequences::Indent), "a:\n  - x\n  - y\nb:\n  - z\n");
    assert_eq!(check(text, 2, IndentSequences::Flush), "a:\n- x\n- y\nb:\n- z\n");
    // Inside a sequence entry too.
    let nested = "- k:\n  - 1\n  - 2\n  m: 3\n";
    assert_eq!(check(nested, 2, IndentSequences::Indent), "- k:\n    - 1\n    - 2\n  m: 3\n");
}

#[test]
fn comments_move_with_the_structure_they_are_written_for() {
    let text = "# top\na:\n      # about b\n      b: 1   # trailing\n      # between\n      c: 2\n    # deeper\n# last\nd: 1\n";
    assert_eq!(check(text, 2, IndentSequences::Keep), "# top\na:\n  # about b\n  b: 1   # trailing\n  # between\n  c: 2\n  # deeper\n# last\nd: 1\n");
    let items = "l:\n    # first\n    - a\n    # second\n    - b\n";
    assert_eq!(check(items, 2, IndentSequences::Keep), "l:\n  # first\n  - a\n  # second\n  - b\n");
}

#[test]
fn block_scalars_are_re_indented_as_a_whole() {
    let text = "a:\n      b: |\n          line one\n            indented more\n\n          line two\n      c: >-\n          folded\n          text\n      d: 1\n";
    assert_eq!(
        check(text, 2, IndentSequences::Keep),
        "a:\n  b: |\n    line one\n      indented more\n\n    line two\n  c: >-\n    folded\n    text\n  d: 1\n"
    );
    // Keep chomping keeps the blank lines at the end.
    let keep = "a:\n    b: |+\n        x\n\n    c: 1\n";
    assert_eq!(check(keep, 2, IndentSequences::Keep), "a:\n  b: |+\n    x\n\n  c: 1\n");
    // An indentation indicator moves with its parent.
    let explicit = "a:\n    b: |2\n          x\n";
    assert_eq!(check(explicit, 2, IndentSequences::Keep), "a:\n  b: |2\n        x\n", "(the content has four spaces of its own)");
    let in_seq = "-   |\n       text\n-   - |-\n         deep\n";
    check(in_seq, 2, IndentSequences::Keep);
    check("--- |\n x\n--- >\n   y\n", 2, IndentSequences::Keep);
}

#[test]
fn flow_collections_and_multiline_scalars_move_with_their_line() {
    let text = "a:\n      b: [1,\n          2, {x: 1,\n              y: 2}]\n      c: plain text\n        that goes on\n      d: \"quoted\n        text\"\n      e: 'single\n        text'\n";
    let out = check(text, 2, IndentSequences::Keep);
    assert_eq!(
        out,
        "a:\n  b: [1,\n      2, {x: 1,\n          y: 2}]\n  c: plain text\n    that goes on\n  d: \"quoted\n    text\"\n  e: 'single\n    text'\n"
    );
    // Trailing white space inside a multi-line scalar is not touched; after it, it goes.
    assert_eq!(fmt("a: \"x  \n  y\"   \nb: 1\n"), "a: \"x  \n  y\"\nb: 1\n");
}

#[test]
fn anchors_aliases_tags_and_quoting_are_left_alone() {
    let text = "base: &b\n    x: 1\n    y: !!str 2\nuse:\n    <<: *b\n    z: \"q: r\"\n    'k k': 'it''s'\nlist: &l\n    - 1\n    - *b\nseq: !!seq\n    - a\n";
    assert_eq!(
        check(text, 2, IndentSequences::Keep),
        "base: &b\n  x: 1\n  y: !!str 2\nuse:\n  <<: *b\n  z: \"q: r\"\n  'k k': 'it''s'\nlist: &l\n  - 1\n  - *b\nseq: !!seq\n  - a\n"
    );
}

#[test]
fn complex_keys_and_compact_nesting() {
    let text = "?   - a\n    - b\n:   value\n? [x, y]\n: z\nk:\n    ? q\n    : r\n-1: x\n";
    let out = check(text, 2, IndentSequences::Keep);
    assert!(out.starts_with("?   - a\n    - b\n:   value\n"), "{out}");
    let seqs = "- - a\n  - b\n-     - c\n      - d\n-   k: v\n    w: x\n";
    check(seqs, 2, IndentSequences::Keep);
    check(seqs, 4, IndentSequences::Indent);
}

#[test]
fn multi_document_streams_are_each_handled() {
    let text = "%YAML 1.2\n---\na:\n      b: 1\n...\n---\n    - x   \n    -   y\n--- # note\nk:\n   - 1\n";
    let out = check(text, 2, IndentSequences::Keep);
    assert_eq!(out, "%YAML 1.2\n---\na:\n  b: 1\n...\n---\n- x\n-   y\n--- # note\nk:\n  - 1\n");
}

#[test]
fn trailing_white_space_goes_and_the_final_newline_is_one() {
    assert_eq!(fmt("a: 1   \nb:\t\n  \n\nc: 2 \t\n"), "a: 1\nb:\n\n\nc: 2\n");
    assert_eq!(fmt("a: 1\n\n\n\n"), "a: 1\n");
    assert_eq!(fmt("a: 1\n   \n  "), "a: 1\n");
    assert_eq!(fmt("a: 1"), "a: 1", "no final newline is added");
    // Blank lines at the end of a keep-chomped block scalar are its value.
    assert_eq!(fmt("a: |+\n  x\n\n\n"), "a: |+\n  x\n\n\n");
}

#[test]
fn line_endings_and_a_byte_order_mark_survive() {
    let text = "\u{FEFF}a:\r\n      b: 1  \r\n      c:\r\n          - x\r\n";
    let out = check(text, 2, IndentSequences::Keep);
    assert_eq!(out, "\u{FEFF}a:\r\n  b: 1\r\n  c:\r\n    - x\r\n");
    let mixed = "a:\n      b: 1\r\n      c: 2\n";
    assert_eq!(fmt(mixed), "a:\n  b: 1\r\n  c: 2\n");
}

#[test]
fn tabs_inside_values_stay_and_a_tab_as_indentation_is_refused() {
    let text = "a:\n      b: x\ty\n      c: \"p\tq\"\n";
    assert_eq!(check(text, 2, IndentSequences::Keep), "a:\n  b: x\ty\n  c: \"p\tq\"\n");
    assert!(pretty_print("a:\n  \tb: 1\n", &FormatOptions::default()).is_err());
}

#[test]
fn multi_line_plain_scalars_and_comments_between_items() {
    let text = "list:\n      - first line\n        second line\n      # between\n      - item\n        with more\n      -\n        key: v\n";
    check(text, 2, IndentSequences::Keep);
    check(text, 3, IndentSequences::Indent);
}

#[test]
fn text_that_is_not_yaml_is_refused_and_untouched() {
    assert!(matches!(pretty_print("a: [1\nb: 2\n", &FormatOptions::default()), Err(FormatError::NotWellFormed { .. })));
    assert!(matches!(pretty_print("a:\n  b: 1\n c: 2\n", &FormatOptions::default()), Err(FormatError::NotWellFormed { .. })));
    assert_eq!(pretty_print("", &FormatOptions::default()), Ok(Vec::new()));
    assert_eq!(pretty_print("# only a comment\n", &FormatOptions::default()), Ok(Vec::new()));
}

#[test]
fn very_deep_and_very_wide_texts_do_not_crash() {
    let deep: String = (0..200).map(|i| format!("{}k{i}:\n", " ".repeat(i))).collect::<String>() + &" ".repeat(200) + "v: 1\n";
    eprintln!("deep");
    let _ = pretty_print(&deep, &options(16, IndentSequences::Keep));
    let nested = "- ".repeat(3000) + "x\n";
    eprintln!("nested");
    let _ = pretty_print(&nested, &options(16, IndentSequences::Keep));
    eprintln!("wide");
    let wide: String = (0..20000).map(|i| format!("k{i}: {i}\n")).collect();
    assert_eq!(pretty_print(&wide, &FormatOptions::default()), Ok(Vec::new()));
}

/// Inputs that are awkward for a re-indenter: none is refused, and each keeps its value and is stable.
#[test]
fn a_regression_suite_of_tricky_inputs() {
    let inputs = [
        // Multi-line plain scalars in all places.
        "a: one
    two
    three
b:
  - x
    y
  - z
",
        "? a long key
  that goes on
: v
",
        // Quoted keys and values.
        "\"a b\":   1\n'c d':\n      \"e\": 2\n",
        // Anchors on sequences, mappings and scalars.
        "s: &s\n  - 1\n  - &i 2\nm: &m\n      k: *i\nt: *s\n",
        // Comments everywhere.
        "# c1\na: # c2\n      # c3\n      - 1 # c4\n      # c5\nb: 2\n# c6\n",
        "a:\n      b:\n            c: 1\n      # about d\n      d: 2\n   # odd\n",
        // Tabs inside scalars and after indicators.
        "a: 1\tx\nb: \"p\tq\"\n",
        "- \tx\n-\ty\n",
        // CRLF everywhere, with a block scalar.
        "a:\r\n      b: |\r\n            x\r\n              y\r\n      c: 1\r\n",
        // Block scalars of every kind of header.
        "a: |-\n    x\nb: >+\n    y\n\nc: |4-\n      z\nd: >\n    # not a comment\n    w\n",
        "- |\n   x\n- >\n   y\n-   key: |\n         z\n",
        // An empty block scalar and empty values.
        "a: |\nb:\nc: >\n",
        // Flow collections spanning lines under indented keys.
        "k:\n      - [a,\n         b]\n      - {c: d,\n         e: f}\n",
        // Strings that look like structure.
        "a: \"- not a seq\"\nb: 'k: v'\nc: x # y\nd: \"#no\"\n",
        // A key that is a long dash or marker-like text.
        "--- a\n--- b\n...\n",
        "a: ---\nb: ...\n",
        // Deeply mixed nesting.
        "a:\n  - b:\n      - c:\n          d: 1\n      - - e\n        - f\n  -\n    g: 1\n",
        // Comments between sequence items in a compact sequence.
        "a:\n- 1\n# between\n- 2\n  # deeper\n- 3\nb: 1\n",
        // Windows-style document starts and unusual spacing after dashes.
        "-   a\n-     b: 1\n      c: 2\n",
        // Unicode.
        "名前:\n        値: \"日本語\"\n",
        // Only comments and markers.
        "# x\n---\n# y\n...\n",
    ];
    let mut failures = Vec::new();
    for input in inputs {
        let read = parse(input, &Options::default());
        assert!(read.diagnostics.iter().all(|d| d.severity() != Severity::Error), "the input is not YAML: {input:?}");
        for style in [IndentSequences::Keep, IndentSequences::Indent, IndentSequences::Flush] {
            for unit in [1, 2, 4] {
                if let Err(why) = std::panic::catch_unwind(|| check(input, unit, style)) {
                    let why = why.downcast_ref::<String>().cloned().unwrap_or_default();
                    failures.push(format!("{input:?} (unit {unit}, {style:?}): {why}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// A generator of valid YAML written in many styles.

fn scalar() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("1".to_owned()),
        Just("plain".to_owned()),
        Just("two words".to_owned()),
        Just("\"quoted: text\"".to_owned()),
        Just("'single # not a comment'".to_owned()),
        Just("null".to_owned()),
        Just("[1, 2, {a: b}]".to_owned()),
        Just("!!str 12".to_owned()),
        Just("&x value".to_owned()),
    ]
}

#[derive(Clone, Debug)]
enum Node {
    Scalar(String),
    Block(bool, String),
    Multiline(String),
    Map(Vec<(String, Node)>),
    Seq(Vec<Node>),
}

fn node() -> impl Strategy<Value = Node> {
    let leaf = prop_oneof![
        scalar().prop_map(Node::Scalar),
        (any::<bool>(), "[a-z ]{1,8}").prop_map(|(literal, t)| Node::Block(literal, t)),
        "[a-z]{1,5}".prop_map(Node::Multiline),
    ];
    leaf.prop_recursive(4, 24, 4, |inner| {
        prop_oneof![
            prop::collection::vec(("[a-z]{1,4}", inner.clone()), 1..4).prop_map(Node::Map),
            prop::collection::vec(inner, 1..4).prop_map(Node::Seq),
        ]
    })
}

type Noise = std::iter::Cycle<std::vec::IntoIter<u8>>;

/// A collection or scalar starting on a fresh line at `col`, with `step` spaces per level and sequences under keys flush if `flush`.
fn entries(node: &Node, col: usize, step: usize, flush: bool, noise: &mut Noise, out: &mut String) {
    let pad = " ".repeat(col);
    match node {
        Node::Map(members) => {
            for (k, v) in members {
                match noise.next().unwrap_or(0) % 5 {
                    0 => out.push_str(&format!("{pad}# a comment\n")),
                    1 => out.push('\n'),
                    _ => {}
                }
                out.push_str(&format!("{pad}{k}:"));
                value(v, true, col, step, flush, noise, out);
            }
        }
        Node::Seq(items) => {
            for item in items {
                if noise.next().unwrap_or(0).is_multiple_of(4) {
                    out.push_str(&format!("{pad}# item\n"));
                }
                out.push_str(&format!("{pad}-"));
                match item {
                    Node::Map(_) | Node::Seq(_) if noise.next().unwrap_or(0).is_multiple_of(2) => {
                        // Compact: the first entry on the dash's line.
                        let gap = 1 + usize::from(noise.next().unwrap_or(0) % 3);
                        let mut inner = String::new();
                        entries(item, col + 1 + gap, step, flush, noise, &mut inner);
                        let trimmed = inner.trim_start_matches(' ');
                        if trimmed.starts_with('#') || trimmed.starts_with('\n') {
                            out.push('\n');
                            out.push_str(&inner);
                        } else {
                            out.push_str(&" ".repeat(gap));
                            out.push_str(trimmed);
                        }
                    }
                    _ => value(item, false, col, step, flush, noise, out),
                }
            }
        }
        other => {
            out.push_str(&pad);
            leaf(other, col, noise, out);
        }
    }
}

/// What follows `key:` (if `under_key`) or `-` at `col`.
fn value(node: &Node, under_key: bool, col: usize, step: usize, flush: bool, noise: &mut Noise, out: &mut String) {
    let trailing = if noise.next().unwrap_or(0).is_multiple_of(4) { "  " } else { "" };
    match node {
        Node::Map(_) => {
            out.push_str(trailing);
            out.push('\n');
            entries(node, col + step, step, flush, noise, out);
        }
        Node::Seq(_) => {
            out.push_str(trailing);
            out.push('\n');
            entries(node, if flush && under_key { col } else { col + step }, step, flush, noise, out);
        }
        scalar => {
            out.push(' ');
            leaf(scalar, col, noise, out);
        }
    }
}

fn leaf(node: &Node, col: usize, noise: &mut Noise, out: &mut String) {
    match node {
        Node::Scalar(s) => {
            out.push_str(s);
            if noise.next().unwrap_or(0).is_multiple_of(5) {
                out.push_str("   # c");
            }
            out.push('\n');
        }
        Node::Multiline(w) => out.push_str(&format!("{w}\n{}{w} more\n", " ".repeat(col + 2))),
        Node::Block(literal, t) => {
            let base = " ".repeat(col + 2 + usize::from(noise.next().unwrap_or(0) % 3));
            out.push_str(if *literal { "|\n" } else { ">\n" });
            out.push_str(&format!("{base}{t}\n{base}  deeper\n\n{base}{t}\n"));
        }
        _ => out.push('\n'),
    }
}

const PIECES: &[&str] = &[
    "a: 1",
    "b:",
    "- x",
    "- y: 1",
    "-",
    "# note",
    "k: |",
    "k: >-",
    "text",
    "more text",
    "[1, 2]",
    "{a: b}",
    "? q",
    ": r",
    "---",
    "...",
    "&x v",
    "key: *x",
    "\"quoted",
    "end\"",
    "a: [1,",
    "2]",
    "'s'",
    "- - n",
    "!!str 1",
];

proptest! {
    #![proptest_config(ProptestConfig { cases: 500, max_global_rejects: 100_000, ..ProptestConfig::default() })]

    #[test]
    fn generated_documents_keep_their_value_and_formatting_is_idempotent(
        tree in node(),
        step in 1usize..6,
        flush in any::<bool>(),
        unit in 1usize..6,
        style in 0usize..3,
        noise in prop::collection::vec(any::<u8>(), 1..16),
    ) {
        let mut cycle = noise.into_iter().cycle();
        let mut text = String::new();
        entries(&tree, 0, step, flush, &mut cycle, &mut text);
        let read = parse(&text, &Options::default());
        prop_assume!(read.diagnostics.iter().all(|d| d.severity() != Severity::Error));
        let style = [IndentSequences::Keep, IndentSequences::Indent, IndentSequences::Flush][style];
        let out = check(&text, unit, style);
        prop_assert!(!out.is_empty());
    }

    #[test]
    fn any_text_is_either_refused_or_keeps_its_value(lines in prop::collection::vec(("[ ]{0,6}", prop::sample::select(PIECES.to_vec()), "[ ]{0,2}"), 0..10)) {
        let text: String = lines.iter().map(|(lead, piece, trail)| format!("{lead}{piece}{trail}\n")).collect();
        let read = parse(&text, &Options::default());
        prop_assume!(read.diagnostics.iter().all(|d| d.severity() != Severity::Error));
        if let Ok(edits) = pretty_print(&text, &FormatOptions::default()) {
            let out = apply(&text, &edits);
            prop_assert_eq!(values(&out), values(&text));
            prop_assert_eq!(skeleton(&out), skeleton(&text));
        }
    }
}
