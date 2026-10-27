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

/// The stretch of text an estimate at `at` (the start of a line) lexes: from the first line that starts no further back than
/// [`ESTIMATE_WINDOW`] bytes, to `at`. Counted in bytes, not lines, so a very long line before `at` cannot make it read more.
fn window_before(buffer: &Buffer, at: usize) -> Range<usize> {
    let wanted = at.saturating_sub(ESTIMATE_WINDOW);
    let line = buffer.line_of(wanted);
    let start = buffer.line_start(line).unwrap_or(0);
    // A line that starts before the window is not read whole: the window begins at the next one.
    let from = if start >= wanted { start } else { buffer.line_start(line + 1).unwrap_or(at) };
    from.min(at)..at
}

/// The state at the start of `line`, guessed from the text just before it: the lexer is run over the window before it (see
/// [`window_before`]) from the state of content, which is the state at the start of most lines. Everything the lexer knows is
/// found that way (a comment, a CDATA section, a processing instruction, a tag, a value or a declaration that goes over lines, and what a
/// comment inside a DOCTYPE returns to), as long as it begins in the window.
fn estimate_start(buffer: &Buffer, line: usize) -> LineStart {
    let Some(at) = buffer.line_start(line) else { return LineStart::default() };
    let Ok(window) = buffer.slice(window_before(buffer, at)) else { return LineStart::default() };
    let mut state = LineStart::default();
    for text in window.split_inclusive('\n') {
        let (mode, after_open) = lex_line(text, state.mode, state.after_open, None);
        state = LineStart { mode, after_open };
    }
    state
}

/// Colours the lines of a buffer, remembering the lexer's mode at the start of the lines it has looked at.
#[derive(Clone, Debug, Default)]
pub struct Highlighter {
    /// `starts[i]` is the state at the start of line `i`; known for the first `starts.len()` lines.
    starts: Vec<LineStart>,
    /// Stretches of lines past those, whose first line's state is an estimate (see [`FarJump`]), in the order of the text.
    islands: Vec<Island>,
    /// How a jump far past what is known is treated; none (the default): the lines before it are all read, so the answer is exact.
    far_jump: Option<FarJump>,
}

/// How a very large text is coloured at a place far past what has been looked at. The colour of a line depends on where the lexer is at
/// its start, which is known exactly only by reading everything before it. For a text of 100 MB that is over a second, so a jump
/// further than `lines` past what is known starts from an estimate instead: the lexer is run over the 256 KiB just before, from the
/// state of content. That is right unless a construct that goes over lines (a comment, a CDATA section, a tag, a value, a processing
/// instruction, a declaration) began before that and has not ended; the colours are wrong until it ends. The estimate is dropped when
/// reading from the top of the text (a jump of up to `lines` at a time) reaches it, and by an edit before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FarJump {
    /// A line further than this past the last one whose state is known starts from an estimate.
    pub lines: usize,
    /// Lines before the one asked for that the estimate starts at, so that scrolling up a little stays in the same stretch.
    pub lookbehind: usize,
}

impl FarJump {
    /// What a very large text is coloured with: a jump of more than 2000 lines is estimated, from 64 lines before the one asked for.
    pub const LARGE_TEXT: FarJump = FarJump { lines: 2000, lookbehind: 64 };
    /// A text longer than this (in bytes) is a very large text: reading a megabyte takes about 12 ms, which a jump can afford.
    pub const LARGE_TEXT_FROM: usize = 1024 * 1024;

    /// What to use for a text of `len` bytes: [`FarJump::LARGE_TEXT`] when it is a very large one, nothing (read everything) when not.
    pub fn for_len(len: usize) -> Option<FarJump> {
        (len > Self::LARGE_TEXT_FROM).then_some(Self::LARGE_TEXT)
    }
}

/// The longest stretch of text before a line that an estimate looks at, in bytes.
const ESTIMATE_WINDOW: usize = 256 * 1024;

/// Lines whose start states were worked out from an estimate at the first of them.
#[derive(Clone, Debug)]
struct Island {
    first: usize,
    starts: Vec<LineStart>,
}

