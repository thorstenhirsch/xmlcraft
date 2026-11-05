//! Colouring a line of YAML: the class of each part of the line, and the state to carry to the next line.
//!
//! This is a reading of the shapes YAML is written in (keys, dashes, quoted scalars, block scalars, comments, anchors, tags), line by
//! line; the parser is the authority on what is valid, and its problems are underlined by the editor on top of this.

use std::ops::Range;

pub use xmlcraft_json_syntax::JsonClass;

/// A part of a line: a byte range of the line and its class.
pub type Span = (Range<usize>, JsonClass);

/// Where a line starts: in ordinary text, in the lines of a block scalar (`|` or `>`) whose content is indented more than `parent`
/// columns, or in a quoted scalar that went over the end of the previous line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    #[default]
    Normal,
    BlockScalar {
        parent: usize,
    },
    DoubleQuoted,
    SingleQuoted,
}

/// Classify one line (its break may be included). Returns the spans, plain text left out, and the mode at the end of the line. Total: any
/// text gives some answer.
pub fn highlight_line(line: &str, start: Mode) -> (Vec<Span>, Mode) {
    let body = line.trim_end_matches(['\n', '\r']);
    let mut spans: Vec<Span> = Vec::new();
    let mut push = |range: Range<usize>, class: JsonClass| {
        if !range.is_empty() {
            spans.push((range, class));
        }
    };
    let indent = body.len() - body.trim_start_matches([' ', '\t']).len();
    let mut at = indent;
    match start {
        Mode::BlockScalar { parent } => {
            if body.trim().is_empty() || indent > parent {
                push(indent..body.len(), JsonClass::String);
                return (spans, start);
            }
        }
        Mode::DoubleQuoted | Mode::SingleQuoted => {
            let quote = if start == Mode::DoubleQuoted { '"' } else { '\'' };
            let (end, closed) = quoted_end(body, 0, quote);
            push(0..end, JsonClass::String);
            if !closed {
                return (spans, start);
            }
            at = end;
        }
        Mode::Normal => {}
    }
    let mode = block_line(body, at, indent, &mut push);
    (spans, mode)
}

/// Where a quoted scalar ends in `line`, looking from `from` (which is inside it): after the closing quote, or at the end of the line.
fn quoted_end(line: &str, from: usize, quote: char) -> (usize, bool) {
    let mut chars = line.get(from..).unwrap_or_default().char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if quote == '"' && c == '\\' {
            chars.next();
        } else if c == quote {
            if quote == '\'' && chars.peek().is_some_and(|(_, n)| *n == '\'') {
                chars.next();
            } else {
                return (from + i + c.len_utf8(), true);
            }
        }
    }
    (line.len(), false)
}

fn is_break(c: Option<char>) -> bool {
    c.is_none_or(|c| c == ' ' || c == '\t')
}

