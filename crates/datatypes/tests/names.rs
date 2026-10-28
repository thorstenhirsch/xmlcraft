//! The name predicates.

use xmlcraft_datatypes::{is_language, is_name, is_name_char, is_name_start_char, is_ncname, is_nmtoken, split_qname};

#[test]
fn name_characters() {
    for c in [':', 'A', 'z', '_', '\u{C0}', '\u{F8}', '\u{37F}', '\u{200C}', '\u{2070}', '\u{3001}', '\u{F900}', '\u{FDF0}', '\u{10000}', '\u{EFFFF}']
    {
        assert!(is_name_start_char(c) && is_name_char(c), "{c:?}");
    }
    for c in ['-', '.', '0', '9', '\u{B7}', '\u{300}', '\u{36F}', '\u{203F}', '\u{2040}'] {
        assert!(!is_name_start_char(c) && is_name_char(c), "{c:?}");
    }
    for c in [' ', '!', '<', '/', '\u{D7}', '\u{F7}', '\u{37E}', '\u{2000}', '\u{2190}', '\u{FFFE}', '\u{F0000}', '\u{0}'] {
        assert!(!is_name_start_char(c) && !is_name_char(c), "{c:?}");
    }
}

#[test]
fn names() {
    for yes in ["a", "a:b", "_x", ":x", "a-b.c", "é", "a\u{B7}", "日本"] {
        assert!(is_name(yes), "{yes:?}");
    }
    for no in ["", "1a", "-a", ".a", "a b", "a!", "\u{B7}a"] {
        assert!(!is_name(no), "{no:?}");
    }
    assert!(is_ncname("a") && is_ncname("a-b") && !is_ncname("a:b") && !is_ncname(":a") && !is_ncname("") && !is_ncname("1"));
    assert!(
        is_nmtoken("1a")
            && is_nmtoken("-")
            && is_nmtoken("a:b")
            && is_nmtoken("\u{B7}")
            && !is_nmtoken("")
            && !is_nmtoken("a b")
            && !is_nmtoken("a!")
    );
}

#[test]
fn languages() {
    for yes in ["en", "en-US", "x", "abcdefgh", "en-abcdefgh", "a-1", "zh-Hant-TW", "i-navajo"] {
        assert!(is_language(yes), "{yes:?}");
    }
    for no in ["", "en_US", "abcdefghi", "en-abcdefghi", "en-", "-en", "en--us", "e1", "1", "en us"] {
        assert!(!is_language(no), "{no:?}");
    }
}

#[test]
fn qualified_names_split_at_the_one_colon() {
    assert_eq!(split_qname("a"), Some((None, "a")));
    assert_eq!(split_qname("p:a"), Some((Some("p"), "a")));
    for bad in ["", ":a", "a:", "a:b:c", "1:a", "a:1", "a b", "p: a"] {
        assert_eq!(split_qname(bad), None, "{bad:?}");
    }
}
