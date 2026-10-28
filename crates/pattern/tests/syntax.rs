//! XML Schema regular expressions: what matches, what is refused and where.

use xmlcraft_pattern::{ErrorKind, MAX_COUNT, MAX_DEPTH, MAX_PATTERN_CHARS, Pattern, PatternError};

fn pattern(source: &str) -> Pattern {
    Pattern::new(source).unwrap_or_else(|e| panic!("{source:?} should be a pattern: {e}"))
}

fn matches(source: &str, value: &str) -> bool {
    pattern(source).is_match(value)
}

fn refused(source: &str) -> PatternError {
    Pattern::new(source).expect_err(&format!("{source:?} should be refused"))
}

#[test]
fn the_whole_value_has_to_match_and_there_are_no_anchors() {
    assert!(matches("abc", "abc"));
    for value in ["xabc", "abcx", "ab", "", "ABC"] {
        assert!(!matches("abc", value), "{value:?}");
    }
    // `^` and `$` are ordinary characters.
    assert!(matches("^abc$", "^abc$"));
    assert!(!matches("^abc$", "abc"));
    assert!(matches("a$", "a$"));
    // And a newline inside the value is nothing special either.
    assert!(!matches("abc", "abc\n"));
}

#[test]
fn the_empty_pattern_matches_only_the_empty_value() {
    assert!(matches("", ""));
    assert!(!matches("", "a"));
    assert!(matches("()", ""));
    assert!(matches("(a|)", ""));
    assert!(matches("(a|)", "a"));
    assert!(matches("a||b", ""));
}

#[test]
fn the_dot_is_anything_but_a_line_end() {
    assert!(matches(".", "x"));
    assert!(matches(".", "\u{1D11E}"), "a character outside the BMP is one character");
    assert!(matches(".", " "));
    assert!(matches(".", "\t"));
    for value in ["\n", "\r", "", "ab"] {
        assert!(!matches(".", value), "{value:?}");
    }
    assert!(matches(".*", "a b\tc"));
    assert!(!matches(".*", "a\nb"));
}

#[test]
fn quantifiers_count() {
    for (source, yes, no) in [
        ("a?", &["", "a"][..], &["aa", "b"][..]),
        ("a*", &["", "a", "aaaa"], &["b", "ab"]),
        ("a+", &["a", "aaa"], &["", "b"]),
        ("a{2}", &["aa"], &["a", "aaa", ""]),
        ("a{2,}", &["aa", "aaaaa"], &["a", ""]),
        ("a{2,3}", &["aa", "aaa"], &["a", "aaaa"]),
        ("a{0}", &[""], &["a"]),
        ("a{0,0}", &[""], &["a"]),
        ("(ab){2}", &["abab"], &["ab", "ababab", "aabb"]),
        ("(ab)*c", &["c", "abc", "ababc"], &["ac", "abab"]),
        ("a|bc", &["a", "bc"], &["b", "abc", "ac"]),
        ("(a|b)+", &["a", "abba"], &["", "c"]),
        ("[0-9]{3}-[0-9]{4}", &["555-1234"], &["5551234", "55-1234"]),
    ] {
        for value in yes {
            assert!(matches(source, value), "{source} should match {value:?}");
        }
        for value in no {
            assert!(!matches(source, value), "{source} should not match {value:?}");
        }
    }
    assert!(matches("a{0,1000}", &"a".repeat(1000)));
    assert!(!matches("a{0,1000}", &"a".repeat(1001)));
}

