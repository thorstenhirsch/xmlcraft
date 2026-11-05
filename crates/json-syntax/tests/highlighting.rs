//! Colouring lines of JSON.

use proptest::prelude::*;
use xmlcraft_json_syntax::{JsonClass, Mode, highlight_line};

fn classes(line: &str) -> Vec<(String, JsonClass)> {
    highlight_line(line, Mode::Normal).0.into_iter().map(|(r, c)| (line[r].to_owned(), c)).collect()
}

#[test]
fn names_strings_numbers_and_keywords_have_their_own_classes() {
    use JsonClass::*;
    assert_eq!(
        classes("  \"a\": \"b\", \"n\" : -1.5e3, \"t\": true, \"z\": null"),
        [
            ("\"a\"".to_owned(), Key),
            (":".to_owned(), Punctuation),
            ("\"b\"".to_owned(), String),
            (",".to_owned(), Punctuation),
            ("\"n\"".to_owned(), Key),
            (":".to_owned(), Punctuation),
            ("-1.5e3".to_owned(), Number),
            (",".to_owned(), Punctuation),
            ("\"t\"".to_owned(), Key),
            (":".to_owned(), Punctuation),
            ("true".to_owned(), Keyword),
            (",".to_owned(), Punctuation),
            ("\"z\"".to_owned(), Key),
            (":".to_owned(), Punctuation),
            ("null".to_owned(), Keyword),
        ]
    );
    // JSON5: a bare name is a key; an escaped quote does not end a string.
    assert_eq!(classes("{a: 'x\\'y'}")[1], ("a".to_owned(), Key));
    assert_eq!(classes("\"a\\\"b\"")[0], ("\"a\\\"b\"".to_owned(), String));
}

#[test]
fn a_word_is_a_number_only_if_it_is_one_and_a_key_before_a_colon_is_a_key_whatever_it_is_spelt_like() {
    use JsonClass::*;
    for (word, class) in [("-", Error), ("+", Error), ("1abc", Error), ("1.2.3", Error), ("-1.5e3", Number), ("0x1F", Number), (".5", Number)] {
        assert_eq!(classes(word), [(word.to_owned(), class)], "{word}");
    }
    assert_eq!(classes("true: 1")[0], ("true".to_owned(), Key));
    assert_eq!(classes("null :")[0], ("null".to_owned(), Key));
    assert_eq!(classes("true")[0], ("true".to_owned(), Keyword));
    assert_eq!(classes("[1,2]").iter().filter(|(t, _)| t == ",").count(), 1, "punctuation is not joined");
}

#[test]
fn a_string_that_is_not_closed_and_a_stray_word_are_errors() {
    assert_eq!(classes("\"abc"), [("\"abc".to_owned(), JsonClass::Error)]);
    assert_eq!(classes("hello"), [("hello".to_owned(), JsonClass::Error)]);
}

#[test]
fn comments_are_carried_from_one_line_to_the_next() {
    use JsonClass::*;
    let (spans, mode) = highlight_line("1 /* open", Mode::Normal);
    assert_eq!(mode, Mode::BlockComment);
    assert_eq!(spans.iter().map(|(_, c)| *c).collect::<Vec<_>>(), [Number, Comment]);
    let (spans, mode) = highlight_line("still */ 2 // tail\n", mode);
    assert_eq!(mode, Mode::Normal);
    assert_eq!(
        spans.iter().map(|(r, c)| (&"still */ 2 // tail\n"[r.clone()], *c)).collect::<Vec<_>>(),
        [("still */", Comment), ("2", Number), ("// tail", Comment)]
    );
    let (_, mode) = highlight_line("/* a */ /* b", Mode::Normal);
    assert_eq!(mode, Mode::BlockComment, "the second comment is the one that is open");
}

proptest! {
    #[test]
    fn any_line_is_coloured_with_ranges_that_are_ordered_and_on_characters(line in ".{0,100}", block in any::<bool>()) {
        let start = if block { Mode::BlockComment } else { Mode::Normal };
        let (spans, _) = highlight_line(&line, start);
        let mut last = 0;
        for (range, _) in &spans {
            prop_assert!(range.start >= last && range.start < range.end && range.end <= line.len(), "{range:?} in {line:?}");
            prop_assert!(line.is_char_boundary(range.start) && line.is_char_boundary(range.end));
            last = range.end;
        }
    }
}
