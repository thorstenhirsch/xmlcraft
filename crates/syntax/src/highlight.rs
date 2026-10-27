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
///
/// A name right after a tag opener is an element name, but this does not know that the previous line ended with an opener; a [`Highlighter`]
/// does, and carries that across line breaks.
pub fn highlight_line(line: &str, start: Mode) -> (Vec<Span>, Mode) {
    let mut spans = Vec::new();
    let (mode, _) = lex_line(line, start, false, Some(&mut spans));
    (spans, mode)
}

/// What a line ended with, which is what the next line starts from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct LineStart {
    mode: Mode,
    /// The last token that was not whitespace opened a tag (`<` or `</`), so a name now is an element name.
    after_open: bool,
}

/// Lex one line from `start`, with `after_open` saying whether the line before ended by opening a tag. Fills `spans` when it is given
/// (a pass that only needs where the line ends leaves it out and skips the classification). Returns the mode and `after_open` at the end.
fn lex_line(line: &str, start: Mode, after_open: bool, mut spans: Option<&mut Vec<Span>>) -> (Mode, bool) {
    let mut lexer = Lexer::with_mode(line, start);
    let mut after_open = after_open;
    loop {
        // What kind of markup the token is in depends on where the lexer was when it started.
        let before = lexer.mode();
        let Some(token) = lexer.next() else { break };
        if let Some(spans) = spans.as_deref_mut() {
            let class = classify(token.kind, before, after_open);
            if class != HighlightClass::Plain {
                match spans.last_mut() {
                    Some((range, last)) if *last == class && range.end == token.start => range.end = token.end,
                    _ => spans.push((token.range(), class)),
                }
            }
        }
        match token.kind {
            TokenKind::TagOpen | TokenKind::EndTagOpen => after_open = true,
            TokenKind::Whitespace => {}
            _ => after_open = false,
        }
    }
    (lexer.mode(), after_open)
}

fn classify(kind: TokenKind, before: Mode, after_open: bool) -> HighlightClass {
    match kind {
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
    }
}

/// Colours the lines of a buffer, remembering the lexer's mode at the start of the lines it has looked at.
#[derive(Clone, Debug, Default)]
pub struct Highlighter {
    /// `starts[i]` is the state at the start of line `i`; known for the first `starts.len()` lines.
    starts: Vec<LineStart>,
}

