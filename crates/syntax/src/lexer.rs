//! The XML lexer: a total, lossless, restartable tokenizer.
//!
//! The text is cut into tokens that tile it exactly; every token is at least one byte. **No token contains a line feed except as its
//! last byte**, so every line start is a token boundary and the lexer's [`Mode`] there is a complete checkpoint: an editor colouring
//! lines 400 to 440 resumes from the checkpoint of line 400 instead of lexing from the top.
//!
//! The lexer is a small state machine: [`Mode`] says where in the markup it is (content, inside a tag, inside an attribute value, a
//! comment, a DOCTYPE and so on). Broken markup recovers the way people expect while typing: a `<` inside a tag or an attribute value
//! ends that tag (the missing `>` or quote is the parser's to report) and starts the next one.

use crate::chars::{is_name_char, is_name_start_char};

/// Which quote opened a quoted string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Quote {
    Double,
    Single,
}

impl Quote {
    fn byte(self) -> u8 {
        match self {
            Quote::Double => b'"',
            Quote::Single => b'\'',
        }
    }

    fn from_byte(b: u8) -> Option<Self> {
        match b {
            b'"' => Some(Quote::Double),
            b'\'' => Some(Quote::Single),
            _ => None,
        }
    }
}

/// Where a comment, a processing instruction or a quoted string returns to when it ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Return {
    Content,
    /// The internal subset of a DOCTYPE: inside `[` ... `]`.
    Subset,
    /// The DOCTYPE outside its internal subset.
    Doctype,
    /// A markup declaration in the subset: `<!ELEMENT` ... `>`.
    Decl,
    /// The XML declaration.
    XmlDecl,
}

impl Return {
    fn mode(self) -> Mode {
        match self {
            Return::Content => Mode::Content,
            Return::Subset => Mode::Subset,
            Return::Doctype => Mode::Doctype,
            Return::Decl => Mode::Decl,
            Return::XmlDecl => Mode::XmlDecl,
        }
    }
}

/// Where the lexer is in the markup. This is all of its state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    /// Character data between tags.
    #[default]
    Content,
    /// Inside a start or end tag.
    Tag,
    /// Inside a quoted attribute value.
    AttrValue(Quote),
    Comment(Return),
    Cdata,
    /// Right after `<?`, where the target goes.
    PiTarget(Return),
    /// A processing instruction's data.
    Pi(Return),
    XmlDecl,
    Doctype,
    Subset,
    Decl,
    /// Inside a quoted literal of the DOCTYPE, a declaration or the XML declaration.
    Literal(Quote, Return),
}

/// What a token is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// Character data (up to the next markup, reference or line end).
    Text,
    /// `&#65;` or `&#x41;`.
    CharRef,
    /// `&name;`.
    EntityRef,
    /// A `&` that does not start a complete reference.
    BareAmp,
    /// A `<` that does not start any markup.
    BareLt,
    /// `<` followed by a name: a start tag begins.
    TagOpen,
    /// `</`.
    EndTagOpen,
    /// `>` closing a tag, the DOCTYPE or a declaration.
    Gt,
    /// `/>`.
    SlashGt,
    /// A name: element, attribute, DOCTYPE keyword or declaration name.
    Name,
    Eq,
    /// Spaces, tabs and line breaks inside markup.
    Whitespace,
    /// A `"` or `'` opening or closing a quoted string.
    Quote,
    /// Text of an attribute value.
    AttrText,
    /// Text of a DOCTYPE, declaration or XML-declaration literal.
    LiteralText,
    CommentStart,
    CommentText,
    CommentEnd,
    CdataStart,
    CdataText,
    CdataEnd,
    /// `<?`.
    PiStart,
    /// The name after `<?`.
    PiTarget,
    PiText,
    /// `?>`.
    PiEnd,
    /// `<!DOCTYPE`.
    DoctypeStart,
    /// `<!ELEMENT`, `<!ATTLIST`, `<!ENTITY`, `<!NOTATION` (any `<!name` in the subset).
    DeclStart,
    LBracket,
    RBracket,
    /// `%name;` in the DTD.
    PercentRef,
    /// One of `( ) | , ? * + # %` in a declaration.
    Punct,
    /// A character that fits nowhere (including `<!` that starts nothing).
    Error,
}

