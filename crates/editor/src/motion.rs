//! Moving through text: by character cluster, by word, by line, and by display column.
//!
//! All of it is pure and total: given a valid offset it returns a valid offset, and given an invalid one it clamps. Looking at the text
//! is limited to a small window around the offset, so a one-megabyte line costs no more than a short one.

use unicode_segmentation::GraphemeCursor;
use unicode_width::UnicodeWidthChar;
use xmlcraft_text::Buffer;

/// How far around an offset the text is read to find a cluster boundary, in bytes.
const REACH: usize = 128;

fn floor_boundary(buf: &Buffer, mut offset: usize) -> usize {
    offset = offset.min(buf.len());
    while offset > 0 && !buf.is_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn ceil_boundary(buf: &Buffer, mut offset: usize) -> usize {
    offset = offset.min(buf.len());
    while offset < buf.len() && !buf.is_boundary(offset) {
        offset += 1;
    }
    offset
}

/// `offset` moved onto a character boundary inside the text.
pub fn clamp(buf: &Buffer, offset: usize) -> usize {
    floor_boundary(buf, offset)
}

/// The end of the character cluster that starts at `offset` (the text length at the end).
pub fn next_grapheme(buf: &Buffer, offset: usize) -> usize {
    let offset = clamp(buf, offset);
    if offset >= buf.len() {
        return buf.len();
    }
    let start = floor_boundary(buf, offset.saturating_sub(REACH));
    let end = ceil_boundary(buf, offset.saturating_add(REACH));
    let Ok(window) = buf.slice(start..end) else { return buf.next_char(offset) };
    let mut cursor = GraphemeCursor::new(offset - start, window.len(), true);
    match cursor.next_boundary(&window, 0) {
        Ok(Some(boundary)) => start + boundary,
        // The window is not the whole text and the cluster is longer than it, or the window has no context: take one character.
        _ => buf.next_char(offset),
    }
}

/// The start of the character cluster that ends at `offset` (0 at the start).
pub fn prev_grapheme(buf: &Buffer, offset: usize) -> usize {
    let offset = clamp(buf, offset);
    if offset == 0 {
        return 0;
    }
    let start = floor_boundary(buf, offset.saturating_sub(REACH));
    let end = ceil_boundary(buf, offset.saturating_add(REACH));
    let Ok(window) = buf.slice(start..end) else { return buf.prev_char(offset) };
    let mut cursor = GraphemeCursor::new(offset - start, window.len(), true);
    match cursor.prev_boundary(&window, 0) {
        Ok(Some(boundary)) => start + boundary,
        _ => buf.prev_char(offset),
    }
}

/// `offset` moved back to the start of the character cluster it is inside (itself, when it already is on a boundary). A click lands
/// anywhere, including between a letter and its accent; the caret must not.
pub fn snap_to_cluster(buf: &Buffer, offset: usize) -> usize {
    let offset = clamp(buf, offset);
    let next = next_grapheme(buf, offset);
    if next <= offset {
        return offset; // at the end of the text
    }
    prev_grapheme(buf, next).min(offset)
}

/// What kind of character this is, for moving and selecting by word. Letters, digits and the characters of XML names (`_ - . :`) make
/// words, so double-clicking `xs:element` selects all of it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CharClass {
    Space,
    Word,
    Other,
}

pub fn char_class(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':') {
        CharClass::Word
    } else {
        CharClass::Other
    }
}

fn class_at(buf: &Buffer, offset: usize) -> Option<CharClass> {
    buf.char_at(offset).map(char_class)
}

fn class_before(buf: &Buffer, offset: usize) -> Option<CharClass> {
    if offset == 0 { None } else { class_at(buf, buf.prev_char(offset)) }
}

/// The start of the word before `offset`: spaces are skipped, then the run of one kind of character.
pub fn word_left(buf: &Buffer, offset: usize) -> usize {
    let mut at = clamp(buf, offset);
    while class_before(buf, at) == Some(CharClass::Space) {
        at = buf.prev_char(at);
    }
    if let Some(class) = class_before(buf, at) {
        while class_before(buf, at) == Some(class) {
            at = buf.prev_char(at);
        }
    }
    at
}

/// The end of the word after `offset`: spaces are skipped, then the run of one kind of character.
pub fn word_right(buf: &Buffer, offset: usize) -> usize {
    let mut at = clamp(buf, offset);
    while class_at(buf, at) == Some(CharClass::Space) {
        at = buf.next_char(at);
    }
    if let Some(class) = class_at(buf, at) {
        while class_at(buf, at) == Some(class) {
            at = buf.next_char(at);
        }
    }
    at
}