#[test]
fn character_classes() {
    for (source, yes, no) in [
        ("[abc]", &["a", "c"][..], &["d", "", "ab"][..]),
        ("[a-c]", &["a", "b", "c"], &["d", "A"]),
        ("[^a]", &["b", "\n", "é"], &["a", ""]),
        ("[^a-c]", &["d", "A"], &["b"]),
        ("[-a]", &["-", "a"], &["b"]),
        ("[a-]", &["-", "a"], &["b"]),
        ("[-]", &["-"], &["a"]),
        ("[^-]", &["a"], &["-"]),
        ("[a\\-z]", &["a", "-", "z"], &["b"]),
        ("[\\^a]", &["^", "a"], &["b"]),
        ("[a^]", &["^", "a"], &["b"]),
        ("[\\]]", &["]"], &["["]),
        ("[\\[]", &["["], &["]"]),
        ("[\\n\\r\\t ]+", &["\n", " \t\r\n"], &["a"]),
        ("[.*+?(){}|]", &[".", "*", "+", "?", "(", ")", "{", "}", "|"], &["a"]),
        ("[\\.\\\\]", &[".", "\\"], &["a"]),
        ("[\\p{Lu}\\d]", &["A", "5"], &["a"]),
        ("[a-z-[aeiou]]", &["b", "z"], &["a", "e", "A"]),
        ("[a-z-[aeiou]]+", &["xyz"], &["xyza"]),
        ("[\\p{L}-[\\p{Lu}]]", &["a", "é"], &["A", "5"]),
        ("[^a-z-[x]]", &["A", "-"], &["a", "y", "x"]),
        ("[a-z-[m-p-[n]]]", &["a", "n", "z"], &["m", "o", "p"]),
        ("[\\d-[5]]", &["4", "6"], &["5", "a"]),
        ("[a-[b]]", &["a"], &["b"]),
        ("[a-[a]]", &[], &["a", ""]),
        ("[\\u0041]", &[], &["A"]),
    ] {
        // `A` is not an escape of this language: it is refused, below.
        if source == "[\\u0041]" {
            assert!(Pattern::new(source).is_err());
            continue;
        }
        for value in yes {
            assert!(matches(source, value), "{source} should match {value:?}");
        }
        for value in no {
            assert!(!matches(source, value), "{source} should not match {value:?}");
        }
    }
}

#[test]
fn single_character_escapes() {
    for (source, value) in [
        ("\\n", "\n"),
        ("\\r", "\r"),
        ("\\t", "\t"),
        ("\\\\", "\\"),
        ("\\|", "|"),
        ("\\.", "."),
        ("\\?", "?"),
        ("\\*", "*"),
        ("\\+", "+"),
        ("\\(", "("),
        ("\\)", ")"),
        ("\\{", "{"),
        ("\\}", "}"),
        ("\\-", "-"),
        ("\\[", "["),
        ("\\]", "]"),
        ("\\^", "^"),
    ] {
        assert!(matches(source, value), "{source} should match {value:?}");
        assert!(!matches(source, "x"), "{source}");
    }
    assert!(matches("a\\{3\\}", "a{3}"));
}

#[test]
fn multi_character_escapes() {
    for (source, yes, no) in [
        ("\\s", &[" ", "\t", "\n", "\r"][..], &["a", "\u{A0}", "\u{2003}"][..]),
        ("\\S", &["a", "\u{A0}"], &[" ", "\n"]),
        ("\\d", &["5", "\u{0663}", "\u{FF15}"], &["a", "-"]),
        ("\\D", &["a", "-"], &["5"]),
        ("\\w", &["a", "é", "5", "日", "$", "+"], &["-", " ", ".", ",", "!", "_", "\u{0}", "\u{E000}"]),
        ("\\W", &["-", " ", "."], &["a", "5"]),
        ("\\i", &["a", "Z", "_", ":", "é", "日"], &["1", "-", ".", " ", "\u{B7}"]),
        ("\\I", &["1", "-", " "], &["a", "_"]),
        ("\\c", &["a", "1", "-", ".", "_", ":", "\u{B7}", "\u{300}", "é"], &[" ", "!", "<"]),
        ("\\C", &[" ", "!", "<"], &["a", "-"]),
        ("\\i\\c*", &["a", "abc-def.x", "_x", "a:b", "xml:lang"], &["1a", "-a", "", "a b"]),
    ] {
        for value in yes {
            assert!(matches(source, value), "{source} should match {value:?}");
        }
        for value in no {
            assert!(!matches(source, value), "{source} should not match {value:?}");
        }
    }
}