/// A token: its kind and where it is in the text, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub start: usize,
    pub end: usize,
}

impl Token {
    pub fn range(&self) -> std::ops::Range<usize> {
        self.start..self.end
    }

    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    /// Never true for a token the lexer produces.
    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

/// A place the lexer can resume from: a token boundary and the mode there. A checkpoint is only good for the text it was taken in;
/// drop the ones at or after an edit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Checkpoint {
    pub offset: usize,
    pub mode: Mode,
}

/// What one step of the state machine did.
enum Step {
    /// A token ending at `end` (exclusive), after which the lexer is in `next`.
    Token(TokenKind, usize, Mode),
    /// Change mode without consuming anything (error recovery).
    Switch(Mode),
}

/// The lexer. It is an iterator of [`Token`]s.
#[derive(Clone, Debug)]
pub struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    mode: Mode,
}

impl<'a> Lexer<'a> {
    /// Lex `src` from the start, as document content.
    pub fn new(src: &'a str) -> Self {
        Self { src, pos: 0, mode: Mode::Content }
    }

    /// Lex `src` from the start in another mode (a DTD file starts in [`Mode::Subset`]).
    pub fn with_mode(src: &'a str, mode: Mode) -> Self {
        Self { src, pos: 0, mode }
    }

    /// Continue from a checkpoint taken in the same text. An offset past the end, or inside a character, is moved to the next
    /// boundary, so a stale checkpoint gives wrong colours at worst, never a panic.
    pub fn resume(src: &'a str, at: Checkpoint) -> Self {
        let mut pos = at.offset.min(src.len());
        while !src.is_char_boundary(pos) {
            pos += 1; // ends at src.len(), which is a boundary
        }
        Self { src, pos, mode: at.mode }
    }

    /// Where the next token starts.
    pub fn offset(&self) -> usize {
        self.pos
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn checkpoint(&self) -> Checkpoint {
        Checkpoint { offset: self.pos, mode: self.mode }
    }

    // --- looking at the text -------------------------------------------------------------------------------------------------

    /// The text from here on, as bytes.
    fn bytes(&self) -> &'a [u8] {
        self.src.as_bytes().get(self.pos..).unwrap_or_default()
    }

    fn byte(&self) -> u8 {
        self.bytes().first().copied().unwrap_or(0)
    }

    fn byte_at(&self, ahead: usize) -> Option<u8> {
        self.bytes().get(ahead).copied()
    }

    fn looking_at(&self, s: &str) -> bool {
        self.bytes().starts_with(s.as_bytes())
    }

    /// The character starting at byte `at` (which must be a boundary).
    fn char_at(&self, at: usize) -> Option<char> {
        self.src.get(at..)?.chars().next()
    }

    fn char_len(&self) -> usize {
        self.char_at(self.pos).map_or(1, char::len_utf8)
    }

    /// The end of the run of characters from `from` that satisfy `keep`.
    fn chars_end(&self, from: usize, keep: fn(char) -> bool) -> usize {
        let mut end = from;
        for c in self.src.get(from..).unwrap_or_default().chars() {
            if !keep(c) {
                break;
            }
            end += c.len_utf8();
        }
        end
    }

    fn name_end(&self, from: usize) -> usize {
        self.chars_end(from, is_name_char)
    }

    /// The end of a run that stops before any byte in `stops` and after a line feed.
    fn run_end(&self, stops: &[u8]) -> usize {
        let hay = self.bytes();
        match hay.iter().position(|b| *b == b'\n' || stops.contains(b)) {
            Some(i) if hay.get(i) == Some(&b'\n') => self.pos + i + 1,
            Some(i) => self.pos + i,
            None => self.src.len(),
        }
    }

    /// The end of text that runs up to `terminator` (not including it) or past the next line feed. The terminator is not at the
    /// current position (the caller checked).
    fn run_to(&self, terminator: &str) -> usize {
        let hay = self.bytes();
        let term = terminator.as_bytes();
        let mut i = 0;
        while let Some(&b) = hay.get(i) {
            if b == b'\n' {
                return self.pos + i + 1;
            }
            if i > 0 && hay.get(i..).is_some_and(|rest| rest.starts_with(term)) {
                return self.pos + i;
            }
            i += 1;
        }
        self.src.len()
    }

