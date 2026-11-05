//! Colouring a line of JSON: the class of each part of the line, and the state to carry to the next line.

use std::ops::Range;

/// What a part of a line is, for colouring.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JsonClass {
    Punctuation,
    /// The name of an object member (a string that a `:` follows on the line, and a bare name).
    Key,
    String,
    Number,
    /// `true`, `false`, `null` (and JSON5's `Infinity` and `NaN`).
    Keyword,
    Comment,
    /// A string that is not closed on its line, or a stray character.
    Error,
}

/// Where a line starts: in ordinary text, or inside a block comment that an earlier line opened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    #[default]
    Normal,
    BlockComment,
}

/// A part of a line: a byte range of the line and its class.
pub type Span = (Range<usize>, JsonClass);

/// Classify one line (its break may be included). Returns the spans, adjacent ones of the same class joined and plain text left out, and
/// the mode at the end of the line. Total: any text gives some answer.
pub fn highlight_line(line: &str, start: Mode) -> (Vec<Span>, Mode) {
    let mut spans: Vec<Span> = Vec::new();
    let mut mode = start;
    let mut i = 0;
    let push = |spans: &mut Vec<Span>, range: Range<usize>, class: JsonClass| {
        if range.is_empty() {
            return;
        }
        match spans.last_mut() {
            Some((last, c)) if *c == class && last.end == range.start && class != JsonClass::Punctuation => last.end = range.end,
            _ => spans.push((range, class)),
        }
    };
    while i < line.len() {
        let rest = line.get(i..).unwrap_or_default();
        if mode == Mode::BlockComment {
            match rest.find("*/") {
                Some(at) => {
                    push(&mut spans, i..i + at + 2, JsonClass::Comment);
                    i += at + 2;
                    mode = Mode::Normal;
                }
                None => {
                    push(&mut spans, i..line.len(), JsonClass::Comment);
                    i = line.len();
                }
            }
            continue;
        }
        let Some(c) = rest.chars().next() else { break };
        match c {
            c if c.is_whitespace() => i += c.len_utf8(),
            '{' | '}' | '[' | ']' | ',' => {
                push(&mut spans, i..i + 1, JsonClass::Punctuation);
                i += 1;
            }
            ':' => {
                push(&mut spans, i..i + 1, JsonClass::Punctuation);
                i += 1;
            }
            '/' if rest.starts_with("//") => {
                let end = rest.find(['\n', '\r']).map_or(line.len(), |at| i + at);
                push(&mut spans, i..end, JsonClass::Comment);
                i = end;
            }
            '/' if rest.starts_with("/*") => {
                mode = Mode::BlockComment;
                match rest.get(2..).and_then(|r| r.find("*/")) {
                    Some(at) => {
                        push(&mut spans, i..i + at + 4, JsonClass::Comment);
                        i += at + 4;
                        mode = Mode::Normal;
                    }
                    None => {
                        push(&mut spans, i..line.len(), JsonClass::Comment);
                        i = line.len();
                    }
                }
            }
            '"' | '\'' => {
                let (end, closed) = string_end(rest, c);
                let after = rest.get(end..).unwrap_or_default().trim_start_matches([' ', '\t']);
                let class = if !closed {
                    JsonClass::Error
                } else if after.starts_with(':') {
                    JsonClass::Key
                } else {
                    JsonClass::String
                };
                push(&mut spans, i..i + end, class);
                i += end;
            }
            _ => {
                // A run of characters up to white space, punctuation or a comment.
                let mut len = 0;
                for (at, ch) in rest.char_indices() {
                    if ch.is_whitespace()
                        || matches!(ch, '{' | '}' | '[' | ']' | ',' | ':' | '"' | '\'')
                        || (at > 0 && (rest.get(at..).is_some_and(|r| r.starts_with("//") || r.starts_with("/*"))))
                    {
                        break;
                    }
                    len = at + ch.len_utf8();
                }
                let len = len.max(c.len_utf8());
                let word = rest.get(..len).unwrap_or_default();
                let after = rest.get(len..).unwrap_or_default().trim_start_matches([' ', '\t']);
                let class = match word {
                    "true" | "false" | "null" | "Infinity" | "-Infinity" | "+Infinity" | "NaN" => JsonClass::Keyword,
                    w if after.starts_with(':') && w.chars().next().is_some_and(|f| f.is_alphabetic() || f == '_' || f == '$') => JsonClass::Key,
                    w if w.chars().next().is_some_and(|f| f.is_ascii_digit() || matches!(f, '-' | '+' | '.')) => JsonClass::Number,
                    _ => JsonClass::Error,
                };
                push(&mut spans, i..i + len, class);
                i += len;
            }
        }
    }
    (spans, mode)
}

/// Where a string that starts at the beginning of `rest` ends (after its closing quote, or at the end of the line), and whether it is
/// closed.
fn string_end(rest: &str, quote: char) -> (usize, bool) {
    let mut chars = rest.char_indices().skip(1);
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            c if c == quote => return (i + c.len_utf8(), true),
            '\n' | '\r' => return (i, false),
            _ => {}
        }
    }
    (rest.len(), false)
}
