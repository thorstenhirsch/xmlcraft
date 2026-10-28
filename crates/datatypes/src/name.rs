//! Names: the lexical spaces of `Name`, `NCName`, `NMTOKEN`, `language` and `QName`, and the XML name characters.
//!
//! (The name characters are those of XML 1.0, fifth edition. The syntax crate has the same predicates; the two are siblings in the
//! layering and neither may use the other.)

/// May a name start with `c`? (`NameStartChar`, colon included.)
pub fn is_name_start_char(c: char) -> bool {
    matches!(c,
        ':' | 'A'..='Z' | '_' | 'a'..='z'
        | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}' | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}'
        | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
}

/// May a name have `c`? (`NameChar`.)
pub fn is_name_char(c: char) -> bool {
    is_name_start_char(c) || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

/// `Name`: a name start character, then name characters.
pub fn is_name(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(is_name_start_char) && chars.all(is_name_char)
}

/// `NCName`: a `Name` with no colon.
pub fn is_ncname(text: &str) -> bool {
    is_name(text) && !text.contains(':')
}

/// `Nmtoken`: one or more name characters.
pub fn is_nmtoken(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_name_char)
}

/// `language`: RFC 3066 form, `[a-zA-Z]{1,8}(-[a-zA-Z0-9]{1,8})*`.
pub fn is_language(text: &str) -> bool {
    let mut parts = text.split('-');
    let first_ok = parts.next().is_some_and(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphabetic()));
    first_ok && parts.all(|p| (1..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// `QName`: `NCName` or `NCName:NCName`; the prefix (if any) and the local part.
pub fn split_qname(text: &str) -> Option<(Option<&str>, &str)> {
    match text.split_once(':') {
        None => is_ncname(text).then_some((None, text)),
        Some((prefix, local)) => (is_ncname(prefix) && is_ncname(local)).then_some((Some(prefix), local)),
    }
}