    /// The end of a run of spaces, tabs and carriage returns, taking in a line feed that ends it.
    fn whitespace_end(&self) -> usize {
        let hay = self.bytes();
        let mut i = 0;
        while let Some(&b) = hay.get(i) {
            match b {
                b' ' | b'\t' | b'\r' => i += 1,
                b'\n' => return self.pos + i + 1,
                _ => break,
            }
        }
        self.pos + i
    }

    fn is_whitespace_here(&self) -> bool {
        matches!(self.byte(), b' ' | b'\t' | b'\r' | b'\n')
    }

    // --- the modes -----------------------------------------------------------------------------------------------------------

    fn step(&self) -> Step {
        let p = self.pos;
        match self.mode {
            Mode::Content => self.content(),
            Mode::Tag => self.tag(),
            Mode::AttrValue(q) => self.attr_value(q),
            Mode::Comment(ret) => {
                if self.looking_at("-->") {
                    Step::Token(TokenKind::CommentEnd, p + 3, ret.mode())
                } else {
                    Step::Token(TokenKind::CommentText, self.run_to("-->"), self.mode)
                }
            }
            Mode::Cdata => {
                if self.looking_at("]]>") {
                    Step::Token(TokenKind::CdataEnd, p + 3, Mode::Content)
                } else {
                    Step::Token(TokenKind::CdataText, self.run_to("]]>"), self.mode)
                }
            }
            Mode::PiTarget(ret) => {
                if self.char_at(p).is_some_and(is_name_start_char) {
                    let end = self.name_end(p);
                    // The XML declaration has pseudo-attributes, which are lexed like a tag's.
                    let is_xml_decl = ret == Return::Content && self.src.get(p..end) == Some("xml");
                    Step::Token(TokenKind::PiTarget, end, if is_xml_decl { Mode::XmlDecl } else { Mode::Pi(ret) })
                } else {
                    Step::Switch(Mode::Pi(ret))
                }
            }
            Mode::Pi(ret) => {
                if self.looking_at("?>") {
                    Step::Token(TokenKind::PiEnd, p + 2, ret.mode())
                } else {
                    Step::Token(TokenKind::PiText, self.run_to("?>"), self.mode)
                }
            }
            Mode::XmlDecl => self.xml_decl(),
            Mode::Doctype => self.doctype(),
            Mode::Subset => self.subset(),
            Mode::Decl => self.decl(),
            Mode::Literal(q, ret) => {
                if self.byte() == q.byte() {
                    Step::Token(TokenKind::Quote, p + 1, ret.mode())
                } else if self.byte() == b'<' && ret == Return::XmlDecl {
                    Step::Switch(Mode::Content) // an unterminated declaration
                } else {
                    let stops: &[u8] = if ret == Return::XmlDecl { &[q.byte(), b'<'] } else { &[q.byte()] };
                    Step::Token(TokenKind::LiteralText, self.run_end(stops), self.mode)
                }
            }
        }
    }

    /// `&` at the current position, in the mode `back` (which it leaves unchanged).
    fn reference(&self, back: Mode) -> Step {
        let p = self.pos;
        let after = p + 1;
        let found = match self.byte_at(1) {
            Some(b'#') => {
                let hex = self.byte_at(2) == Some(b'x');
                let digits_from = if hex { p + 3 } else { p + 2 };
                let digits = self.src.as_bytes().get(digits_from..).unwrap_or_default();
                let n = digits.iter().take_while(|b| if hex { b.is_ascii_hexdigit() } else { b.is_ascii_digit() }).count();
                (n > 0 && digits.get(n) == Some(&b';')).then_some((TokenKind::CharRef, digits_from + n + 1))
            }
            Some(_) if self.char_at(after).is_some_and(is_name_start_char) => {
                let end = self.name_end(after);
                (self.src.as_bytes().get(end) == Some(&b';')).then_some((TokenKind::EntityRef, end + 1))
            }
            _ => None,
        };
        match found {
            Some((kind, end)) => Step::Token(kind, end, back),
            None => Step::Token(TokenKind::BareAmp, after, back),
        }
    }

