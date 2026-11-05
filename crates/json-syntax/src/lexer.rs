//! Cutting a text into the tokens of JSON (and of its relatives: comments, single quotes and bare words are tokens too; whether they are
//! allowed is for the parser to say).

use std::ops::Range;

/// What a token is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Colon,
    Comma,
    /// A quoted string; `closed` is false when the line or the text ended first.
    Str {
        quote: char,
        closed: bool,
    },
    /// Anything else that is not white space or punctuation: a number, `true`, `NaN`, a bare name.
    Word,
    LineComment,
    BlockComment {
        closed: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Token {
    pub kind: Kind,
    pub span: Range<usize>,
}

/// Reads tokens from a text, skipping white space.
pub(crate) struct Lexer<'a> {
    text: &'a str,
    pos: usize,
}

fn is_punctuation(c: char) -> bool {
    matches!(c, '{' | '}' | '[' | ']' | ':' | ',' | '"' | '\'')
}

impl<'a> Lexer<'a> {
    pub(crate) fn new(text: &'a str) -> Self {
        Self { text, pos: 0 }
    }

    pub(crate) fn text(&self) -> &'a str {
        self.text
    }

    fn rest(&self) -> &'a str {
        self.text.get(self.pos..).unwrap_or_default()
    }

    pub(crate) fn next_token(&mut self) -> Option<Token> {
        // White space (and the byte-order mark some editors leave) between tokens.
        let skipped = self.rest().len() - self.rest().trim_start_matches(|c: char| c.is_whitespace() || c == '\u{FEFF}').len();
        self.pos += skipped;
        let start = self.pos;
        let c = self.rest().chars().next()?;
        let single = |kind: Kind, lexer: &mut Self| {
            lexer.pos += c.len_utf8();
            Token { kind, span: start..lexer.pos }
        };
        match c {
            '{' => Some(single(Kind::LBrace, self)),
            '}' => Some(single(Kind::RBrace, self)),
            '[' => Some(single(Kind::LBracket, self)),
            ']' => Some(single(Kind::RBracket, self)),
            ':' => Some(single(Kind::Colon, self)),
            ',' => Some(single(Kind::Comma, self)),
            '"' | '\'' => Some(self.string(c)),
            '/' if self.rest().starts_with("//") => {
                let end = self.rest().find(['\n', '\r']).unwrap_or(self.rest().len());
                self.pos += end;
                Some(Token { kind: Kind::LineComment, span: start..self.pos })
            }
            '/' if self.rest().starts_with("/*") => {
                let (closed, len) = match self.rest().get(2..).and_then(|r| r.find("*/")) {
                    Some(at) => (true, at + 4),
                    None => (false, self.rest().len()),
                };
                self.pos += len;
                Some(Token { kind: Kind::BlockComment { closed }, span: start..self.pos })
            }
            _ => {
                let mut len = 0;
                for (i, ch) in self.rest().char_indices() {
                    let comment = ch == '/' && i > 0 && (self.rest().get(i..).is_some_and(|r| r.starts_with("//") || r.starts_with("/*")));
                    if ch.is_whitespace() || is_punctuation(ch) || comment {
                        break;
                    }
                    len = i + ch.len_utf8();
                }
                // (A first character that is punctuation was handled above, so there is at least one character.)
                self.pos += len.max(c.len_utf8());
                Some(Token { kind: Kind::Word, span: start..self.pos })
            }
        }
    }

    /// A string from its opening quote to its closing one, or to the end of the line (a break inside a string is only allowed after a
    /// backslash).
    fn string(&mut self, quote: char) -> Token {
        let start = self.pos;
        let rest = self.rest();
        let mut chars = rest.char_indices().skip(1).peekable();
        let (mut closed, mut len) = (false, rest.len());
        while let Some((i, c)) = chars.next() {
            if c == '\\' {
                // The escaped character (a line break too) belongs to the string.
                if let Some((_, escaped)) = chars.next()
                    && escaped == '\r'
                    && chars.peek().is_some_and(|(_, n)| *n == '\n')
                {
                    chars.next();
                }
            } else if c == quote {
                (closed, len) = (true, i + c.len_utf8());
                break;
            } else if c == '\n' || c == '\r' {
                len = i;
                break;
            }
        }
        self.pos += len;
        Token { kind: Kind::Str { quote, closed }, span: start..self.pos }
    }
}
