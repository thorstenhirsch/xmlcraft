//! The text buffer: a rope with checked offsets.

use std::ops::Range;

use crop::Rope;

/// Why an offset or range was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextError {
    /// An offset past the end of the text.
    OutOfRange { offset: usize, len: usize },
    /// An offset inside a multi-byte character.
    NotCharBoundary { offset: usize },
    /// A range that ends before it starts.
    ReversedRange { start: usize, end: usize },
    /// The undo history does not match the text: it was changed behind the history's back.
    HistoryMismatch,
}

impl std::fmt::Display for TextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TextError::OutOfRange { offset, len } => write!(f, "offset {offset} is past the end of the text ({len} bytes)"),
            TextError::NotCharBoundary { offset } => write!(f, "offset {offset} is inside a character"),
            TextError::ReversedRange { start, end } => write!(f, "the range {start}..{end} ends before it starts"),
            TextError::HistoryMismatch => f.write_str("the undo history no longer matches the text"),
        }
    }
}

impl std::error::Error for TextError {}

/// Replace `range` with `text`. An insertion is an empty range; a deletion is empty text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

impl Edit {
    pub fn insert(offset: usize, text: impl Into<String>) -> Self {
        Self { range: offset..offset, text: text.into() }
    }

    pub fn delete(range: Range<usize>) -> Self {
        Self { range, text: String::new() }
    }

    pub fn replace(range: Range<usize>, text: impl Into<String>) -> Self {
        Self { range, text: text.into() }
    }

    /// How many bytes longer (or, negative, shorter) the text becomes.
    pub fn delta(&self) -> isize {
        self.text.len() as isize - (self.range.end.saturating_sub(self.range.start)) as isize
    }
}

/// `text` with `edits` applied. The edits are ascending, disjoint and on character boundaries, as the functions that make them return
/// them; one that is not (out of order, past the end, inside a character) is left out rather than applied wrongly.
pub fn apply_edits(text: &str, edits: &[Edit]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for edit in edits {
        let (Some(before), Some(_)) = (text.get(at..edit.range.start), text.get(edit.range.clone())) else { continue };
        out.push_str(before);
        out.push_str(&edit.text);
        at = edit.range.end;
    }
    out.push_str(text.get(at..).unwrap_or_default());
    out
}

/// A place in the text by line and column. Both start at 0; the column counts characters from the start of the line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Position {
    pub line: usize,
    pub col: usize,
}

/// The text of one document. Cloning is cheap (the rope shares its pieces), so a clone is a snapshot another thread can read while
/// the original goes on changing.
#[derive(Clone, Debug, Default)]
pub struct Buffer {
    rope: Rope,
}