#[test]
fn categories_and_their_complements() {
    for (source, yes, no) in [
        ("\\p{L}+", &["abc", "héllo", "日本語"][..], &["a1", "", " "][..]),
        ("\\p{Lu}", &["A", "É"], &["a", "1"]),
        ("\\p{Ll}", &["a", "é"], &["A"]),
        ("\\p{N}", &["1", "\u{00BD}", "\u{2160}"], &["a"]),
        ("\\p{Nd}", &["1", "\u{0663}"], &["\u{00BD}"]),
        ("\\p{P}", &["!", ".", "-", "("], &["a", "+", "$"]),
        ("\\p{Pd}", &["-", "\u{2013}"], &["("]),
        ("\\p{S}", &["+", "$", "<", "^"], &["a", "!"]),
        ("\\p{Sc}", &["$", "\u{20AC}"], &["+"]),
        ("\\p{Z}", &[" ", "\u{A0}", "\u{2028}"], &["a", "\t"]),
        ("\\p{Zs}", &[" ", "\u{3000}"], &["\u{2028}"]),
        ("\\p{M}", &["\u{0301}"], &["a"]),
        ("\\p{C}", &["\u{0}", "\u{200B}", "\u{E000}"], &["a", " "]),
        ("\\p{Cc}", &["\u{0}", "\t", "\u{9F}"], &["a"]),
        ("\\p{Co}", &["\u{E000}"], &["a"]),
        ("\\P{L}", &["1", " "], &["a"]),
        ("\\P{Lu}+", &["abc123"], &["aBc"]),
    ] {
        for value in yes {
            assert!(matches(source, value), "{source} should match {value:?}");
        }
        for value in no {
            assert!(!matches(source, value), "{source} should not match {value:?}");
        }
    }
}

#[test]
fn blocks_are_named_with_is_and_without_spaces() {
    for (source, yes, no) in [
        ("\\p{IsBasicLatin}+", &["abc", "\u{0}", "~"][..], &["é"][..]),
        ("\\p{IsLatin-1Supplement}", &["é", "\u{80}", "\u{FF}"], &["a", "\u{100}"]),
        ("\\p{IsLatinExtended-A}", &["\u{100}", "\u{17F}"], &["a"]),
        ("\\p{IsCyrillic}+", &["привет"], &["hello"]),
        ("\\p{IsGreek}", &["α", "\u{3FF}"], &["a"]),
        ("\\p{IsGreekandCoptic}", &["α"], &["a"]),
        ("\\p{IsCJKUnifiedIdeographs}+", &["日本語"], &["かな"]),
        ("\\p{IsHiragana}", &["あ"], &["ア"]),
        ("\\p{IsPrivateUse}", &["\u{E000}", "\u{F8FF}", "\u{F0000}", "\u{10FFFD}"], &["a", "\u{F900}"]),
        ("\\p{IsSpecials}", &["\u{FFFD}", "\u{FEFF}", "\u{FFF0}"], &["a", "\u{FFFE}".get(..0).unwrap_or("a")]),
        ("\\p{IsMusicalSymbols}", &["\u{1D11E}"], &["a"]),
        ("\\p{IsHighSurrogates}", &[], &["a", "\u{E000}"]),
        ("\\P{IsBasicLatin}", &["é"], &["a"]),
        ("[\\p{IsBasicLatin}-[\\p{L}]]", &["1", " "], &["a", "é"]),
    ] {
        for value in yes {
            assert!(matches(source, value), "{source} should match {value:?}");
        }
        for value in no {
            assert!(!matches(source, value), "{source} should not match {value:?}");
        }
    }
}

