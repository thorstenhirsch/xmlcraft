//! The formatter on random well-formed documents: it never changes anything but white space between nodes, it is idempotent, it leaves
//! mixed content and `xml:space="preserve"` alone, and it keeps the document well-formed.

use proptest::prelude::*;
use xmlcraft_format::{BlankLines, FormatOptions, Indent, apply, minify, pretty_print};
use xmlcraft_syntax::{SyntaxKind, parse};
use xmlcraft_text::eol::Eol;

fn space() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        Just(" ".to_owned()),
        Just("\n".to_owned()),
        Just("\r\n  ".to_owned()),
        Just("\n\n\n\t".to_owned()),
        Just("   ".to_owned())
    ]
}

fn name() -> impl Strategy<Value = &'static str> {
    prop_oneof![Just("a"), Just("b"), Just("c"), Just("p:d")]
}

fn leaf() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("<e/>".to_owned()),
        Just("<!-- c  c -->".to_owned()),
        Just("<?pi x ?>".to_owned()),
        Just("text".to_owned()),
        Just(" spaced text ".to_owned()),
        Just("<![CDATA[ x < y ]]>".to_owned()),
        Just("&amp;".to_owned()),
        Just("&#65;".to_owned()),
        Just("é日".to_owned()),
    ]
}

fn attrs() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => Just(String::new()),
        1 => Just(" x=\"1\"".to_owned()),
        1 => Just(" xml:space=\"preserve\"".to_owned()),
        1 => Just(" xml:space='default' y='2'".to_owned()),
    ]
}

/// An element: a name, attributes, and children each followed by some white space.
fn element() -> impl Strategy<Value = String> {
    leaf().prop_recursive(5, 40, 5, |inner| {
        (name(), attrs(), space(), proptest::collection::vec((inner, space()), 0..5), space()).prop_map(|(name, attrs, first, children, last)| {
            let mut out = format!("<{name}{attrs}>{first}");
            for (child, gap) in children {
                out.push_str(&child);
                out.push_str(&gap);
            }
            out.push_str(&last);
            out.push_str(&format!("</{name}>"));
            out
        })
    })
}

/// A document: an optional declaration, comments, a root element (never bare text), and trailing white space.
fn document() -> impl Strategy<Value = String> {
    (
        proptest::option::of(Just("<?xml version=\"1.0\"?>".to_owned())),
        space(),
        proptest::option::of(Just("<!-- before -->".to_owned())),
        space(),
        (name(), attrs(), proptest::collection::vec((element(), space()), 0..4)).prop_map(|(name, attrs, kids)| {
            let mut s = format!("<{name}{attrs}>");
            for (kid, gap) in kids {
                s.push_str(&kid);
                s.push_str(&gap);
            }
            s.push_str(&format!("</{name}>"));
            s
        }),
        space(),
        proptest::option::of(Just("<!-- after -->".to_owned())),
        space(),
    )
        .prop_map(|(decl, a, before, b, root, c, after, d)| {
            let mut s = decl.unwrap_or_default();
            if !s.is_empty() {
                s.push_str(&a);
            }
            if let Some(before) = before {
                s.push_str(&before);
                s.push_str(&b);
            }
            s.push_str(&root);
            if let Some(after) = after {
                s.push_str(&c);
                s.push_str(&after);
            }
            s.push_str(&d);
            s
        })
}

fn options() -> impl Strategy<Value = FormatOptions> {
    (
        prop_oneof![Just(Indent::Spaces(2)), Just(Indent::Spaces(4)), Just(Indent::Tab), Just(Indent::Spaces(0))],
        prop_oneof![Just(Eol::Lf), Just(Eol::CrLf)],
        prop_oneof![Just(BlankLines::Keep), Just(BlankLines::Remove)],
    )
        .prop_map(|(indent, eol, blank_lines)| FormatOptions { indent, eol, blank_lines })
}

fn without_space(text: &str) -> String {
    text.chars().filter(|c| !matches!(c, ' ' | '\t' | '\r' | '\n')).collect()
}

/// Elements whose content must come out byte for byte: those with text of their own, and those that preserve space.
fn untouchable(text: &str) -> Vec<String> {
    let tree = parse(text).syntax();
    tree.descendants()
        .filter(|n| n.kind() == SyntaxKind::Element)
        .filter(|element| {
            let mixed = element.children_with_tokens().any(|c| match c.kind() {
                SyntaxKind::Text => c.as_token().is_some_and(|t| !t.text().chars().all(|ch| matches!(ch, ' ' | '\t' | '\r' | '\n'))),
                SyntaxKind::CData | SyntaxKind::CharRef | SyntaxKind::EntityRef => true,
                _ => false,
            });
            let preserve = element.text().to_string().contains("xml:space=\"preserve\"") && {
                let start_tag = element.children().find(|n| n.kind() == SyntaxKind::StartTag).map(|t| t.text().to_string()).unwrap_or_default();
                start_tag.contains("xml:space=\"preserve\"")
            };
            mixed || preserve
        })
        .map(|element| element.text().to_string())
        .collect()
}

proptest! {
    #[test]
    fn pretty_printing_changes_only_white_space_between_nodes(src in document(), options in options()) {
        prop_assume!(!parse(&src).has_errors());
        let edits = pretty_print(&src, &options).expect("a well-formed document");
        // Ascending, disjoint, and only white space is replaced.
        prop_assert!(edits.windows(2).all(|w| w[0].range.end <= w[1].range.start));
        for e in &edits {
            prop_assert!(src.get(e.range.clone()).is_some_and(|old| old.chars().all(|c| matches!(c, ' ' | '\t' | '\r' | '\n'))), "{e:?}");
            prop_assert!(e.text.chars().all(|c| matches!(c, ' ' | '\t' | '\r' | '\n')));
        }
        let out = apply(&src, &edits);
        prop_assert_eq!(without_space(&out), without_space(&src));
        prop_assert!(!parse(&out).has_errors(), "still well-formed: {out:?}");
        // Idempotent.
        prop_assert_eq!(pretty_print(&out, &options).expect("well-formed"), vec![], "second pass on {:?}", out);
        // Mixed content and preserved space come through byte for byte.
        for kept in untouchable(&src) {
            prop_assert!(out.contains(&kept), "{kept:?} in {out:?}");
        }
    }

    #[test]
    fn minifying_changes_only_white_space_and_is_idempotent(src in document()) {
        prop_assume!(!parse(&src).has_errors());
        let edits = minify(&src).expect("a well-formed document");
        prop_assert!(edits.windows(2).all(|w| w[0].range.end <= w[1].range.start));
        let out = apply(&src, &edits);
        prop_assert_eq!(without_space(&out), without_space(&src));
        prop_assert!(!parse(&out).has_errors());
        prop_assert_eq!(minify(&out).expect("well-formed"), vec![]);
        for kept in untouchable(&src) {
            prop_assert!(out.contains(&kept), "{kept:?} in {out:?}");
        }
        // Formatting what was minified gives the same as formatting the original.
        // (Blank lines and the final newline are what minifying throws away, so they are left out of the comparison.)
        let options = FormatOptions { blank_lines: BlankLines::Remove, ..FormatOptions::default() };
        let from_minified = apply(&out, &pretty_print(&out, &options).expect("ok"));
        let from_original = apply(&src, &pretty_print(&src, &options).expect("ok"));
        prop_assert_eq!(from_minified.trim_end(), from_original.trim_end());
    }

    #[test]
    fn any_text_at_all_is_handled_without_panicking(src in "\\PC{0,120}", options in options()) {
        let _ = pretty_print(&src, &options);
        let _ = minify(&src);
    }
}