impl Highlighter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget everything from `line` on: an edit started there (or earlier).
    pub fn invalidate_from(&mut self, line: usize) {
        self.starts.truncate(line.saturating_add(1));
    }

    /// Forget everything (a new document).
    pub fn reset(&mut self) {
        self.starts.clear();
    }

    /// How many lines' start modes are known.
    pub fn known_lines(&self) -> usize {
        self.starts.len()
    }

    /// The text of `line` including its break, or `None` past the end.
    fn line_with_break(buffer: &Buffer, line: usize) -> Option<String> {
        let range = buffer.line_range_with_break(line)?;
        buffer.slice(range).ok()
    }

    /// The state at the start of `line`, lexing forward from the last known line if needed. The lines before are only run through for
    /// where they end, without building their spans.
    fn start_of(&mut self, buffer: &Buffer, line: usize) -> LineStart {
        if self.starts.is_empty() {
            self.starts.push(LineStart::default());
        }
        while self.starts.len() <= line {
            let at = self.starts.len() - 1;
            let start = self.starts.get(at).copied().unwrap_or_default();
            let next = match Self::line_with_break(buffer, at) {
                Some(text) => {
                    let (mode, after_open) = lex_line(&text, start.mode, start.after_open, None);
                    LineStart { mode, after_open }
                }
                None => break, // past the end of the text
            };
            self.starts.push(next);
        }
        self.starts.get(line).copied().unwrap_or_default()
    }

    /// Where in the markup `offset` is, as the lexer sees it: in content, in a tag, in an attribute value (and which quote opened it), a
    /// comment and so on. Lexes from the start of the line `offset` is on, using the modes of the lines before it that are known. An
    /// offset past the end is the end; one inside a character is the character's start.
    pub fn mode_at(&mut self, buffer: &Buffer, offset: usize) -> Mode {
        // An offset inside a character is the character's start.
        let offset = if offset >= buffer.len() || buffer.char_at(offset).is_some() { offset.min(buffer.len()) } else { buffer.prev_char(offset) };
        let line = buffer.line_of(offset);
        let start = self.start_of(buffer, line);
        let Some(line_start) = buffer.line_start(line) else { return start.mode };
        let Ok(before) = buffer.slice(line_start..offset) else { return start.mode };
        lex_line(&before, start.mode, start.after_open, None).0
    }

    /// The coloured parts of `line` (byte ranges of the line's text without its break).
    pub fn spans(&mut self, buffer: &Buffer, line: usize) -> Vec<Span> {
        let start = self.start_of(buffer, line);
        let Some(text) = Self::line_with_break(buffer, line) else { return Vec::new() };
        let mut spans = Vec::new();
        lex_line(&text, start.mode, start.after_open, Some(&mut spans));
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

    fn mode_at_caret(text: &str) -> Mode {
        let offset = text.find('|').expect("a caret");
        let buffer = Buffer::from_text(&text.replacen('|', "", 1));
        Highlighter::new().mode_at(&buffer, offset)
    }

    #[test]
    fn the_mode_at_an_offset_says_where_in_the_markup_it_is() {
        use crate::lexer::{Quote, Return};
        assert_eq!(mode_at_caret("|<a b=\"c\">"), Mode::Content);
        assert_eq!(mode_at_caret("<a| b=\"c\">"), Mode::Tag);
        assert_eq!(mode_at_caret("<a b=|\"c\">"), Mode::Tag);
        assert_eq!(mode_at_caret("<a b=\"|c\">"), Mode::AttrValue(Quote::Double));
        assert_eq!(mode_at_caret("<a b='c|'>"), Mode::AttrValue(Quote::Single));
        assert_eq!(mode_at_caret("<a b=\"c\"|>"), Mode::Tag);
        assert_eq!(mode_at_caret("<a b=\"c\">|text"), Mode::Content);
        assert_eq!(mode_at_caret("<!-- a| -->"), Mode::Comment(Return::Content));
        assert_eq!(mode_at_caret("<![CDATA[ a| ]]>"), Mode::Cdata);
        // Earlier lines count: a value and a comment that go on from line to line.
        assert_eq!(mode_at_caret("<a b=\"x\n|y\">"), Mode::AttrValue(Quote::Double));
        assert_eq!(mode_at_caret("<!--\n\n|-->"), Mode::Comment(Return::Content));
        assert_eq!(mode_at_caret("<a\n  b=|\n>"), Mode::Tag);
        // The end of the text, and offsets that are not positions in it.
        assert_eq!(mode_at_caret("<a b=\"|"), Mode::AttrValue(Quote::Double));
        let buffer = Buffer::from_text("<a é=\"x\">");
        let mut h = Highlighter::new();
        assert_eq!(h.mode_at(&buffer, 999), Mode::Content, "past the end is the end");
        assert_eq!(h.mode_at(&buffer, 4), Mode::Tag, "inside the é (bytes 3 and 4) is the start of the é");
        assert_eq!(h.mode_at(&buffer, 5), Mode::Tag, "just after it");
        assert_eq!(Highlighter::new().mode_at(&Buffer::from_text(""), 0), Mode::Content);
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
    fn a_name_after_an_opener_on_the_line_before_is_still_an_element_name() {
        // `</` and the name are apart, which XML does not allow but the parser recovers from: the name is the end tag's.
        let buffer = Buffer::from_text("</\na>\n<b\nc=\"1\"/>");
        let mut h = Highlighter::new();
        assert_eq!(h.spans(&buffer, 1)[0], (0..1, ElementName), "`a` after `</` on the line above");
        // After an attribute name instead, a name on the next line is an attribute.
        assert_eq!(h.spans(&buffer, 3)[0], (0..1, AttributeName));
        // A name after a tag opener and a blank line.
        let buffer = Buffer::from_text("</\n\nx>");
        assert_eq!(Highlighter::new().spans(&buffer, 2)[0], (0..1, ElementName));
        // The same holds when the line is asked for first (the state is found by running the lines before it) and after an edit.
        let mut h = Highlighter::new();
        assert_eq!(h.spans(&buffer, 2)[0], (0..1, ElementName));
        h.invalidate_from(1);
        assert_eq!(h.spans(&buffer, 2)[0], (0..1, ElementName));
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

        /// The mode at any offset is the mode the lexer is in after lexing everything before it, from the top.
        #[test]
        fn the_mode_at_an_offset_is_what_lexing_the_text_before_it_gives(
            pieces in proptest::collection::vec(proptest::sample::select(PIECES), 0..60),
            fraction in 0.0f64..=1.0,
            invalidate in 0usize..8,
        ) {
            let text = pieces.concat();
            let buffer = Buffer::from_text(&text);
            let mut h = Highlighter::new();
            // Some lines known, some forgotten: the answer does not depend on what was cached.
            let _ = h.spans(&buffer, 3);
            h.invalidate_from(invalidate);
            let mut offset = (text.len() as f64 * fraction) as usize;
            while !text.is_char_boundary(offset) {
                offset -= 1;
            }
            let mut lexer = Lexer::new(&text[..offset]);
            for _ in lexer.by_ref() {}
            prop_assert_eq!(h.mode_at(&buffer, offset), lexer.mode());
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