/// The word (or run of spaces, or run of other characters) at `offset`, as a range; the one before it at the end of a line.
pub fn word_range_at(buf: &Buffer, offset: usize) -> std::ops::Range<usize> {
    let at = clamp(buf, offset);
    let line = buf.line_of(at);
    let line_end = buf.line_range(line).map_or(at, |r| r.end);
    // At the end of a line the word before the caret is the one meant.
    let anchor = if at >= line_end && at > 0 && buf.line_range(line).is_some_and(|r| r.start < at) { buf.prev_char(at) } else { at };
    let Some(class) = class_at(buf, anchor) else { return at..at };
    if matches!(buf.char_at(anchor), Some('\n' | '\r')) {
        return at..at;
    }
    let (mut start, mut end) = (anchor, buf.next_char(anchor));
    while start > 0 && class_before(buf, start) == Some(class) && !matches!(buf.char_at(buf.prev_char(start)), Some('\n' | '\r')) {
        start = buf.prev_char(start);
    }
    while class_at(buf, end) == Some(class) && !matches!(buf.char_at(end), Some('\n' | '\r')) {
        end = buf.next_char(end);
    }
    start..end
}

/// The offset of the first character on the line that is not a space or tab (the line's end for a blank line).
pub fn first_non_blank(buf: &Buffer, line: usize) -> usize {
    let Some(range) = buf.line_range(line) else { return buf.len() };
    let mut at = range.start;
    while at < range.end && matches!(buf.char_at(at), Some(' ' | '\t')) {
        at += 1;
    }
    at
}

/// Where Home goes: to the first non-blank character, or to the line start when already there.
pub fn smart_home(buf: &Buffer, offset: usize) -> usize {
    let line = buf.line_of(offset);
    let start = buf.line_start(line).unwrap_or(0);
    let first = first_non_blank(buf, line);
    if offset == first { start } else { first }
}

/// The width in display columns of `c` when it starts at `column`: tabs go to the next tab stop, wide characters take two, combining
/// marks none.
pub fn char_width(c: char, column: usize, tab_width: usize) -> usize {
    match c {
        '\t' => {
            let tab = tab_width.max(1);
            tab - column % tab
        }
        _ => UnicodeWidthChar::width(c).unwrap_or(0),
    }
}

/// The display column of `offset` on its line.
pub fn display_column(buf: &Buffer, offset: usize, tab_width: usize) -> usize {
    let offset = clamp(buf, offset);
    let line = buf.line_of(offset);
    let start = buf.line_start(line).unwrap_or(0);
    let Ok(prefix) = buf.slice(start..offset) else { return 0 };
    prefix.chars().fold(0, |column, c| column + char_width(c, column, tab_width))
}