    fn content(&self) -> Step {
        let p = self.pos;
        match self.byte() {
            b'<' => match self.byte_at(1) {
                Some(b'/') => Step::Token(TokenKind::EndTagOpen, p + 2, Mode::Tag),
                Some(b'?') => Step::Token(TokenKind::PiStart, p + 2, Mode::PiTarget(Return::Content)),
                Some(b'!') => {
                    if self.looking_at("<!--") {
                        Step::Token(TokenKind::CommentStart, p + 4, Mode::Comment(Return::Content))
                    } else if self.looking_at("<![CDATA[") {
                        Step::Token(TokenKind::CdataStart, p + 9, Mode::Cdata)
                    } else if self.looking_at("<!DOCTYPE") {
                        Step::Token(TokenKind::DoctypeStart, p + 9, Mode::Doctype)
                    } else {
                        Step::Token(TokenKind::Error, p + 2, Mode::Content)
                    }
                }
                _ if self.char_at(p + 1).is_some_and(is_name_start_char) => Step::Token(TokenKind::TagOpen, p + 1, Mode::Tag),
                _ => Step::Token(TokenKind::BareLt, p + 1, Mode::Content),
            },
            b'&' => self.reference(Mode::Content),
            _ => Step::Token(TokenKind::Text, self.run_end(b"<&"), Mode::Content),
        }
    }

    fn tag(&self) -> Step {
        let p = self.pos;
        let b = self.byte();
        match b {
            b'>' => Step::Token(TokenKind::Gt, p + 1, Mode::Content),
            b'/' if self.byte_at(1) == Some(b'>') => Step::Token(TokenKind::SlashGt, p + 2, Mode::Content),
            b'<' => Step::Switch(Mode::Content), // a tag left open
            b'=' => Step::Token(TokenKind::Eq, p + 1, Mode::Tag),
            _ => match Quote::from_byte(b) {
                Some(q) => Step::Token(TokenKind::Quote, p + 1, Mode::AttrValue(q)),
                None => self.name_or_space_or_error(Mode::Tag),
            },
        }
    }

    /// Whitespace, a name, or one character of error, staying in `mode`.
    fn name_or_space_or_error(&self, mode: Mode) -> Step {
        let p = self.pos;
        if self.is_whitespace_here() {
            Step::Token(TokenKind::Whitespace, self.whitespace_end(), mode)
        } else if self.char_at(p).is_some_and(is_name_char) {
            Step::Token(TokenKind::Name, self.name_end(p), mode)
        } else {
            Step::Token(TokenKind::Error, p + self.char_len(), mode)
        }
    }

    fn attr_value(&self, q: Quote) -> Step {
        let p = self.pos;
        match self.byte() {
            b if b == q.byte() => Step::Token(TokenKind::Quote, p + 1, Mode::Tag),
            b'<' => Step::Switch(Mode::Content), // a value left open
            b'&' => self.reference(self.mode),
            _ => Step::Token(TokenKind::AttrText, self.run_end(&[q.byte(), b'<', b'&']), self.mode),
        }
    }

    fn xml_decl(&self) -> Step {
        let p = self.pos;
        let b = self.byte();
        match b {
            b'?' if self.byte_at(1) == Some(b'>') => Step::Token(TokenKind::PiEnd, p + 2, Mode::Content),
            b'<' => Step::Switch(Mode::Content), // a declaration left open
            b'=' => Step::Token(TokenKind::Eq, p + 1, Mode::XmlDecl),
            _ => match Quote::from_byte(b) {
                Some(q) => Step::Token(TokenKind::Quote, p + 1, Mode::Literal(q, Return::XmlDecl)),
                None => self.name_or_space_or_error(Mode::XmlDecl),
            },
        }
    }

    fn doctype(&self) -> Step {
        let p = self.pos;
        let b = self.byte();
        match b {
            b'>' => Step::Token(TokenKind::Gt, p + 1, Mode::Content),
            b'<' => Step::Switch(Mode::Content), // a DOCTYPE left open
            b'[' => Step::Token(TokenKind::LBracket, p + 1, Mode::Subset),
            _ => match Quote::from_byte(b) {
                Some(q) => Step::Token(TokenKind::Quote, p + 1, Mode::Literal(q, Return::Doctype)),
                None => self.name_or_space_or_error(Mode::Doctype),
            },
        }
    }