impl Highlighter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Treat a jump far past what is known as [`FarJump`] says (for a very large text), or always read everything before (`None`).
    pub fn set_far_jump(&mut self, far_jump: Option<FarJump>) {
        if self.far_jump != far_jump {
            self.far_jump = far_jump;
            self.islands.clear();
        }
    }

    /// Forget everything from `line` on: an edit started there (or earlier).
    pub fn invalidate_from(&mut self, line: usize) {
        self.starts.truncate(line.saturating_add(1));
        // Stretches that begin after the edit are about lines that moved; the one it is in is as good as far as the edit.
        self.islands.retain(|island| island.first <= line);
        if let Some(island) = self.islands.last_mut() {
            island.starts.truncate(line.saturating_add(1) - island.first);
        }
        self.islands.retain(|island| !island.starts.is_empty());
    }

    /// Forget everything (a new document).
    pub fn reset(&mut self) {
        self.starts.clear();
        self.islands.clear();
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
        if let Some(far) = self.far_jump
            && line >= self.starts.len().saturating_add(far.lines)
        {
            return self.start_in_island(buffer, line, far);
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

    /// The state at the start of `line`, far past what is known: from the island that has it, or one made for it.
    fn start_in_island(&mut self, buffer: &Buffer, line: usize, far: FarJump) -> LineStart {
        // A line the text does not have is not worth an island (and `line` can be anything: it is the caller's).
        if line >= buffer.line_count() {
            return LineStart::default();
        }
        // Islands that start in what is known now are not needed any more.
        let known = self.starts.len();
        self.islands.retain(|island| island.first >= known);
        let at = self.islands.partition_point(|island| island.first <= line);
        let near = at.checked_sub(1).and_then(|i| self.islands.get(i).map(|island| (i, island.first.saturating_add(island.starts.len()))));
        let index = match near {
            // Near the end of an island: it goes on to the line.
            Some((i, end)) if line < end.saturating_add(far.lines) => i,
            _ => {
                let first = line.saturating_sub(far.lookbehind).max(known);
                let island = Island { first, starts: vec![estimate_start(buffer, first)] };
                let insert_at = self.islands.partition_point(|other| other.first <= first);
                self.islands.insert(insert_at, island);
                insert_at
            }
        };
        // Lex forward to the line; an island that is run into on the way is replaced by what this one finds.
        while let Some(island) = self.islands.get(index) {
            let next = island.first.saturating_add(island.starts.len());
            if next > line {
                break;
            }
            let last = island.starts.last().copied().unwrap_or_default();
            let Some(text) = Self::line_with_break(buffer, next - 1) else { break };
            let (mode, after_open) = lex_line(&text, last.mode, last.after_open, None);
            if let Some(island) = self.islands.get_mut(index) {
                island.starts.push(LineStart { mode, after_open });
            }
            while self.islands.get(index + 1).is_some_and(|other| other.first <= next) {
                self.islands.remove(index + 1);
            }
        }
        self.islands.get(index).and_then(|island| island.starts.get(line.saturating_sub(island.first))).copied().unwrap_or_default()
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

    fn exact_and_estimated(far: FarJump) -> (Highlighter, Highlighter) {
        let exact = Highlighter::new();
        let mut estimated = Highlighter::new();
        estimated.set_far_jump(Some(far));
        (exact, estimated)
    }

    /// A text of `n` lines of single-line markup (nothing goes over lines).
    fn simple_lines(n: usize) -> String {
        (0..n).map(|i| format!("<a x=\"{i}\">t &amp; {i}<!-- c{i} --></a>")).collect::<Vec<_>>().join("\n")
    }

    const SMALL: FarJump = FarJump { lines: 10, lookbehind: 3 };

    #[test]
    fn a_far_jump_starts_from_an_estimate_and_is_right_when_nothing_goes_over_lines() {
        let buffer = Buffer::from_text(&simple_lines(300));
        let (mut exact, mut estimated) = exact_and_estimated(SMALL);
        for line in [250, 249, 247, 120, 299, 5, 130] {
            assert_eq!(estimated.spans(&buffer, line), exact.spans(&buffer, line), "line {line}");
        }
        // The estimate did not read what is in between: far fewer lines than the jumps cover are known in the prefix.
        assert!(estimated.known_lines() < 20, "{}", estimated.known_lines());
        assert!(exact.known_lines() >= 299);
    }

    #[test]
    fn anything_that_goes_over_lines_is_found_in_the_text_before() {
        let body = (0..60).map(|i| format!("  line {i} of text")).collect::<Vec<_>>().join("\n");
        let tag_body = (0..60).map(|i| format!("  a{i}=\"v{i}\"")).collect::<Vec<_>>().join("\n");
        let value_body = (0..60).map(|i| format!("  value line {i}")).collect::<Vec<_>>().join("\n");
        let cases = [
            ("comment", "<!--".to_owned(), body.clone(), "-->"),
            ("cdata", "<![CDATA[".to_owned(), body.clone(), "]]>"),
            ("processing instruction", "<?target".to_owned(), body.clone(), "?>"),
            ("tag", "<e".to_owned(), tag_body, ">"),
            ("attribute value", "<e a=\"".to_owned(), value_body, "\" />"),
            ("doctype", "<!DOCTYPE r [".to_owned(), "<!ELEMENT r ANY>\n<!-- c -->\n<!ENTITY e \"v\">".to_owned(), "]>"),
            // Markers of one kind inside another are text, not the start of something.
            ("comment holding markers", "<!--".to_owned(), "<![CDATA[ <?pi <e a=\" ]]> ".to_owned() + &body, "-->"),
            ("cdata holding markers", "<![CDATA[".to_owned(), "<!-- <?pi ".to_owned() + &body, "]]>"),
            ("comment inside a doctype", "<!DOCTYPE r [\n<!--".to_owned(), body, "-->\n]>"),
        ];
        for (name, open, inside, close) in cases {
            let text = format!("{}\n{open}\n{inside}\n{close}\n{}", simple_lines(100), simple_lines(100));
            let buffer = Buffer::from_text(&text);
            let (mut exact, mut estimated) = exact_and_estimated(SMALL);
            // In the middle of it, near where it ends, and after it.
            let end = 100 + buffer.line_count().saturating_sub(200);
            for line in [130, end - 2, end - 1, end, end + 1, end + 10] {
                assert_eq!(estimated.spans(&buffer, line), exact.spans(&buffer, line), "{name}, line {line}");
            }
        }
    }

    #[test]
    fn the_window_is_counted_in_bytes_from_the_start_of_a_line_and_a_long_line_does_not_widen_it() {
        let window_of = |text: &str, line: usize| {
            let buffer = Buffer::from_text(text);
            let at = buffer.line_start(line).expect("a line");
            (window_before(&buffer, at), at, buffer)
        };
        // A short text: all of it.
        let (window, at, _) = window_of("<a/>\n<b/>\n<c/>", 2);
        assert_eq!((window.start, window.end), (0, at));
        // Lines of 100 bytes: it starts at a line, and is not longer than the window.
        let text = "x".repeat(99) + "\n";
        let text = text.repeat((ESTIMATE_WINDOW / 100) * 3);
        let (window, at, buffer) = window_of(&text, (ESTIMATE_WINDOW / 100) * 2);
        assert!(window.len() <= ESTIMATE_WINDOW && window.len() > ESTIMATE_WINDOW - 100, "{}", window.len());
        assert_eq!(buffer.line_start(buffer.line_of(window.start)), Some(window.start));
        assert_eq!(window.end, at);
        // One huge line before the one asked for: it is not read (the window is empty), however long it is.
        let text = format!("{}\n<a/>", "y".repeat(ESTIMATE_WINDOW * 4));
        let (window, at, _) = window_of(&text, 1);
        assert_eq!((window.len(), window.end), (0, at));
        // A long line that starts inside the window is read to its end, which is the window's end at most.
        let text = format!("<a/>\n{}\n<b/>", "z".repeat(ESTIMATE_WINDOW / 2));
        let (window, _, _) = window_of(&text, 2);
        assert_eq!(window.start, 0);
    }

    #[test]
    fn a_construct_longer_than_the_window_is_not_found_but_the_colours_are_right_again_after_it() {
        let long = (0..40_000).map(|i| format!("text {i} text text")).collect::<Vec<_>>().join("\n");
        assert!(long.len() > ESTIMATE_WINDOW * 2);
        let text = format!("{}\n<!--\n{long}\n-->\n{}", simple_lines(30), simple_lines(30));
        let buffer = Buffer::from_text(&text);
        let (mut exact, mut estimated) = exact_and_estimated(SMALL);
        let middle = 30 + 20_000;
        assert_ne!(estimated.spans(&buffer, middle), exact.spans(&buffer, middle), "inside it the colour is a guess, and the guess is content");
        // Lines after the comment have the lexer back in content, whatever it started from.
        let after = 30 + 2 + 40_000 + 3;
        for line in [after, after + 5, after + 29] {
            assert_eq!(estimated.spans(&buffer, line), exact.spans(&buffer, line), "line {line}");
        }
    }

    #[test]
    fn a_line_the_text_does_not_have_is_no_trouble_in_a_far_jump() {
        let buffer = Buffer::from_text(&simple_lines(50));
        for lookbehind in [0, 1, 64] {
            let mut estimated = Highlighter::new();
            estimated.set_far_jump(Some(FarJump { lines: 2, lookbehind }));
            for line in [50, 51, 1_000_000, usize::MAX - 1, usize::MAX, 20, usize::MAX] {
                let spans = estimated.spans(&buffer, line);
                assert!(line >= 50 && spans.is_empty() || line < 50 && !spans.is_empty(), "line {line}: {spans:?}");
            }
            assert_eq!(estimated.mode_at(&buffer, usize::MAX), Mode::Content);
        }
    }

    #[test]
    fn an_edit_makes_the_estimates_after_it_forgotten() {
        let mut buffer = Buffer::from_text(&simple_lines(200));
        let (_, mut estimated) = exact_and_estimated(SMALL);
        estimated.spans(&buffer, 150);
        estimated.spans(&buffer, 180);
        // A comment is opened at line 100: what was worked out for the lines after it is not right any more.
        buffer.apply(&xmlcraft_text::Edit::insert(buffer.line_start(100).unwrap_or(0), "<!--\n")).expect("inserts");
        estimated.invalidate_from(100);
        let mut exact = Highlighter::new();
        for line in [150, 180, 101] {
            assert_eq!(estimated.spans(&buffer, line), exact.spans(&buffer, line), "line {line}");
        }
        assert!(estimated.known_lines() <= 101);
        estimated.reset();
        assert_eq!(estimated.known_lines(), 0);
    }

    #[test]
    fn mode_at_works_in_a_far_place_too() {
        let buffer = Buffer::from_text(&simple_lines(200));
        let (mut exact, mut estimated) = exact_and_estimated(SMALL);
        let offset = buffer.line_start(170).unwrap_or(0) + 6;
        assert_eq!(estimated.mode_at(&buffer, offset), exact.mode_at(&buffer, offset));
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

        /// With nothing that goes over lines, estimating gives what reading everything gives, for any lines in any order.
        #[test]
        fn estimating_agrees_with_reading_everything_when_no_construct_goes_over_lines(
            lines in 1usize..120,
            order in proptest::collection::vec(0usize..130, 1..30),
            far_lines in 1usize..20,
            lookbehind in 0usize..10,
        ) {
            let buffer = Buffer::from_text(&simple_lines(lines));
            let (mut exact, mut estimated) = exact_and_estimated(FarJump { lines: far_lines, lookbehind });
            for line in order {
                prop_assert_eq!(estimated.spans(&buffer, line), exact.spans(&buffer, line), "line {}", line);
            }
        }

        /// A text that fits in the window is read from its start by an estimate, so estimating is exact for any text at all.
        #[test]
        fn estimating_in_a_text_that_fits_the_window_is_exact_whatever_goes_over_lines(
            pieces in proptest::collection::vec(proptest::sample::select(PIECES), 0..80),
            order in proptest::collection::vec(0usize..40, 1..20),
            far_lines in 1usize..6,
            lookbehind in 0usize..4,
        ) {
            let buffer = Buffer::from_text(&pieces.concat());
            let (mut exact, mut estimated) = exact_and_estimated(FarJump { lines: far_lines, lookbehind });
            for line in order {
                prop_assert_eq!(estimated.spans(&buffer, line), exact.spans(&buffer, line), "line {}", line);
            }
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
