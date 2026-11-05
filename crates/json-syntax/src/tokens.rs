//! The tokens of a text, for code that works on the white space between them (a formatter): where each is and what kind of thing.

use std::ops::Range;

use crate::lexer::{Kind, Lexer};
use crate::parser::Dialect;

/// What a token is, as far as layout goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// `{` or `[`.
    Open,
    /// `}` or `]`.
    Close,
    Colon,
    Comma,
    /// A string, a number, `true`, a bare name: anything that is a value or a name.
    Scalar,
    LineComment,
    BlockComment,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Range<usize>,
}

/// The tokens of `text` in order, comments included, and where the first one may start (after a byte-order mark). At most `max`
/// tokens are read; the answer says whether there were more.
pub fn tokens(text: &str, dialect: Dialect, max: usize) -> (Vec<Token>, usize, bool) {
    let mut lexer = Lexer::new(text, dialect == Dialect::Json5);
    let first = if text.starts_with('\u{FEFF}') { '\u{FEFF}'.len_utf8() } else { 0 };
    let mut out = Vec::new();
    while let Some(token) = lexer.next_token() {
        if out.len() >= max {
            return (out, first, true);
        }
        let kind = match token.kind {
            Kind::LBrace | Kind::LBracket => TokenKind::Open,
            Kind::RBrace | Kind::RBracket => TokenKind::Close,
            Kind::Colon => TokenKind::Colon,
            Kind::Comma => TokenKind::Comma,
            Kind::Str { .. } | Kind::Word => TokenKind::Scalar,
            Kind::LineComment => TokenKind::LineComment,
            Kind::BlockComment { .. } => TokenKind::BlockComment,
        };
        out.push(Token { kind, span: token.span });
    }
    (out, first, false)
}

/// For each of `offsets` (ascending), whether it is inside a string (a quoted scalar). The text is read only as far as the last offset.
pub fn inside_strings(text: &str, dialect: Dialect, offsets: &[usize]) -> Vec<bool> {
    let mut lexer = Lexer::new(text, dialect == Dialect::Json5);
    let mut out = Vec::with_capacity(offsets.len());
    let mut current = lexer.next_token();
    for &offset in offsets {
        while current.as_ref().is_some_and(|t| t.span.end <= offset) {
            current = lexer.next_token();
        }
        out.push(current.as_ref().is_some_and(|t| t.span.start <= offset && matches!(t.kind, Kind::Str { .. })));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_are_in_a_string_or_not() {
        let text = "{ /* \"no\" */ \"a\": \"b c\", d: 'e' }";
        let at = |needle: &str| text.find(needle).expect("there");
        let offsets = [at("no"), at("a\":"), at("b c") + 1, at(", d"), at("'e'") + 1, text.len() - 1, text.len() + 5];
        assert_eq!(inside_strings(text, Dialect::Json5, &offsets), [false, true, true, false, true, false, false]);
        assert_eq!(inside_strings("", Dialect::Json, &[0, 3]), [false, false]);
    }
}