#[test]
fn what_a_pattern_may_not_say_is_refused_where_it_is_said() {
    use ErrorKind::*;
    for (source, position, kind) in [
        ("a(", 1, UnclosedGroup),
        ("(a|b", 0, UnclosedGroup),
        ("((a)", 0, UnclosedGroup),
        ("a)", 1, UnbalancedClose),
        (")", 0, UnbalancedClose),
        ("(a))", 3, UnbalancedClose),
        ("[a", 0, UnclosedClass),
        ("[a-", 0, UnclosedClass),
        ("[", 0, UnclosedClass),
        ("[]", 0, EmptyClass),
        ("[^]", 0, EmptyClass),
        ("*a", 0, NothingToRepeat('*')),
        ("+", 0, NothingToRepeat('+')),
        ("?", 0, NothingToRepeat('?')),
        ("(*)", 1, NothingToRepeat('*')),
        ("a|*", 2, NothingToRepeat('*')),
        ("a**", 2, DoubleQuantifier),
        ("a+?", 2, DoubleQuantifier),
        ("a*?", 2, DoubleQuantifier),
        ("a{2}{3}", 4, DoubleQuantifier),
        ("a{2}?", 4, DoubleQuantifier),
        ("a{", 1, BadQuantifier),
        ("a{1", 1, BadQuantifier),
        ("a{1,", 1, BadQuantifier),
        ("a{,2}", 1, BadQuantifier),
        ("a{x}", 1, BadQuantifier),
        ("a{1,x}", 1, BadQuantifier),
        ("a{1;2}", 1, BadQuantifier),
        ("a{2,1}", 1, QuantifierRange),
        ("a{1000001}", 1, QuantifierTooLarge),
        ("\\q", 0, UnknownEscape('q')),
        ("\\a", 0, UnknownEscape('a')),
        ("\\u0041", 0, UnknownEscape('u')),
        ("\\1", 0, UnknownEscape('1')),
        ("\\b", 0, UnknownEscape('b')),
        ("[\\q]", 1, UnknownEscape('q')),
        ("a\\", 1, TrailingBackslash),
        ("\\p", 0, BadProperty),
        ("\\pL", 0, BadProperty),
        ("\\p{", 0, BadProperty),
        ("\\p{}", 0, BadProperty),
        ("\\p{L", 0, BadProperty),
        ("\\p{Xx}", 0, UnknownCategory("Xx".into())),
        ("\\p{Greek}", 0, UnknownCategory("Greek".into())),
        ("\\p{l}", 0, UnknownCategory("l".into())),
        ("\\P{Zz}", 0, UnknownCategory("Zz".into())),
        ("\\p{IsNope}", 0, UnknownBlock("IsNope".into())),
        ("\\p{Is}", 0, UnknownBlock("Is".into())),
        ("\\p{isBasicLatin}", 0, UnknownCategory("isBasicLatin".into())),
        ("a]", 1, NeedsEscape(']')),
        ("a}", 1, NeedsEscape('}')),
        ("{", 0, NeedsEscape('{')),
        ("{2}", 0, NeedsEscape('{')),
        ("[a[b]", 2, NeedsEscape('[')),
        ("[z-a]", 2, BadRange),
        ("[a-\\d]", 2, BadRange),
        ("[\\d-z]", 3, BadRange),
        ("[a-c-e]", 4, BadRange),
        ("[a--]", 2, BadRange),
        ("[--a]", 2, BadRange),
        ("[a-z-[aeiou]x]", 12, SubtractionNotLast),
        ("[a-z-[aeiou]-[x]]", 12, SubtractionNotLast),
    ] {
        let e = refused(source);
        assert_eq!((e.position, e.kind.clone()), (position, kind), "{source}: {e}");
    }
}

#[test]
fn every_error_has_a_message_and_a_position_one_can_show() {
    let e = refused("ab(c");
    assert_eq!(e.to_string(), "at character 3: a ( has no )");
    assert!(!refused("a{2,1}").message().is_empty());
}

#[test]
fn a_pattern_has_limits() {
    assert_eq!(refused(&"a".repeat(MAX_PATTERN_CHARS + 1)).kind, ErrorKind::TooLong);
    assert!(Pattern::new(&"a".repeat(MAX_PATTERN_CHARS)).is_ok());
    let deep = "(".repeat(MAX_DEPTH + 1) + &")".repeat(MAX_DEPTH + 1);
    assert_eq!(refused(&deep).kind, ErrorKind::TooDeep);
    let ok = "(".repeat(MAX_DEPTH) + "a" + &")".repeat(MAX_DEPTH);
    assert!(matches(&ok, "a"));
    // Character classes nest only through subtraction.
    let classes = "[a-z".to_owned() + &"-[a-z".repeat(MAX_DEPTH + 1) + &"]".repeat(MAX_DEPTH + 2);
    assert_eq!(refused(&classes).kind, ErrorKind::TooDeep);
    assert_eq!(MAX_COUNT, 1_000_000);
    // A pattern that is fine as text and too big as an automaton.
    assert_eq!(refused("(((a{1000}){1000}){1000}){1000}").kind, ErrorKind::TooComplex);
    assert_eq!(refused("((\\w{1000}){1000}){1000}").kind, ErrorKind::TooComplex);
}

