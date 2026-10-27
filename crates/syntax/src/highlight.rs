//! Syntax colouring: what each part of a line is, found by lexing one line at a time.
//!
//! The lexer's whole state is a [`Mode`] and no token spans a line break, so a line can be lexed alone given the mode at its start. A
//! [`Highlighter`] remembers the mode at the start of every line it has seen, lexes forward only as far as the line asked for, and
//! throws away what an edit made stale.

use std::ops::Range;

use xmlcraft_text::Buffer;

use crate::lexer::{Lexer, Mode, Return, TokenKind};

/// What a stretch of text is, for colouring.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HighlightClass {
    /// Character data, and whitespace: drawn in the plain text colour.
    Plain,
    /// `<`, `>`, `</`, `/>`, `=`, and the quotes around a value.
    Punctuation,
    ElementName,
    AttributeName,
    /// The text of an attribute value, quotes included.
    AttributeValue,
    /// `&amp;`, `&#65;`, `%name;`.
    Reference,
    Comment,
    /// `<![CDATA[`, `]]>`.
    CdataMarkup,
    CdataText,
    /// `<?`, the target, the data and `?>` of a processing instruction, and the XML declaration's markup.
    ProcessingInstruction,
    /// The pseudo-attributes of the XML declaration.
    DeclarationName,
    DeclarationValue,
    /// `<!DOCTYPE`, `<!ELEMENT` and its kin, and the keywords inside.
    Declaration,
    /// Quoted literals in a DOCTYPE or declaration.
    Literal,
    /// Something that is not well-formed: a bare `<` or `&`, a stray character.
    Error,
}

/// A coloured part of a line, as a byte range of the line.
pub type Span = (Range<usize>, HighlightClass);

/// Classify the tokens of one line. `line` is the line's text including its line break, if it has one; `start` is the lexer's mode at the
/// start of the line. Returns the spans (adjacent ones of the same class joined, plain text left out) and the mode at the end of the line,
/// which is the next line's start.
pub fn highlight_line(line: &str, start: Mode) -> (Vec<Span>, Mode) {
    let mut lexer = Lexer::with_mode(line, start);
    let mut spans: Vec<Span> = Vec::new();
    // Whether the last meaningful token opened a tag, so that the name after it is an element name.
    let mut after_open = false;
    loop {
        // What kind of markup the token is in depends on where the lexer was when it started.
        let before = lexer.mode();
        let Some(token) = lexer.next() else { break };
        let class = match token.kind {
            TokenKind::Text | TokenKind::Whitespace | TokenKind::Punct => HighlightClass::Plain,
            TokenKind::CharRef | TokenKind::EntityRef | TokenKind::PercentRef => HighlightClass::Reference,
            TokenKind::BareAmp | TokenKind::BareLt | TokenKind::Error => HighlightClass::Error,
            TokenKind::Gt if matches!(before, Mode::Doctype | Mode::Decl) => HighlightClass::Declaration,
            TokenKind::TagOpen | TokenKind::EndTagOpen | TokenKind::Gt | TokenKind::SlashGt | TokenKind::Eq => HighlightClass::Punctuation,
            TokenKind::Name => match before {
                Mode::XmlDecl => HighlightClass::DeclarationName,
                Mode::Doctype | Mode::Decl | Mode::Subset => HighlightClass::Declaration,
                _ if after_open => HighlightClass::ElementName,
                _ => HighlightClass::AttributeName,
            },
            TokenKind::Quote => match before {
                Mode::Tag | Mode::AttrValue(_) => HighlightClass::AttributeValue,
                Mode::XmlDecl | Mode::Literal(_, Return::XmlDecl) => HighlightClass::DeclarationValue,
                Mode::Doctype | Mode::Decl | Mode::Literal(..) => HighlightClass::Literal,
                _ => HighlightClass::Punctuation,
            },
            TokenKind::AttrText => HighlightClass::AttributeValue,
            TokenKind::LiteralText => {
                if matches!(before, Mode::Literal(_, Return::XmlDecl)) {
                    HighlightClass::DeclarationValue
                } else {
                    HighlightClass::Literal
                }
            }
            TokenKind::CommentStart | TokenKind::CommentText | TokenKind::CommentEnd => HighlightClass::Comment,
            TokenKind::CdataStart | TokenKind::CdataEnd => HighlightClass::CdataMarkup,
            TokenKind::CdataText => HighlightClass::CdataText,
            TokenKind::PiStart | TokenKind::PiTarget | TokenKind::PiText | TokenKind::PiEnd => HighlightClass::ProcessingInstruction,
            TokenKind::DoctypeStart | TokenKind::DeclStart | TokenKind::LBracket | TokenKind::RBracket => HighlightClass::Declaration,
        };
        match token.kind {
            TokenKind::TagOpen | TokenKind::EndTagOpen => after_open = true,
            TokenKind::Whitespace => {}
            _ => after_open = false,
        }
        if class == HighlightClass::Plain {
            continue;
        }
        match spans.last_mut() {
            Some((range, last)) if *last == class && range.end == token.start => range.end = token.end,
            _ => spans.push((token.range(), class)),
        }
    }
    (spans, lexer.mode())
}

