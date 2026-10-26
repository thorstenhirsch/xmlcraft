//! Interface strings. The English text is the key, as in PdfCraft (`tl!("Open")`). Only English exists for now: `tl!` returns its
//! argument, and the translation catalogs arrive with the localisation milestone. Every user-visible string goes through it so
//! that later work has one place to hook into.

/// The text to show for `english`.
pub fn t(english: &str) -> &str {
    english
}

/// Fill `{name}` placeholders in `template` from `values`. An unknown placeholder is left as written, and a value is never
/// scanned for placeholders itself.
pub fn fmt(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let name = &after[..close];
                match values.iter().find(|(n, _)| *n == name) {
                    Some((_, value)) => out.push_str(value),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// `tl!("Open")`: the interface text for an English string.
macro_rules! tl {
    ($s:expr) => {
        $crate::i18n::t($s)
    };
}
pub(crate) use tl;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_filled_and_unknown_ones_kept() {
        assert_eq!(fmt("{name} — unsaved changes", &[("name", "a.xml")]), "a.xml — unsaved changes");
        assert_eq!(fmt("Line {line}, column {col}", &[("line", "3"), ("col", "9")]), "Line 3, column 9");
        assert_eq!(fmt("{missing} stays", &[]), "{missing} stays");
    }

    #[test]
    fn malformed_templates_never_panic() {
        assert_eq!(fmt("open { brace", &[("x", "1")]), "open { brace");
        assert_eq!(fmt("{", &[]), "{");
        assert_eq!(fmt("}{", &[]), "}{");
        assert_eq!(fmt("é{é}é", &[("é", "ü")]), "éüé");
        assert_eq!(fmt("", &[]), "");
    }

    #[test]
    fn a_value_is_not_scanned_for_placeholders() {
        assert_eq!(fmt("{a}", &[("a", "{b}"), ("b", "no")]), "{b}");
    }
}