/// The offset on `line` whose display column is the largest not past `column` (the line's end when the line is shorter), on a cluster
/// boundary.
pub fn offset_at_column(buf: &Buffer, line: usize, column: usize, tab_width: usize) -> usize {
    let Some(range) = buf.line_range(line) else { return buf.len() };
    let mut at = range.start;
    let mut col = 0;
    while at < range.end {
        let next = next_grapheme(buf, at).min(range.end);
        let width: usize = buf.slice(at..next).map(|s| s.chars().fold(col, |c, ch| c + char_width(ch, c, tab_width)) - col).unwrap_or(0);
        if col + width > column {
            // A cluster that straddles the goal belongs to the nearer side.
            return if column - col > width / 2 && width > 0 { next } else { at };
        }
        col += width;
        at = next;
    }
    range.end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(s: &str) -> Buffer {
        Buffer::from_text(s)
    }

    #[test]
    fn clusters_move_as_one() {
        // e + combining acute, a flag (two regional indicators), a family emoji with zero-width joiners, CRLF.
        let text = "e\u{301}x\u{1F1E9}\u{1F1EA}y\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}z\r\nw";
        let buf = b(text);
        let mut offsets = vec![0];
        loop {
            let next = next_grapheme(&buf, *offsets.last().expect("non-empty"));
            if next == *offsets.last().expect("non-empty") {
                break;
            }
            offsets.push(next);
        }
        let pieces: Vec<&str> = offsets.windows(2).map(|w| &text[w[0]..w[1]]).collect();
        assert_eq!(pieces, ["e\u{301}", "x", "\u{1F1E9}\u{1F1EA}", "y", "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}", "z", "\r\n", "w"]);
        // Backwards gives the same boundaries.
        let mut back = vec![buf.len()];
        while *back.last().expect("non-empty") > 0 {
            back.push(prev_grapheme(&buf, *back.last().expect("non-empty")));
        }
        back.reverse();
        assert_eq!(back, offsets);
    }

    #[test]
    fn snapping_goes_to_the_start_of_the_cluster() {
        let buf = b("ae\u{301}x\u{1F468}\u{200D}\u{1F469}");
        assert_eq!(snap_to_cluster(&buf, 0), 0);
        assert_eq!(snap_to_cluster(&buf, 1), 1);
        assert_eq!(snap_to_cluster(&buf, 2), 1, "between the letter and its accent");
        assert_eq!(snap_to_cluster(&buf, 4), 4, "already on a boundary");
        assert_eq!(snap_to_cluster(&buf, 5), 5);
        assert_eq!(snap_to_cluster(&buf, 9), 5, "inside the joined emoji");
        assert_eq!(snap_to_cluster(&buf, buf.len()), buf.len(), "the end of the text stays the end");
        assert_eq!(snap_to_cluster(&buf, 999), buf.len());
        assert_eq!(snap_to_cluster(&b(""), 0), 0);
        assert_eq!(snap_to_cluster(&b("a\r\nb"), 2), 1, "inside a CRLF");
    }

    #[test]
    fn clusters_at_the_edges_and_out_of_range() {
        let buf = b("é");
        assert_eq!(next_grapheme(&buf, 0), 2);
        assert_eq!(next_grapheme(&buf, 2), 2);
        assert_eq!(next_grapheme(&buf, 999), 2);
        assert_eq!(prev_grapheme(&buf, 0), 0);
        assert_eq!(prev_grapheme(&buf, 1), 0, "inside a character: clamped first");
        assert_eq!(next_grapheme(&b(""), 0), 0);
    }

    #[test]
    fn a_long_line_does_not_need_the_whole_line() {
        let text = format!("{}e\u{301}{}", "a".repeat(100_000), "b".repeat(100_000));
        let buf = b(&text);
        assert_eq!(next_grapheme(&buf, 100_000), 100_003);
        assert_eq!(prev_grapheme(&buf, 100_003), 100_000);
        // A cluster longer than the window falls back to single characters rather than failing.
        let long = format!("a{}", "\u{301}".repeat(500));
        let buf = b(&long);
        assert!(next_grapheme(&buf, 0) >= 1);
    }

    #[test]
    fn words() {
        let buf = b("  <xs:element name=\"a-b\"/> foo_bar  baz");
        assert_eq!(word_right(&buf, 0), 3, "spaces, then the `<` run");
        assert_eq!(word_right(&buf, 3), 13, "xs:element is one word");
        assert_eq!(word_left(&buf, 13), 3);
        assert_eq!(word_left(&buf, 3), 2, "the `<` run, after skipping nothing");
        assert_eq!(word_left(&buf, 0), 0);
        assert_eq!(word_right(&buf, buf.len()), buf.len());
        assert_eq!(&"  <xs:element name=\"a-b\"/> foo_bar  baz"[word_range_at(&buf, 6)], "xs:element");
        assert_eq!(&"  <xs:element name=\"a-b\"/> foo_bar  baz"[word_range_at(&buf, 13)], " ", "between words: the spaces");
        assert_eq!(word_range_at(&b("ab\ncd"), 2), 0..2, "at a line end: the word before");
        assert_eq!(word_range_at(&b("ab\n\ncd"), 3), 3..3, "an empty line has no word");
        assert_eq!(word_range_at(&b(""), 0), 0..0);
    }

    #[test]
    fn home_toggles_between_the_indent_and_the_line_start() {
        let buf = b("a\n    <b/>\n\t\n");
        assert_eq!(smart_home(&buf, 8), 6, "to the first non-blank");
        assert_eq!(smart_home(&buf, 6), 2, "then to the start");
        assert_eq!(smart_home(&buf, 2), 6);
        assert_eq!(smart_home(&buf, 0), 0);
        assert_eq!(first_non_blank(&buf, 2), 12, "a blank line: its end");
    }

    #[test]
    fn columns_count_tabs_wide_characters_and_marks() {
        let buf = b("\tab\t日e\u{301}x");
        assert_eq!(display_column(&buf, 0, 4), 0);
        assert_eq!(display_column(&buf, 1, 4), 4);
        assert_eq!(display_column(&buf, 3, 4), 6);
        assert_eq!(display_column(&buf, 4, 4), 8, "the tab after `ab` goes to the next stop");
        assert_eq!(display_column(&buf, 7, 4), 10, "a wide character takes two");
        assert_eq!(display_column(&buf, buf.len(), 4), 12, "a combining mark takes none");
        assert_eq!(display_column(&buf, 1, 0), 1, "a tab width of zero is treated as one");
    }

    #[test]
    fn the_offset_for_a_column() {
        let buf = b("abc\n日本語\nx");
        assert_eq!(offset_at_column(&buf, 0, 2, 4), 2);
        assert_eq!(offset_at_column(&buf, 0, 99, 4), 3, "past the end: the line end");
        assert_eq!(offset_at_column(&buf, 1, 0, 4), 4);
        assert_eq!(offset_at_column(&buf, 1, 2, 4), 7, "the second wide character starts at column 2");
        assert_eq!(offset_at_column(&buf, 1, 3, 4), 7, "inside a wide character: the nearer edge");
        assert_eq!(offset_at_column(&buf, 1, 5, 4), 10);
        assert_eq!(offset_at_column(&buf, 5, 0, 4), buf.len(), "a line that does not exist");
    }
}