    /// `%name;` at the current position, if it is one: where it ends.
    fn percent_ref_end(&self) -> Option<usize> {
        let after = self.pos + 1;
        if !self.char_at(after).is_some_and(is_name_start_char) {
            return None;
        }
        let end = self.name_end(after);
        (self.src.as_bytes().get(end) == Some(&b';')).then_some(end + 1)
    }

    fn subset(&self) -> Step {
        let p = self.pos;
        match self.byte() {
            b']' => Step::Token(TokenKind::RBracket, p + 1, Mode::Doctype),
            b'%' => match self.percent_ref_end() {
                Some(end) => Step::Token(TokenKind::PercentRef, end, Mode::Subset),
                None => Step::Token(TokenKind::Error, p + 1, Mode::Subset),
            },
            b'<' if self.looking_at("<!--") => Step::Token(TokenKind::CommentStart, p + 4, Mode::Comment(Return::Subset)),
            b'<' if self.looking_at("<?") => Step::Token(TokenKind::PiStart, p + 2, Mode::PiTarget(Return::Subset)),
            b'<' if self.looking_at("<!") && self.char_at(p + 2).is_some_and(is_name_start_char) => {
                Step::Token(TokenKind::DeclStart, self.name_end(p + 2), Mode::Decl)
            }
            _ if self.is_whitespace_here() => Step::Token(TokenKind::Whitespace, self.whitespace_end(), Mode::Subset),
            _ => Step::Token(TokenKind::Error, p + self.char_len(), Mode::Subset),
        }
    }

    fn decl(&self) -> Step {
        let p = self.pos;
        let b = self.byte();
        match b {
            b'>' => Step::Token(TokenKind::Gt, p + 1, Mode::Subset),
            b'<' => Step::Switch(Mode::Subset), // a declaration left open
            b'%' => match self.percent_ref_end() {
                Some(end) => Step::Token(TokenKind::PercentRef, end, Mode::Decl),
                None => Step::Token(TokenKind::Punct, p + 1, Mode::Decl),
            },
            b'(' | b')' | b'|' | b',' | b'?' | b'*' | b'+' | b'#' => Step::Token(TokenKind::Punct, p + 1, Mode::Decl),
            _ => match Quote::from_byte(b) {
                Some(q) => Step::Token(TokenKind::Quote, p + 1, Mode::Literal(q, Return::Decl)),
                None => self.name_or_space_or_error(Mode::Decl),
            },
        }
    }
}

impl Iterator for Lexer<'_> {
    type Item = Token;

    fn next(&mut self) -> Option<Token> {
        // A mode switch consumes nothing; two in a row is the most any recovery needs before some mode produces a token.
        for _ in 0..4 {
            if self.pos >= self.src.len() {
                return None;
            }
            match self.step() {
                Step::Switch(mode) => self.mode = mode,
                Step::Token(kind, end, next) => {
                    // Every mode ends tokens on a boundary past the start; this keeps the lexer total even if one did not.
                    let end = if end > self.pos && self.src.is_char_boundary(end) { end } else { self.pos + self.char_len() };
                    let token = Token { kind, start: self.pos, end };
                    self.pos = end;
                    self.mode = next;
                    return Some(token);
                }
            }
        }
        // Not reachable by construction; a character taken as an error and a fresh start still make progress.
        let token = Token { kind: TokenKind::Error, start: self.pos, end: self.pos + self.char_len() };
        self.pos = token.end;
        self.mode = Mode::Content;
        Some(token)
    }
}

#[cfg(test)]
mod tests {
    use super::TokenKind::*;
    use super::*;
    use proptest::prelude::*;

    fn lex(src: &str) -> Vec<(TokenKind, &str)> {
        Lexer::new(src).map(|t| (t.kind, &src[t.range()])).collect()
    }

