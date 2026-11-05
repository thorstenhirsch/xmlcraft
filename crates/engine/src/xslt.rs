//! The XSLT stylesheet a document names: `<?xml-stylesheet type="text/xsl" href="style.xsl"?>` before its root element (what F10 uses when
//! no stylesheet is chosen).

use xmlcraft_syntax::{SyntaxKind, SyntaxNode, parse};

/// The value of a pseudo-attribute in the text of a processing instruction.
fn pseudo(text: &str, name: &str) -> Option<String> {
    let mut rest = text;
    loop {
        rest = rest.trim_start();
        let name_end = rest.find(|c: char| c == '=' || c.is_whitespace())?;
        let (found, after) = rest.split_at_checked(name_end)?;
        let value_part = after.trim_start().strip_prefix('=')?.trim_start();
        let quote = value_part.chars().next().filter(|c| matches!(c, '"' | '\''))?;
        let (value, remainder) = value_part.get(1..)?.split_once(quote)?;
        if found == name {
            return Some(decode(value));
        }
        rest = remainder;
    }
}

fn decode(value: &str) -> String {
    xmlcraft_syntax::decode_references(value)
}

/// The `href` of the first `xml-stylesheet` instruction before the root element whose `type` is an XSLT one (or has none), and
/// that is not an `alternate` or a CSS style sheet.
pub fn stylesheet_named(root: &SyntaxNode, text: &str) -> Option<String> {
    for node in root.children() {
        if node.kind() == SyntaxKind::Element {
            break;
        }
        if node.kind() != SyntaxKind::ProcessingInstruction {
            continue;
        }
        let range = usize::from(node.text_range().start())..usize::from(node.text_range().end());
        let Some(pi) = text.get(range) else { continue };
        let Some(body) = pi.strip_prefix("<?xml-stylesheet").filter(|rest| rest.starts_with(char::is_whitespace)) else { continue };
        let body = body.strip_suffix("?>").unwrap_or(body);
        if pseudo(body, "alternate").as_deref() == Some("yes") {
            continue;
        }
        let xslt_type = match pseudo(body, "type").as_deref() {
            None => true,
            Some(t) => matches!(t.to_ascii_lowercase().as_str(), "text/xsl" | "text/xml" | "application/xml" | "application/xslt+xml"),
        };
        if xslt_type && let Some(href) = pseudo(body, "href").filter(|h| !h.trim().is_empty()) {
            return Some(href);
        }
    }
    None
}

/// [`stylesheet_named`] for a text.
pub fn stylesheet_of(text: &str) -> Option<String> {
    let parsed = parse(text);
    stylesheet_named(&parsed.syntax(), text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_xslt_stylesheet_before_the_root_is_the_one_named() {
        let named = |pi: &str| stylesheet_of(&format!("<?xml version=\"1.0\"?>\n{pi}\n<r/>"));
        assert_eq!(named("<?xml-stylesheet type=\"text/xsl\" href=\"a.xsl\"?>").as_deref(), Some("a.xsl"));
        assert_eq!(named("<?xml-stylesheet href='b.xslt'?>").as_deref(), Some("b.xslt"), "no type: taken for XSLT");
        assert_eq!(named("<?xml-stylesheet type=\"text/css\" href=\"c.css\"?>"), None);
        assert_eq!(named("<?xml-stylesheet type=\"text/xsl\" href=\"d.xsl\" alternate=\"yes\"?>"), None);
        assert_eq!(
            named("<?xml-stylesheet type=\"text/css\" href=\"c.css\"?><?xml-stylesheet type=\"application/xslt+xml\" href=\"e&amp;f.xsl\"?>")
                .as_deref(),
            Some("e&f.xsl")
        );
        assert_eq!(named("<?xml-stylesheet type=\"text/xsl\"?>"), None, "no href");
        assert_eq!(stylesheet_of("<r/><?xml-stylesheet href=\"x.xsl\"?>"), None, "after the root it is not a prolog instruction");
        assert_eq!(stylesheet_of(""), None);
    }
}
