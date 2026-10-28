//! `anyURI`: the schema language accepts nearly any string, because a processor is to escape what a URI may not hold and then check.
//! What is refused here: an escape that is not `%` and two hexadecimal digits, a second `#`, a scheme that is not made of the characters
//! a scheme is made of, and a control character.

/// Whether `lexical` is a valid `anyURI` value.
pub fn is_any_uri(lexical: &str) -> bool {
    if lexical.chars().any(char::is_control) {
        return false;
    }
    // Percent-escapes.
    let bytes = lexical.as_bytes();
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let ok = bytes.get(i + 1..i + 3).is_some_and(|hex| hex.iter().all(u8::is_ascii_hexdigit));
            if !ok {
                return false;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    // One fragment at most.
    if lexical.matches('#').count() > 1 {
        return false;
    }
    // The text before the first `/`, `?` or `#` has a scheme if it has a colon; a scheme starts with a letter. (A relative reference
    // whose first segment has a colon would be read as a scheme, so it is not valid either.)
    let end = lexical.find(['/', '?', '#']).unwrap_or(lexical.len());
    match lexical.get(..end).and_then(|head| head.split_once(':')) {
        None => true,
        Some((scheme, _)) => {
            let mut chars = scheme.chars();
            chars.next().is_some_and(|c| c.is_ascii_alphabetic()) && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        }
    }
}