#[test]
fn a_pattern_is_kept_as_it_was_written_and_describes_what_matches() {
    use regex_automata::meta::Regex;
    use regex_syntax::hir::{Hir, Look};
    let p = pattern("[a-c]+\\.x");
    assert_eq!(p.as_str(), "[a-c]+\\.x");
    // The tree is the values that match, without the anchors: anchored, it is the pattern.
    let anchored = Hir::concat(vec![Hir::look(Look::Start), p.hir().clone(), Hir::look(Look::End)]);
    let again = Regex::builder().build_from_hir(&anchored).expect("a regex");
    for value in ["abc.x", "a.x", "x", "abcd.x", ".x", "", "a.xx"] {
        assert_eq!(again.is_match(value), p.is_match(value), "{value:?}");
    }
}

#[test]
fn a_repetition_of_a_repetition_is_one_and_not_a_lazy_quantifier() {
    for (source, yes, no) in [
        ("(a+)?", &["", "a", "aaa"][..], &["b"][..]),
        ("((a){1,}){0,1}", &["", "aa"], &["b"]),
        ("(a*)*", &["", "aa"], &["b"]),
        ("((a){0,1}){0,}", &["", "aaa"], &["ab"]),
        ("(a{2}){3}", &["aaaaaa"], &["aaaa", "aaaaaaaa"]),
        ("(a?){2}b", &["b", "ab", "aab"], &["aaab"]),
    ] {
        for value in yes {
            assert!(matches(source, value), "{source} should match {value:?}");
        }
        for value in no {
            assert!(!matches(source, value), "{source} should not match {value:?}");
        }
    }
}

#[test]
fn a_block_has_exactly_its_code_points() {
    for (name, lo, hi) in [
        ("BasicLatin", 0x0000, 0x007F),
        ("Latin-1Supplement", 0x0080, 0x00FF),
        ("LatinExtended-A", 0x0100, 0x017F),
        ("Greek", 0x0370, 0x03FF),
        ("Cyrillic", 0x0400, 0x04FF),
        ("Hebrew", 0x0590, 0x05FF),
        ("Arabic", 0x0600, 0x06FF),
        ("Thai", 0x0E00, 0x0E7F),
        ("GeneralPunctuation", 0x2000, 0x206F),
        ("Arrows", 0x2190, 0x21FF),
        ("Hiragana", 0x3040, 0x309F),
        ("Katakana", 0x30A0, 0x30FF),
        ("CJKUnifiedIdeographs", 0x4E00, 0x9FFF),
        ("HangulSyllables", 0xAC00, 0xD7A3),
        ("HalfwidthandFullwidthForms", 0xFF00, 0xFFEF),
        ("MathematicalAlphanumericSymbols", 0x1D400, 0x1D7FF),
        ("Tags", 0xE0000, 0xE007F),
    ] {
        let p = pattern(&format!("\\p{{Is{name}}}"));
        let at = |code: u32| char::from_u32(code).map(String::from);
        assert!(at(lo).is_some_and(|c| p.is_match(&c)), "{name}: first");
        assert!(at(hi).is_some_and(|c| p.is_match(&c)), "{name}: last");
        assert!(!lo.checked_sub(1).and_then(at).is_some_and(|c| p.is_match(&c)), "{name}: before");
        assert!(!at(hi + 1).is_some_and(|c| p.is_match(&c)), "{name}: after");
    }
}

#[test]
fn the_largest_count_is_a_count_and_one_more_is_too_large() {
    assert_eq!(refused("a{1000001}").kind, ErrorKind::QuantifierTooLarge);
    assert_eq!(refused("a{0,1000001}").kind, ErrorKind::QuantifierTooLarge);
    // Of the largest count that is allowed, nothing more is asked here than that it is not called too large.
    assert!(Pattern::new("(){1000000}").is_ok());
    assert!(matches("(){1000000}", ""));
}