/// The rest of a line from `at`, in block context.
fn block_line(line: &str, mut at: usize, indent: usize, push: &mut impl FnMut(Range<usize>, JsonClass)) -> Mode {
    let rest_of = |from: usize| line.get(from..).unwrap_or_default();
    // A document marker.
    if at == 0 && (line.starts_with("---") || line.starts_with("...")) && is_break(line.get(3..).and_then(|r| r.chars().next())) {
        push(0..3, JsonClass::Punctuation);
        at = 3;
    }
    // The column of the node this line begins (the dash of an entry, or its key): the content of a block scalar is indented more.
    let mut node = indent;
    let mut mode = Mode::Normal;
    // How many flow collections (`[`, `{`) are open on this line: a plain scalar in one ends at `,` `]` `}`.
    let mut flow = 0usize;
    loop {
        // Skip white space.
        while let Some(c) = rest_of(at).chars().next() {
            if c == ' ' || c == '\t' {
                at += 1;
            } else {
                break;
            }
        }
        let rest = rest_of(at);
        let Some(first) = rest.chars().next() else { break };
        let after = rest.chars().nth(1);
        match first {
            '#' if at == 0 || line.get(..at).is_some_and(|p| p.ends_with([' ', '\t'])) => {
                push(at..line.len(), JsonClass::Comment);
                break;
            }
            '-' | '?' if is_break(after) => {
                push(at..at + 1, JsonClass::Punctuation);
                node = at;
                at += 1;
            }
            ':' if is_break(after) || flow > 0 => {
                push(at..at + 1, JsonClass::Punctuation);
                at += 1;
            }
            '[' | ']' | '{' | '}' | ',' => {
                match first {
                    '[' | '{' => flow += 1,
                    ']' | '}' => flow = flow.saturating_sub(1),
                    _ => {}
                }
                push(at..at + 1, JsonClass::Punctuation);
                at += 1;
            }
            '&' | '*' | '!' => {
                let len = rest.find([' ', '\t', ',', ']', '}']).unwrap_or(rest.len());
                push(at..at + len.max(1), JsonClass::Keyword);
                at += len.max(1);
            }
            '|' | '>' => {
                // A block scalar header: the indicator and its modifiers; the lines that follow are its content.
                let len = rest.find([' ', '\t']).unwrap_or(rest.len());
                push(at..at + len, JsonClass::Punctuation);
                at += len;
                mode = Mode::BlockScalar { parent: node };
            }
            '"' | '\'' => {
                let start_col = at;
                let (end, closed) = quoted_end(line, at + 1, first);
                let after = rest_of(end).trim_start_matches([' ', '\t']);
                // (In a flow collection a quoted key may be followed at once by its colon: `{"a":1}`.)
                let is_key = closed && (is_key_colon(after) || (flow > 0 && after.starts_with(':')));
                push(
                    at..end,
                    if !closed {
                        JsonClass::String
                    } else if is_key {
                        JsonClass::Key
                    } else {
                        JsonClass::String
                    },
                );
                at = end;
                if !closed {
                    mode = if first == '"' { Mode::DoubleQuoted } else { Mode::SingleQuoted };
                    break;
                }
                if is_key && flow == 0 {
                    node = start_col;
                }
            }
            _ => {
                let (end, is_key) = plain_scalar(rest, flow > 0);
                // (Only YAML's own white space ends a scalar's text: `trim_end` would take a Unicode space such as U+0085 off as well,
                // and a range of one byte would then end inside it.)
                let text = rest.get(..end).unwrap_or_default().trim_end_matches([' ', '\t']);
                let class = if is_key { JsonClass::Key } else { scalar_class(text) };
                if is_key && flow == 0 {
                    node = at;
                }
                // At least the character that is there, whole.
                let first_len = first.len_utf8();
                push(at..at + text.len().max(first_len).min(rest.len()), class);
                at += end.max(first_len);
            }
        }
    }
    mode
}

/// A `:` that ends a key: followed by a space or the end of the line.
fn is_key_colon(rest: &str) -> bool {
    rest.strip_prefix(':').is_some_and(|after| is_break(after.chars().next()))
}

/// The end of a plain scalar at the start of `rest` (before ` #`, before `: ` which makes it a key, or at the end of the line), and
/// whether it is a key.
fn plain_scalar(rest: &str, in_flow: bool) -> (usize, bool) {
    let bytes = rest.as_bytes();
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        match b {
            b':' if rest.get(i + 1..).is_some_and(|a| is_break(a.chars().next())) => return (i, true),
            b'#' if i > 0 && matches!(bytes.get(i - 1), Some(b' ' | b'\t')) => return (i, false),
            b',' | b']' | b'}' if in_flow => return (i.max(1), false),
            _ => {}
        }
        i += 1;
    }
    (rest.len(), false)
}

fn scalar_class(text: &str) -> JsonClass {
    // Exactly what the reader makes of a plain scalar: null and booleans are keywords, numbers are numbers.
    match crate::parser::plain(text) {
        Some(xmlcraft_json_syntax::Data::Null | xmlcraft_json_syntax::Data::Bool(_)) => JsonClass::Keyword,
        Some(xmlcraft_json_syntax::Data::Number(_)) => JsonClass::Number,
        _ => JsonClass::String,
    }
}