/// Colours the lines of a buffer, remembering the lexer's mode at the start of the lines it has looked at.
#[derive(Clone, Debug, Default)]
pub struct Highlighter {
    /// `modes[i]` is the mode at the start of line `i`; known for the first `modes.len()` lines.
    modes: Vec<Mode>,
}

impl Highlighter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget everything from `line` on: an edit started there (or earlier).
    pub fn invalidate_from(&mut self, line: usize) {
        self.modes.truncate(line.saturating_add(1));
    }

    /// Forget everything (a new document).
    pub fn reset(&mut self) {
        self.modes.clear();
    }

    /// How many lines' start modes are known.
    pub fn known_lines(&self) -> usize {
        self.modes.len()
    }

    /// The text of `line` including its break, or `None` past the end.
    fn line_with_break(buffer: &Buffer, line: usize) -> Option<String> {
        let range = buffer.line_range_with_break(line)?;
        buffer.slice(range).ok()
    }

    /// The mode at the start of `line`, lexing forward from the last known line if needed.
    fn mode_at(&mut self, buffer: &Buffer, line: usize) -> Mode {
        if self.modes.is_empty() {
            self.modes.push(Mode::Content);
        }
        while self.modes.len() <= line {
            let at = self.modes.len() - 1;
            let start = self.modes.get(at).copied().unwrap_or_default();
            let next = match Self::line_with_break(buffer, at) {
                Some(text) => highlight_line(&text, start).1,
                None => break, // past the end of the text
            };
            self.modes.push(next);
        }
        self.modes.get(line).copied().unwrap_or_default()
    }

    /// The coloured parts of `line` (byte ranges of the line's text without its break).
    pub fn spans(&mut self, buffer: &Buffer, line: usize) -> Vec<Span> {
        let start = self.mode_at(buffer, line);
        let Some(text) = Self::line_with_break(buffer, line) else { return Vec::new() };
        let (mut spans, _) = highlight_line(&text, start);
        // The line break is not part of the line the view draws.
        let visible = buffer.line_range(line).map_or(text.len(), |r| r.len());
        spans.retain_mut(|(range, _)| {
            range.end = range.end.min(visible);
            range.start < range.end
        });
        spans
    }
}

#[cfg(test)]
mod tests {
    use super::HighlightClass::*;
    use super::*;
    use proptest::prelude::*;

    fn classes(line: &str) -> Vec<(&str, HighlightClass)> {
        highlight_line(line, Mode::Content).0.into_iter().map(|(r, c)| (&line[r], c)).collect()
    }