    #[test]
    fn a_small_element() {
        assert_eq!(
            lex(r#"<a b="c">t</a>"#),
            [
                (TagOpen, "<"),
                (Name, "a"),
                (Whitespace, " "),
                (Name, "b"),
                (Eq, "="),
                (Quote, "\""),
                (AttrText, "c"),
                (Quote, "\""),
                (Gt, ">"),
                (Text, "t"),
                (EndTagOpen, "</"),
                (Name, "a"),
                (Gt, ">"),
            ]
        );
        assert_eq!(lex("<x:a/>"), [(TagOpen, "<"), (Name, "x:a"), (SlashGt, "/>")]);
        assert_eq!(lex("<a b='c d'/>")[5..], [(Quote, "'"), (AttrText, "c d"), (Quote, "'"), (SlashGt, "/>")]);
        assert_eq!(lex(""), []);
    }

    #[test]
    fn references() {
        assert_eq!(
            lex("a&amp;b&#65;&#x41;&bad c&;&#;&#x;"),
            [
                (Text, "a"),
                (EntityRef, "&amp;"),
                (Text, "b"),
                (CharRef, "&#65;"),
                (CharRef, "&#x41;"),
                (BareAmp, "&"),
                (Text, "bad c"),
                (BareAmp, "&"),
                (Text, ";"),
                (BareAmp, "&"),
                (Text, "#;"),
                (BareAmp, "&"),
                (Text, "#x;"),
            ]
        );
        let attr = lex(r#"<a b="x&lt;&#10;&">"#);
        assert!(attr.contains(&(EntityRef, "&lt;")) && attr.contains(&(CharRef, "&#10;")) && attr.contains(&(BareAmp, "&")));
    }

    #[test]
    fn no_token_spans_a_line_break() {
        assert_eq!(lex("a\nb\n\nc"), [(Text, "a\n"), (Text, "b\n"), (Text, "\n"), (Text, "c")]);
        assert_eq!(
            lex("<a\n  b = \"x\ny\"\r\n>"),
            [
                (TagOpen, "<"),
                (Name, "a"),
                (Whitespace, "\n"),
                (Whitespace, "  "),
                (Name, "b"),
                (Whitespace, " "),
                (Eq, "="),
                (Whitespace, " "),
                (Quote, "\""),
                (AttrText, "x\n"),
                (AttrText, "y"),
                (Quote, "\""),
                (Whitespace, "\r\n"),
                (Gt, ">"),
            ]
        );
    }

    #[test]
    fn comments_cdata_and_processing_instructions() {
        assert_eq!(lex("<!-- a\n b -->x"), [(CommentStart, "<!--"), (CommentText, " a\n"), (CommentText, " b "), (CommentEnd, "-->"), (Text, "x")]);
        assert_eq!(lex("<!---->"), [(CommentStart, "<!--"), (CommentEnd, "-->")]);
        assert_eq!(lex("<!-- a"), [(CommentStart, "<!--"), (CommentText, " a")], "unterminated: no end token");
        assert_eq!(lex("<!--->"), [(CommentStart, "<!--"), (CommentText, "->")]);
        assert_eq!(lex("<!--a--->"), [(CommentStart, "<!--"), (CommentText, "a-"), (CommentEnd, "-->")]);
        assert_eq!(lex("<![CDATA[a<b]]>c"), [(CdataStart, "<![CDATA["), (CdataText, "a<b"), (CdataEnd, "]]>"), (Text, "c")]);
        assert_eq!(lex("<![CDATA[]]>"), [(CdataStart, "<![CDATA["), (CdataEnd, "]]>")]);
        assert_eq!(lex("<?php echo ?>"), [(PiStart, "<?"), (PiTarget, "php"), (PiText, " echo "), (PiEnd, "?>")]);
        assert_eq!(lex("<?x?>"), [(PiStart, "<?"), (PiTarget, "x"), (PiEnd, "?>")]);
        assert_eq!(lex("<??>"), [(PiStart, "<?"), (PiEnd, "?>")], "a processing instruction without a target");
        assert_eq!(lex("<?xml-stylesheet href=\"a\"?>"), [(PiStart, "<?"), (PiTarget, "xml-stylesheet"), (PiText, " href=\"a\""), (PiEnd, "?>")]);
    }

    #[test]
    fn the_xml_declaration_has_pseudo_attributes() {
        assert_eq!(
            lex("<?xml version=\"1.0\" encoding='UTF-8'?><a/>"),
            [
                (PiStart, "<?"),
                (PiTarget, "xml"),
                (Whitespace, " "),
                (Name, "version"),
                (Eq, "="),
                (Quote, "\""),
                (LiteralText, "1.0"),
                (Quote, "\""),
                (Whitespace, " "),
                (Name, "encoding"),
                (Eq, "="),
                (Quote, "'"),
                (LiteralText, "UTF-8"),
                (Quote, "'"),
                (PiEnd, "?>"),
                (TagOpen, "<"),
                (Name, "a"),
                (SlashGt, "/>"),
            ]
        );
        // Left open, the declaration ends where the next markup begins.
        assert_eq!(lex("<?xml version=\"1.0\n<a/>").last(), Some(&(SlashGt, "/>")));
    }

    #[test]
    fn doctype_with_an_internal_subset() {
        assert_eq!(
            lex(r#"<!DOCTYPE a SYSTEM "a.dtd">"#),
            [
                (DoctypeStart, "<!DOCTYPE"),
                (Whitespace, " "),
                (Name, "a"),
                (Whitespace, " "),
                (Name, "SYSTEM"),
                (Whitespace, " "),
                (Quote, "\""),
                (LiteralText, "a.dtd"),
                (Quote, "\""),
                (Gt, ">"),
            ]
        );
        assert_eq!(
            lex(r#"<!DOCTYPE a [<!ELEMENT a (#PCDATA)><!ENTITY e "x>y"><!-- c --><?p q?>%pe;]>t"#),
            [
                (DoctypeStart, "<!DOCTYPE"),
                (Whitespace, " "),
                (Name, "a"),
                (Whitespace, " "),
                (LBracket, "["),
                (DeclStart, "<!ELEMENT"),
                (Whitespace, " "),
                (Name, "a"),
                (Whitespace, " "),
                (Punct, "("),
                (Punct, "#"),
                (Name, "PCDATA"),
                (Punct, ")"),
                (Gt, ">"),
                (DeclStart, "<!ENTITY"),
                (Whitespace, " "),
                (Name, "e"),
                (Whitespace, " "),
                (Quote, "\""),
                (LiteralText, "x>y"),
                (Quote, "\""),
                (Gt, ">"),
                (CommentStart, "<!--"),
                (CommentText, " c "),
                (CommentEnd, "-->"),
                (PiStart, "<?"),
                (PiTarget, "p"),
                (PiText, " q"),
                (PiEnd, "?>"),
                (PercentRef, "%pe;"),
                (RBracket, "]"),
                (Gt, ">"),
                (Text, "t"),
            ]
        );
        // A parameter entity declaration: `%` followed by a space is punctuation, not a reference.
        assert!(lex("<!DOCTYPE a [<!ENTITY % p \"v\">]>").contains(&(Punct, "%")));
    }

    #[test]
    fn dtd_text_lexes_in_subset_mode() {
        let src = "<!ELEMENT a EMPTY>\n";
        let kinds: Vec<_> = Lexer::with_mode(src, Mode::Subset).map(|t| t.kind).collect();
        assert_eq!(kinds, [DeclStart, Whitespace, Name, Whitespace, Name, Gt, Whitespace]);
    }

    #[test]
    fn broken_markup_recovers_at_the_next_tag() {
        // A value left open: the next `<` starts a tag.
        assert_eq!(
            lex("<a href=\"x <b>y</b>")[5..],
            [(Quote, "\""), (AttrText, "x "), (TagOpen, "<"), (Name, "b"), (Gt, ">"), (Text, "y"), (EndTagOpen, "</"), (Name, "b"), (Gt, ">")]
        );
        // A tag left open.
        assert_eq!(lex("<a <b>"), [(TagOpen, "<"), (Name, "a"), (Whitespace, " "), (TagOpen, "<"), (Name, "b"), (Gt, ">")]);
        // A DOCTYPE and a declaration left open.
        assert_eq!(lex("<!DOCTYPE a <b/>").last(), Some(&(SlashGt, "/>")));
        let open_decl = lex("<!DOCTYPE a [<!ELEMENT a\n<!ELEMENT b EMPTY>]>");
        assert_eq!(open_decl.iter().filter(|(k, _)| *k == DeclStart).count(), 2);
    }

    #[test]
    fn stray_characters_become_error_tokens() {
        assert_eq!(lex("a < b"), [(Text, "a "), (BareLt, "<"), (Text, " b")]);
        assert_eq!(lex("<"), [(BareLt, "<")]);
        assert_eq!(lex("<1>"), [(BareLt, "<"), (Text, "1>")]);
        assert_eq!(lex("<!x>"), [(Error, "<!"), (Text, "x>")]);
        assert_eq!(lex("<a / b ;>").iter().filter(|(k, _)| *k == Error).count(), 2);
        assert_eq!(lex("<a é>"), [(TagOpen, "<"), (Name, "a"), (Whitespace, " "), (Name, "é"), (Gt, ">")]);
        assert_eq!(lex("<a \u{A0}>")[3], (Error, "\u{A0}"));
    }

    #[test]
    fn multi_byte_text_and_names() {
        let kinds: Vec<_> = lex("<日本>é😀</日本>").iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds, [TagOpen, Name, Gt, Text, EndTagOpen, Name, Gt]);
    }

    #[test]
    fn resuming_from_a_checkpoint_inside_a_comment() {
        let src = "<!-- a\n b -->x";
        let mut lexer = Lexer::new(src);
        lexer.next();
        lexer.next();
        let at = lexer.checkpoint();
        assert_eq!(at, Checkpoint { offset: 7, mode: Mode::Comment(Return::Content) });
        let rest: Vec<_> = Lexer::resume(src, at).map(|t| (t.kind, &src[t.range()])).collect();
        assert_eq!(rest, [(CommentText, " b "), (CommentEnd, "-->"), (Text, "x")]);
    }

    #[test]
    fn a_stale_checkpoint_is_harmless() {
        let src = "é<a/>";
        assert_eq!(Lexer::resume(src, Checkpoint { offset: 1, mode: Mode::Tag }).offset(), 2, "inside a character: moved on");
        assert_eq!(Lexer::resume(src, Checkpoint { offset: 999, mode: Mode::Tag }).count(), 0, "past the end: nothing left");
    }

    /// Pieces that make interesting markup when glued together at random.
    const PIECES: &[&str] = &[
        "<",
        "</",
        ">",
        "/>",
        "/",
        "\"",
        "'",
        "=",
        "&amp;",
        "&#65;",
        "&#x;",
        "&",
        ";",
        "<!--",
        "-->",
        "--",
        "<![CDATA[",
        "]]>",
        "]",
        "<?xml",
        "<?",
        "?>",
        "?",
        "<!DOCTYPE",
        "<!ELEMENT",
        "<!ENTITY",
        "[",
        "%e;",
        "%",
        "#",
        "(",
        ")",
        "|",
        " ",
        "\t",
        "\n",
        "\r\n",
        "\r",
        "a",
        "ab:c",
        "é",
        "日",
        "😀",
        "-",
        ".",
        "1",
        "\u{A0}",
        "\0",
    ];

    fn markup() -> impl Strategy<Value = String> {
        proptest::collection::vec(proptest::sample::select(PIECES), 0..60).prop_map(|v| v.concat())
    }

    proptest! {
        /// Whatever the text, the tokens tile it exactly, none is empty, and none has a line feed before its last byte.
        #[test]
        fn tokens_tile_the_text(src in prop_oneof![markup(), "\\PC{0,200}".boxed()]) {
            let mut at = 0;
            for t in Lexer::new(&src) {
                prop_assert_eq!(t.start, at);
                prop_assert!(t.end > t.start && src.is_char_boundary(t.end));
                let text = &src[t.range()];
                prop_assert!(!text.trim_end_matches('\n').contains('\n'), "line feed inside {:?}", text);
                at = t.end;
            }
            prop_assert_eq!(at, src.len());
        }

        /// The restart property: from any token boundary, with the mode recorded there, the lexer produces exactly the rest.
        #[test]
        fn lexing_can_restart_at_any_token_boundary(src in prop_oneof![markup(), "\\PC{0,200}".boxed()]) {
            let mut lexer = Lexer::new(&src);
            let mut points = Vec::new();
            let mut all = Vec::new();
            loop {
                points.push((lexer.checkpoint(), all.len()));
                match lexer.next() {
                    Some(t) => all.push(t),
                    None => break,
                }
            }
            for (cp, index) in points {
                let rest: Vec<Token> = Lexer::resume(&src, cp).collect();
                prop_assert_eq!(&rest[..], &all[index..], "restart at {:?}", cp);
            }
        }

        /// At most one token per byte, so a lexer cannot loop.
        #[test]
        fn the_lexer_always_finishes(src in "\\PC{0,300}") {
            prop_assert!(Lexer::new(&src).count() <= src.len());
        }
    }
}
