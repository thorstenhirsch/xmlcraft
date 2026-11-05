//! Colouring lines of YAML.

use proptest::prelude::*;
use xmlcraft_yaml_syntax::{JsonClass, Mode, highlight_line};

fn classes(line: &str) -> Vec<(String, JsonClass)> {
    highlight_line(line, Mode::Normal).0.into_iter().map(|(r, c)| (line[r].to_owned(), c)).collect()
}

#[test]
fn keys_values_dashes_and_comments_have_their_classes() {
    use JsonClass::*;
    assert_eq!(
        classes("name: hello # note"),
        [("name".to_owned(), Key), (":".to_owned(), Punctuation), ("hello".to_owned(), String), ("# note".to_owned(), Comment)]
    );
    assert_eq!(classes("- 12"), [("-".to_owned(), Punctuation), ("12".to_owned(), Number)]);
    assert_eq!(classes("  on: true")[2], ("true".to_owned(), Keyword));
    assert_eq!(classes("a: ~")[2], ("~".to_owned(), Keyword));
    assert_eq!(classes("\"q k\": 'v'"), [("\"q k\"".to_owned(), Key), (":".to_owned(), Punctuation), ("'v'".to_owned(), String)]);
    assert_eq!(
        classes("k: &a [1, *b]")[2..].iter().map(|(t, c)| (t.as_str(), *c)).collect::<Vec<_>>(),
        [("&a", Keyword), ("[", Punctuation), ("1", Number), (",", Punctuation), ("*b", Keyword), ("]", Punctuation)]
    );
    assert_eq!(classes("--- # doc")[0], ("---".to_owned(), Punctuation));
    // A # inside a word is not a comment; a URL has no key in it.
    assert_eq!(classes("a: b#c"), [("a".to_owned(), Key), (":".to_owned(), Punctuation), ("b#c".to_owned(), String)]);
    assert_eq!(classes("url: http://x:80/p")[2], ("http://x:80/p".to_owned(), String));
}

#[test]
fn block_scalars_and_multi_line_quotes_carry_over_the_end_of_a_line() {
    use JsonClass::*;
    let (spans, mode) = highlight_line("text: |\n", Mode::Normal);
    assert_eq!(mode, Mode::BlockScalar { parent: 0 });
    assert_eq!(spans.len(), 3);
    let (spans, mode) = highlight_line("  not: a key\n", mode);
    assert_eq!((spans.iter().map(|(_, c)| *c).collect::<Vec<_>>(), mode), (vec![String], Mode::BlockScalar { parent: 0 }));
    let (_, mode) = highlight_line("\n", mode);
    assert_eq!(mode, Mode::BlockScalar { parent: 0 }, "a blank line stays inside");
    let (spans, mode) = highlight_line("next: 1\n", mode);
    assert_eq!((mode, spans[0].1), (Mode::Normal, Key), "a line indented no more ends it");
    // The parent of a block scalar in a list entry is the entry's key.
    assert_eq!(highlight_line("- k: >-\n", Mode::Normal).1, Mode::BlockScalar { parent: 2 });
    // A quoted scalar that is not closed goes on.
    let (_, mode) = highlight_line("a: \"open\n", Mode::Normal);
    assert_eq!(mode, Mode::DoubleQuoted);
    let (spans, mode) = highlight_line("still \\\" in\" # after\n", mode);
    assert_eq!(mode, Mode::Normal);
    assert_eq!(spans.last().map(|(_, c)| *c), Some(Comment));
    let (_, mode) = highlight_line("b: 'it''s\n", Mode::Normal);
    assert_eq!(mode, Mode::SingleQuoted);
}

proptest! {
    #[test]
    fn any_line_is_coloured_with_ranges_that_are_ordered_and_on_characters(line in ".{0,100}", mode in 0u8..4, parent in 0usize..4) {
        let start = [Mode::Normal, Mode::BlockScalar { parent }, Mode::DoubleQuoted, Mode::SingleQuoted][usize::from(mode)];
        let (spans, _) = highlight_line(&line, start);
        let mut last = 0;
        for (range, _) in &spans {
            prop_assert!(range.start >= last && range.start < range.end && range.end <= line.len(), "{range:?} in {line:?}");
            prop_assert!(line.is_char_boundary(range.start) && line.is_char_boundary(range.end), "{range:?} in {line:?}");
            last = range.end;
        }
    }

    #[test]
    fn lines_made_of_yaml_pieces_are_coloured_in_order(pieces in prop::collection::vec(prop_oneof![Just("a"), Just(": "), Just("- "), Just("  "), Just("["), Just("]"), Just("{"), Just(","), Just("&x "), Just("*x"), Just("|"), Just("'"), Just("\""), Just("#"), Just("---"), Just("é"), Just("!t "), Just("? "), Just("\\")], 0..30)) {
        let line: String = pieces.concat();
        let (spans, _) = highlight_line(&line, Mode::Normal);
        let mut last = 0;
        for (range, _) in &spans {
            prop_assert!(range.start >= last && range.end <= line.len() && line.is_char_boundary(range.end), "{range:?} in {line:?}");
            last = range.end;
        }
    }
}