    #[test]
    fn a_tag_with_attributes() {
        assert_eq!(
            classes(r#"<a:b id="x&amp;y" ok>t</a:b>"#),
            [
                ("<", Punctuation),
                ("a:b", ElementName),
                ("id", AttributeName),
                ("=", Punctuation),
                ("\"x", AttributeValue),
                ("&amp;", Reference),
                ("y\"", AttributeValue),
                ("ok", AttributeName),
                (">", Punctuation),
                ("</", Punctuation),
                ("a:b", ElementName),
                (">", Punctuation),
            ]
        );
    }

    #[test]
    fn comments_cdata_pis_and_the_declaration() {
        assert_eq!(classes("<!-- c -->x"), [("<!-- c -->", Comment)]);
        assert_eq!(classes("<![CDATA[a<b]]>"), [("<![CDATA[", CdataMarkup), ("a<b", CdataText), ("]]>", CdataMarkup)]);
        assert_eq!(classes("<?php echo ?>"), [("<?php echo ?>", ProcessingInstruction)]);
        assert_eq!(
            classes("<?xml version=\"1.0\"?>"),
            [
                ("<?xml", ProcessingInstruction),
                ("version", DeclarationName),
                ("=", Punctuation),
                ("\"1.0\"", DeclarationValue),
                ("?>", ProcessingInstruction)
            ][..]
        );
    }

    #[test]
    fn doctype_and_errors() {
        let spans = classes("<!DOCTYPE a [<!ELEMENT a EMPTY>]>");
        assert!(spans.iter().all(|(_, c)| *c == Declaration), "{spans:?}");
        assert_eq!(classes("a < b & c"), [("<", Error), ("&", Error)]);
    }

    #[test]
    fn a_line_starts_in_the_mode_the_one_before_ended_in() {
        let (_, mode) = highlight_line("<!-- open\n", Mode::Content);
        assert_eq!(mode, Mode::Comment(crate::lexer::Return::Content));
        let (spans, mode) = highlight_line("still comment --> <b>", mode);
        assert_eq!(mode, Mode::Content);
        assert_eq!(spans[0], (0..17, Comment));
        // A tag that continues on the next line: the name there is an attribute.
        let (_, mode) = highlight_line("<a\n", Mode::Content);
        assert_eq!(mode, Mode::Tag);
        assert_eq!(highlight_line("  b=\"1\">", mode).0[0], (2..3, AttributeName));
    }

    #[test]
    fn the_highlighter_lexes_lazily_and_forgets_on_invalidation() {
        let buffer = Buffer::from_text("<!--\nx\n-->\n<a/>");
        let mut h = Highlighter::new();
        assert_eq!(h.known_lines(), 0);
        assert_eq!(h.spans(&buffer, 1), [(0..1, Comment)]);
        assert_eq!(h.known_lines(), 2, "lines 0 and 1 are known, no further");
        assert_eq!(h.spans(&buffer, 3).len(), 3, "`<a/>`: punctuation, name, punctuation");
        assert_eq!(h.known_lines(), 4);
        // Editing line 2 makes it and everything after stale, but not what came before.
        h.invalidate_from(2);
        assert_eq!(h.known_lines(), 3);
        assert_eq!(h.spans(&buffer, 99), Vec::new(), "past the end: nothing, no panic");
        h.reset();
        assert_eq!(h.known_lines(), 0);
    }

    #[test]
    fn spans_leave_out_the_line_break() {
        let buffer = Buffer::from_text("<a>\r\n<b>");
        let mut h = Highlighter::new();
        for (line, want) in [(0, 3), (1, 3)] {
            let spans = h.spans(&buffer, line);
            assert_eq!(spans.iter().map(|(r, _)| r.end).max(), Some(want));
        }
    }

    /// Pieces that make interesting markup when glued together at random.
    const PIECES: &[&str] = &[
        "<",
        "</",
        ">",
        "/>",
        "\"",
        "'",
        "=",
        "&amp;",
        "&",
        "<!--",
        "-->",
        "<![CDATA[",
        "]]>",
        "<?xml",
        "<?",
        "?>",
        "<!DOCTYPE",
        "[",
        "]",
        "<!ELEMENT",
        "%e;",
        " ",
        "\n",
        "\r\n",
        "a",
        "b:c",
        "é",
        "日",
        "x=\"1\"",
        "-",
    ];

    proptest! {
        /// Lexing line by line from the modes the lines start in gives exactly the tokens of lexing the whole text.
        #[test]
        fn line_by_line_equals_the_whole_text(pieces in proptest::collection::vec(proptest::sample::select(PIECES), 0..60)) {
            let text = pieces.concat();
            let whole: Vec<(usize, usize, TokenKind)> = Lexer::new(&text).map(|t| (t.start, t.end, t.kind)).collect();
            let mut by_line = Vec::new();
            let mut mode = Mode::Content;
            let mut offset = 0;
            for line in text.split_inclusive('\n') {
                let mut lexer = Lexer::with_mode(line, mode);
                for t in lexer.by_ref() {
                    by_line.push((offset + t.start, offset + t.end, t.kind));
                }
                mode = lexer.mode();
                offset += line.len();
            }
            prop_assert_eq!(whole, by_line);
        }

        /// The highlighter never panics and its spans are inside the line, whatever the text and whatever was invalidated.
        #[test]
        fn spans_are_inside_their_lines(pieces in proptest::collection::vec(proptest::sample::select(PIECES), 0..60), invalidate in 0usize..8) {
            let text = pieces.concat();
            let buffer = Buffer::from_text(&text);
            let mut h = Highlighter::new();
            for line in (0..buffer.line_count()).rev() {
                h.invalidate_from(invalidate);
                let visible = buffer.line_range(line).map_or(0, |r| r.len());
                for (range, _) in h.spans(&buffer, line) {
                    prop_assert!(range.start < range.end && range.end <= visible, "{range:?} in a line of {visible} bytes");
                }
            }
        }
    }
}