impl Buffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_text(text: &str) -> Self {
        Self { rope: Rope::from(text) }
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.rope.byte_len()
    }

    pub fn is_empty(&self) -> bool {
        self.rope.is_empty()
    }

    /// The whole text. Copies it: prefer [`Self::chunks`] or [`Self::slice`] for big documents.
    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// The text in pieces, in order.
    pub fn chunks(&self) -> impl Iterator<Item = &str> {
        self.rope.chunks()
    }

    /// Whether `offset` is inside the text and on a character boundary.
    pub fn is_boundary(&self, offset: usize) -> bool {
        offset <= self.len() && self.rope.is_char_boundary(offset)
    }

    fn check(&self, offset: usize) -> Result<(), TextError> {
        if offset > self.len() {
            return Err(TextError::OutOfRange { offset, len: self.len() });
        }
        if !self.rope.is_char_boundary(offset) {
            return Err(TextError::NotCharBoundary { offset });
        }
        Ok(())
    }

    /// Check that `range` is a valid, ordered range of the text.
    pub fn check_range(&self, range: &Range<usize>) -> Result<(), TextError> {
        if range.end < range.start {
            return Err(TextError::ReversedRange { start: range.start, end: range.end });
        }
        self.check(range.start)?;
        self.check(range.end)
    }

    /// The text of `range`.
    pub fn slice(&self, range: Range<usize>) -> Result<String, TextError> {
        self.check_range(&range)?;
        Ok(self.rope.byte_slice(range).to_string())
    }

    /// The byte at `offset`, if there is one.
    pub fn byte(&self, offset: usize) -> Option<u8> {
        (offset < self.len()).then(|| self.rope.byte(offset))
    }

    /// The character starting at `offset`, if `offset` is the start of one.
    pub fn char_at(&self, offset: usize) -> Option<char> {
        if offset >= self.len() || !self.rope.is_char_boundary(offset) {
            return None;
        }
        let end = (offset + 4).min(self.len());
        // A character is at most 4 bytes; the slice ends mid-character at worst, and `chars().next()` still decodes the first.
        self.rope.byte_slice(offset..self.next_boundary_at_or_after(end)).chars().next()
    }

    fn next_boundary_at_or_after(&self, mut offset: usize) -> usize {
        while offset < self.len() && !self.rope.is_char_boundary(offset) {
            offset += 1;
        }
        offset.min(self.len())
    }

    /// The boundary after the character that starts at `offset` (or the end of the text).
    pub fn next_char(&self, offset: usize) -> usize {
        if offset >= self.len() {
            return self.len();
        }
        match self.char_at(offset) {
            Some(c) => offset + c.len_utf8(),
            // Inside a character: the next boundary.
            None => self.next_boundary_at_or_after(offset),
        }
    }

    /// The boundary before the character that ends at `offset` (or 0).
    pub fn prev_char(&self, offset: usize) -> usize {
        let mut at = offset.min(self.len());
        while at > 0 {
            at -= 1;
            if self.rope.is_char_boundary(at) {
                return at;
            }
        }
        0
    }

    /// The length in bytes of the longest line, line break not counted (a `\r` before a `\n` is). One pass over the text's chunks, without
    /// asking for each line: for a text with millions of lines that is what the view needs to size its horizontal scroll.
    pub fn longest_line_bytes(&self) -> usize {
        let (mut longest, mut current) = (0usize, 0usize);
        for chunk in self.rope.chunks() {
            let mut rest = chunk.as_bytes();
            while let Some(at) = rest.iter().position(|b| *b == b'\n') {
                current += at;
                longest = longest.max(current);
                current = 0;
                rest = rest.get(at + 1..).unwrap_or_default();
            }
            current += rest.len();
        }
        longest.max(current)
    }

    /// The number of lines, at least 1: an empty text and a text ending in a line break each have a last, empty line to type in.
    pub fn line_count(&self) -> usize {
        let counted = self.rope.line_len();
        if self.is_empty() || self.rope.byte(self.len() - 1) == b'\n' { counted + 1 } else { counted }
    }

    /// The line `offset` is on (an offset past the end counts as the end, and one inside a character is where the character starts).
    pub fn line_of(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.len());
        while offset > 0 && !self.rope.is_char_boundary(offset) {
            offset -= 1;
        }
        self.rope.line_of_byte(offset)
    }

    /// The byte offset of the start of `line`, or `None` past the last line.
    pub fn line_start(&self, line: usize) -> Option<usize> {
        if line >= self.line_count() {
            return None;
        }
        Some(if line >= self.rope.line_len() { self.len() } else { self.rope.byte_of_line(line) })
    }

    /// The byte range of `line` without its line break (`\n` or `\r\n`).
    pub fn line_range(&self, line: usize) -> Option<Range<usize>> {
        let start = self.line_start(line)?;
        if line >= self.rope.line_len() {
            return Some(start..start);
        }
        let content = self.rope.line(line);
        Some(start..start + content.byte_len())
    }

    /// The byte range of `line` including its line break.
    pub fn line_range_with_break(&self, line: usize) -> Option<Range<usize>> {
        let start = self.line_start(line)?;
        let end = self.line_start(line + 1).unwrap_or(self.len());
        Some(start..end)
    }

    /// The text of `line` without its line break.
    pub fn line_text(&self, line: usize) -> Option<String> {
        let range = self.line_range(line)?;
        Some(self.rope.byte_slice(range).to_string())
    }

    /// The line and character column of `offset` (clamped into the text, and to the start of a character).
    pub fn position(&self, offset: usize) -> Position {
        let mut offset = offset.min(self.len());
        while !self.rope.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = self.rope.line_of_byte(offset);
        let start = self.line_start(line).unwrap_or(0);
        // The line break is not part of the line: an offset on the `\n` of a `\r\n` is at the end of the content, not past the `\r`.
        let content_end = self.line_range(line).map_or(offset, |r| r.end);
        Position { line, col: self.rope.byte_slice(start..offset.min(content_end).max(start)).chars().count() }
    }

    /// The offset of `pos`. A line past the end is the end of the text; a column past the end of the line is the end of the line
    /// (before its line break).
    pub fn offset_of(&self, pos: Position) -> usize {
        let Some(range) = self.line_range(pos.line) else { return self.len() };
        let mut offset = range.start;
        for (i, c) in self.rope.byte_slice(range.clone()).chars().enumerate() {
            if i == pos.col {
                break;
            }
            offset += c.len_utf8();
        }
        offset.min(range.end)
    }

    /// Apply an edit; returns the edit that undoes it.
    pub fn apply(&mut self, edit: &Edit) -> Result<Edit, TextError> {
        self.check_range(&edit.range)?;
        let old = self.rope.byte_slice(edit.range.clone()).to_string();
        self.rope.replace(edit.range.clone(), &edit.text);
        Ok(Edit { range: edit.range.start..edit.range.start + edit.text.len(), text: old })
    }

    /// Apply edits one after another (each to the result of the one before); returns their inverses in the order that undoes them
    /// (the last edit's first). If one fails, the ones before it are undone and the error is returned: the buffer is unchanged.
    pub fn apply_all(&mut self, edits: &[Edit]) -> Result<Vec<Edit>, TextError> {
        let mut inverses: Vec<Edit> = Vec::with_capacity(edits.len());
        for edit in edits {
            match self.apply(edit) {
                Ok(inverse) => inverses.push(inverse),
                Err(e) => {
                    for inverse in inverses.iter().rev() {
                        // Undoing what was just applied cannot fail: its range is the one that was just written.
                        let _ = self.apply(inverse);
                    }
                    return Err(e);
                }
            }
        }
        inverses.reverse();
        Ok(inverses)
    }

    /// Whether the text equals `other`, without building a string.
    pub fn eq_str(&self, other: &str) -> bool {
        self.len() == other.len() && self.rope.byte_slice(..) == other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(s: &str) -> Buffer {
        Buffer::from_text(s)
    }

    #[test]
    fn the_line_of_an_offset_inside_a_character_is_the_line_of_the_character() {
        // `a`, a line break, then `é` (two bytes at 2..4) and `日` (three at 4..7), then another line.
        let buffer = Buffer::from_text("a\né日\nz");
        assert_eq!((buffer.line_of(1), buffer.line_of(2), buffer.line_of(3), buffer.line_of(5), buffer.line_of(6)), (0, 1, 1, 1, 1));
        assert_eq!((buffer.line_of(7), buffer.line_of(8), buffer.line_of(9), buffer.line_of(999)), (1, 2, 2, 2));
    }

    #[test]
    fn the_longest_line_is_found_across_the_chunks_of_the_rope() {
        assert_eq!(Buffer::from_text("").longest_line_bytes(), 0);
        assert_eq!(Buffer::from_text("abc").longest_line_bytes(), 3);
        assert_eq!(Buffer::from_text("ab\nabcd\n\nx").longest_line_bytes(), 4);
        assert_eq!(Buffer::from_text("a\r\nbb\r\n").longest_line_bytes(), 3, "the CR is before the break, so it is counted");
        assert_eq!(Buffer::from_text("é日\nx").longest_line_bytes(), 5, "bytes, not characters");
        // Lines that cross chunk boundaries (a rope has many) are counted whole.
        let long = "x".repeat(100_000);
        let text = format!("{long}\nshort\n{}\n{long}y", "z".repeat(150_000));
        assert_eq!(Buffer::from_text(&text).longest_line_bytes(), 150_000);
        let one = format!("{long}{long}");
        assert_eq!(Buffer::from_text(&one).longest_line_bytes(), 200_000);
    }

    #[test]
    fn an_empty_text_has_one_empty_line() {
        let buf = b("");
        assert_eq!((buf.len(), buf.line_count()), (0, 1));
        assert_eq!(buf.line_range(0), Some(0..0));
        assert_eq!(buf.line_range(1), None);
        assert_eq!(buf.line_text(0).as_deref(), Some(""));
        assert_eq!(buf.position(0), Position { line: 0, col: 0 });
        assert_eq!(buf.offset_of(Position { line: 5, col: 5 }), 0);
    }

    #[test]
    fn lines_split_at_lf_and_a_cr_before_it_belongs_to_the_break() {
        let buf = b("ab\r\ncd\nef");
        assert_eq!(buf.line_count(), 3);
        assert_eq!(buf.line_range(0), Some(0..2));
        assert_eq!(buf.line_range_with_break(0), Some(0..4));
        assert_eq!(buf.line_text(1).as_deref(), Some("cd"));
        assert_eq!(buf.line_range(2), Some(7..9));
        assert_eq!(buf.line_start(3), None);
    }

    #[test]
    fn a_trailing_line_break_leaves_an_empty_last_line() {
        let buf = b("a\n");
        assert_eq!(buf.line_count(), 2);
        assert_eq!(buf.line_range(1), Some(2..2));
        assert_eq!(buf.line_of(2), 1);
        let crlf = b("a\r\n");
        assert_eq!(crlf.line_count(), 2);
        assert_eq!(crlf.line_range(0), Some(0..1));
        assert_eq!(crlf.line_range(1), Some(3..3));
    }

    #[test]
    fn positions_count_characters_not_bytes() {
        let buf = b("aé\n日本x");
        assert_eq!(buf.position(0), Position { line: 0, col: 0 });
        assert_eq!(buf.position(3), Position { line: 0, col: 2 });
        assert_eq!(buf.position(4), Position { line: 1, col: 0 });
        assert_eq!(buf.position(10), Position { line: 1, col: 2 });
        assert_eq!(buf.offset_of(Position { line: 1, col: 2 }), 10);
        // An offset inside a character is taken as the start of that character.
        assert_eq!(buf.position(2), Position { line: 0, col: 1 });
        // A column past the end of the line stops at the line's end, before its break.
        assert_eq!(buf.offset_of(Position { line: 0, col: 99 }), 3);
        assert_eq!(buf.offset_of(Position { line: 9, col: 0 }), buf.len());
    }

    #[test]
    fn edits_are_checked_and_never_panic() {
        let mut buf = b("aé");
        assert_eq!(buf.apply(&Edit::insert(9, "x")), Err(TextError::OutOfRange { offset: 9, len: 3 }));
        assert_eq!(buf.apply(&Edit::insert(2, "x")), Err(TextError::NotCharBoundary { offset: 2 }));
        // Built from variables on purpose: the range is reversed, which is the case under test.
        let (start, end) = (2, 1);
        assert_eq!(buf.apply(&Edit::delete(start..end)), Err(TextError::ReversedRange { start: 2, end: 1 }));
        assert_eq!(buf.apply(&Edit::delete(0..9)), Err(TextError::OutOfRange { offset: 9, len: 3 }));
        assert!(buf.eq_str("aé"), "a refused edit changes nothing");
        assert!(buf.slice(1..2).is_err() && buf.slice(0..1).is_ok());
    }

    #[test]
    fn an_edit_returns_the_edit_that_undoes_it() {
        let mut buf = b("hello world");
        let undo = buf.apply(&Edit::replace(0..5, "goodbye")).expect("valid");
        assert!(buf.eq_str("goodbye world"));
        assert_eq!(undo, Edit::replace(0..7, "hello"));
        buf.apply(&undo).expect("valid");
        assert!(buf.eq_str("hello world"));
        assert_eq!(Edit::replace(0..5, "goodbye").delta(), 2);
        assert_eq!(Edit::delete(2..6).delta(), -4);
    }

    #[test]
    fn apply_all_rolls_back_when_one_edit_fails() {
        let mut buf = b("abc");
        let r = buf.apply_all(&[Edit::insert(0, "xx"), Edit::insert(1, "yy"), Edit::insert(99, "z")]);
        assert!(r.is_err());
        assert!(buf.eq_str("abc"), "the buffer is unchanged: {}", buf.text());
        let undo = buf.apply_all(&[Edit::insert(0, "xx"), Edit::delete(1..4)]).expect("valid");
        assert!(buf.eq_str("xc"), "{}", buf.text());
        for e in &undo {
            buf.apply(e).expect("valid");
        }
        assert!(buf.eq_str("abc"));
    }

    #[test]
    fn character_stepping_respects_boundaries() {
        let buf = b("a€𝄞");
        assert_eq!(buf.next_char(0), 1);
        assert_eq!(buf.next_char(1), 4);
        assert_eq!(buf.next_char(4), 8);
        assert_eq!(buf.next_char(8), 8);
        assert_eq!(buf.prev_char(8), 4);
        assert_eq!(buf.prev_char(4), 1);
        assert_eq!(buf.prev_char(0), 0);
        assert_eq!(buf.prev_char(3), 1, "from inside a character, to its start");
        assert_eq!(buf.char_at(1), Some('€'));
        assert_eq!(buf.char_at(2), None);
        assert_eq!(buf.char_at(99), None);
        assert_eq!((buf.byte(0), buf.byte(99)), (Some(b'a'), None));
    }

    #[test]
    fn stepping_from_inside_a_character_goes_to_the_next_boundary() {
        let buf = b("a€𝄞");
        assert_eq!(buf.next_char(0), 1);
        assert_eq!(buf.next_char(1), 4);
        assert_eq!(buf.next_char(2), 4, "inside the euro sign: its end, not the end of the text");
        assert_eq!(buf.next_char(3), 4);
        assert_eq!(buf.next_char(5), 8, "inside the clef");
        assert_eq!(buf.next_char(8), 8);
        assert_eq!(buf.next_char(99), 8);
    }

    #[test]
    fn an_offset_on_the_newline_of_a_crlf_is_at_the_end_of_the_line() {
        let buf = b("a\r\nb");
        assert_eq!(buf.position(1), Position { line: 0, col: 1 });
        assert_eq!(buf.position(2), Position { line: 0, col: 1 }, "between the CR and the LF: still the end of line 0");
        assert_eq!(buf.position(3), Position { line: 1, col: 0 });
        assert_eq!(buf.offset_of(buf.position(2)), 1);
    }

    #[test]
    fn a_clone_is_a_snapshot() {
        let mut buf = b("one");
        let snapshot = buf.clone();
        buf.apply(&Edit::insert(3, " two")).expect("valid");
        assert!(snapshot.eq_str("one") && buf.eq_str("one two"));
    }

    #[test]
    fn chunks_concatenate_to_the_text() {
        let text = "x".repeat(100_000) + "é" + &"y".repeat(100_000);
        let buf = b(&text);
        assert_eq!(buf.chunks().collect::<String>(), text);
        assert_eq!(buf.text(), text);
    }
}
