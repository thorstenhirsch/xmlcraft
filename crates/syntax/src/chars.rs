//! Character classes of XML 1.0 (5th edition), section 2.

/// `S`: space, tab, carriage return, line feed.
pub fn is_xml_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

/// `NameStartChar` (also used for names with a prefix, because `:` is allowed).
pub fn is_name_start_char(c: char) -> bool {
    matches!(c,
        ':' | 'A'..='Z' | '_' | 'a'..='z'
        | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}' | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}'
        | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
}

/// `NameChar`.
pub fn is_name_char(c: char) -> bool {
    is_name_start_char(c) || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

/// `Char`: what may appear in a document at all.
pub fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | ' '..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for c in ['a', 'Z', '_', ':', 'é', '日', '\u{10000}'] {
            assert!(is_name_start_char(c), "{c:?}");
        }
        for c in ['-', '.', '0', '9', '\u{B7}', '\u{301}', '\u{203F}'] {
            assert!(is_name_char(c) && !is_name_start_char(c), "{c:?}");
        }
        for c in [' ', '<', '>', '&', '=', '"', '\'', '/', ';', '\u{D7}', '\u{F7}', '\u{FFFE}', '\u{200B}'] {
            assert!(!is_name_char(c), "{c:?}");
        }
    }

    #[test]
    fn document_characters() {
        for c in ['\t', '\n', '\r', ' ', 'a', '\u{D7FF}', '\u{E000}', '\u{FFFD}', '\u{10000}', '\u{10FFFF}'] {
            assert!(is_xml_char(c), "{c:?}");
        }
        for c in ['\0', '\u{1}', '\u{8}', '\u{B}', '\u{C}', '\u{1F}', '\u{FFFE}', '\u{FFFF}'] {
            assert!(!is_xml_char(c), "{c:?}");
        }
        assert!(is_xml_space(' ') && is_xml_space('\r') && !is_xml_space('\u{A0}'));
    }
}
