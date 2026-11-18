//! Colouring an XQuery a line at a time, for the editor: keywords, strings, comments, variables, numbers and the tags of direct
//! constructors. A line starts in the mode the line before it ended in (inside a comment, a string or a tag), so a text is coloured
//! line by line without reading it whole. It is a look at the words, not the parser: `div` is coloured as a keyword even where it
//! names an element.

use std::ops::Range;

use xmlcraft_syntax::HighlightClass;

/// Where a line starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LineMode {
    #[default]
    Normal,
    /// In a comment, this many `(:` deep.
    Comment(u16),
    /// In a string closed by this quote.
    String(char),
    /// In the start tag of a direct constructor (after its name).
    Tag,
    /// In an attribute value of a start tag, closed by this quote.
    TagValue(char),
}

const KEYWORDS: &[&str] = &[
    "allowing",
    "ancestor",
    "ancestor-or-self",
    "and",
    "array",
    "as",
    "ascending",
    "at",
    "attribute",
    "base-uri",
    "boundary-space",
    "by",
    "case",
    "cast",
    "castable",
    "catch",
    "child",
    "collation",
    "comment",
    "construction",
    "context",
    "copy-namespaces",
    "count",
    "decimal-format",
    "declare",
    "default",
    "descendant",
    "descendant-or-self",
    "descending",
    "div",
    "document",
    "document-node",
    "element",
    "else",
    "empty",
    "empty-sequence",
    "encoding",
    "end",
    "eq",
    "every",
    "except",
    "external",
    "following",
    "following-sibling",
    "for",
    "function",
    "ge",
    "greatest",
    "group",
    "gt",
    "idiv",
    "if",
    "import",
    "in",
    "inherit",
    "instance",
    "intersect",
    "is",
    "item",
    "lax",
    "le",
    "least",
    "let",
    "lt",
    "map",
    "mod",
    "module",
    "namespace",
    "namespace-node",
    "ne",
    "next",
    "no-inherit",
    "no-preserve",
    "node",
    "of",
    "only",
    "option",
    "or",
    "order",
    "ordered",
    "ordering",
    "parent",
    "preceding",
    "preceding-sibling",
    "preserve",
    "previous",
    "processing-instruction",
    "return",
    "satisfies",
    "schema",
    "schema-attribute",
    "schema-element",
    "self",
    "sliding",
    "some",
    "stable",
    "start",
    "strict",
    "strip",
    "switch",
    "text",
    "then",
    "to",
    "treat",
    "try",
    "tumbling",
    "type",
    "typeswitch",
    "union",
    "unordered",
    "validate",
    "variable",
    "version",
    "when",
    "where",
    "window",
    "xquery",
];

fn name_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':')
}

/// The coloured parts of `line` (byte ranges of it) and the mode the next line starts in.
pub fn highlight_line(line: &str, mode: LineMode) -> (Vec<(Range<usize>, HighlightClass)>, LineMode) {
    let mut out: Vec<(Range<usize>, HighlightClass)> = Vec::new();
    let mut mode = mode;
    let mut i = 0usize;
    // Where a string that opened on this line began (its quote is read before the string mode is).
    let mut opened: Option<usize> = None;
    let len = line.len();
    let at = |i: usize| line.get(i..).and_then(|s| s.chars().next());
    let starts = |i: usize, s: &str| line.get(i..).is_some_and(|r| r.starts_with(s));
    while i < len {
        let Some(c) = at(i) else { break };
        match mode {
            LineMode::Comment(depth) => {
                let start = i;
                let mut d = depth;
                while i < len {
                    if starts(i, "(:") {
                        d = d.saturating_add(1);
                        i += 2;
                    } else if starts(i, ":)") {
                        i += 2;
                        d = d.saturating_sub(1);
                        if d == 0 {
                            break;
                        }
                    } else {
                        i += at(i).map_or(1, char::len_utf8);
                    }
                }
                out.push((start..i, HighlightClass::Comment));
                mode = if d == 0 { LineMode::Normal } else { LineMode::Comment(d) };
            }
            LineMode::String(q) | LineMode::TagValue(q) => {
                let start = opened.take().unwrap_or(i);
                let mut closed = false;
                while i < len {
                    let ch = at(i).unwrap_or(q);
                    i += ch.len_utf8();
                    if ch == q {
                        if at(i) == Some(q) {
                            i += q.len_utf8();
                        } else {
                            closed = true;
                            break;
                        }
                    }
                }
                out.push((start..i, HighlightClass::AttributeValue));
                mode = match (closed, mode) {
                    (true, LineMode::TagValue(_)) => LineMode::Tag,
                    (true, _) => LineMode::Normal,
                    (false, m) => m,
                };
            }
            LineMode::Tag => {
                if c.is_whitespace() {
                    i += c.len_utf8();
                } else if starts(i, "/>") {
                    out.push((i..i + 2, HighlightClass::Punctuation));
                    i += 2;
                    mode = LineMode::Normal;
                } else if c == '>' {
                    out.push((i..i + 1, HighlightClass::Punctuation));
                    i += 1;
                    mode = LineMode::Normal;
                } else if c == '"' || c == '\'' {
                    opened = Some(i);
                    i += 1;
                    mode = LineMode::TagValue(c);
                } else if c == '=' {
                    out.push((i..i + 1, HighlightClass::Punctuation));
                    i += 1;
                } else if name_start(c) {
                    let start = i;
                    while let Some(n) = at(i).filter(|n| name_char(*n)) {
                        i += n.len_utf8();
                    }
                    out.push((start..i, HighlightClass::AttributeName));
                } else {
                    i += c.len_utf8();
                }
            }
            LineMode::Normal => {
                if c.is_whitespace() {
                    i += c.len_utf8();
                } else if starts(i, "(:") {
                    mode = LineMode::Comment(0);
                } else if c == '"' || c == '\'' {
                    opened = Some(i);
                    i += 1;
                    mode = LineMode::String(c);
                } else if c == '$' {
                    let start = i;
                    i += 1;
                    while let Some(n) = at(i).filter(|n| name_char(*n)) {
                        i += n.len_utf8();
                    }
                    out.push((start..i, HighlightClass::AttributeName));
                } else if c.is_ascii_digit() {
                    let start = i;
                    while let Some(n) = at(i).filter(|n| n.is_ascii_alphanumeric() || *n == '.') {
                        i += n.len_utf8();
                    }
                    out.push((start..i, HighlightClass::Literal));
                } else if c == '<' && at(i + 1).is_some_and(|n| name_start(n) || n == '/') {
                    // a tag of a direct constructor: `<name`, `</name>`
                    let start = i;
                    i += 1;
                    let closing = at(i) == Some('/');
                    if closing {
                        i += 1;
                    }
                    while let Some(n) = at(i).filter(|n| name_char(*n)) {
                        i += n.len_utf8();
                    }
                    if closing && at(i) == Some('>') {
                        i += 1;
                        out.push((start..i, HighlightClass::ElementName));
                    } else {
                        out.push((start..i, HighlightClass::ElementName));
                        mode = LineMode::Tag;
                    }
                } else if starts(i, "<!--") || starts(i, "<?") {
                    let close = if starts(i, "<!--") { "-->" } else { "?>" };
                    let start = i;
                    let end = line.get(i..).and_then(|r| r.find(close)).map_or(len, |e| i + e + close.len());
                    out.push((start..end, HighlightClass::Comment));
                    i = end;
                } else if name_start(c) {
                    let start = i;
                    while let Some(n) = at(i).filter(|n| name_char(*n)) {
                        i += n.len_utf8();
                    }
                    let word = line.get(start..i).unwrap_or("");
                    if KEYWORDS.binary_search(&word).is_ok() {
                        out.push((start..i, HighlightClass::Declaration));
                    }
                } else if matches!(c, '{' | '}' | '(' | ')' | '[' | ']' | ',' | ';') {
                    out.push((i..i + 1, HighlightClass::Punctuation));
                    i += 1;
                } else {
                    i += c.len_utf8();
                }
            }
        }
    }
    // A quote at the very end of the line opens a string the loop did not get to.
    if let Some(start) = opened {
        out.push((start..len, HighlightClass::AttributeValue));
    }
    // (A string's end of line is kept: XQuery strings span lines.)
    (out, mode)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keyword_list_is_sorted_for_the_search() {
        let mut sorted = KEYWORDS.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, KEYWORDS);
    }

    #[test]
    fn words_strings_comments_variables_and_tags_are_coloured() {
        let line = "for $x in (1, 'a') (: c :) return <b n=\"1\">{$x}</b>";
        let (spans, mode) = highlight_line(line, LineMode::Normal);
        assert_eq!(mode, LineMode::Normal);
        let class_of = |text: &str| spans.iter().find(|(r, _)| &line[r.clone()] == text).map(|(_, c)| *c);
        assert_eq!(class_of("for"), Some(HighlightClass::Declaration));
        assert_eq!(class_of("$x"), Some(HighlightClass::AttributeName));
        assert_eq!(class_of("'a'"), Some(HighlightClass::AttributeValue));
        assert_eq!(class_of("(: c :)"), Some(HighlightClass::Comment));
        assert_eq!(class_of("<b"), Some(HighlightClass::ElementName));
        assert_eq!(class_of("</b>"), Some(HighlightClass::ElementName));
        assert_eq!(class_of("1"), Some(HighlightClass::Literal));
    }

    #[test]
    fn comments_and_strings_carry_over_to_the_next_line() {
        assert_eq!(highlight_line("1 (: open (: nested :)", LineMode::Normal).1, LineMode::Comment(1));
        assert_eq!(highlight_line("still :) 2", LineMode::Comment(1)).1, LineMode::Normal);
        assert_eq!(highlight_line("'a string", LineMode::Normal).1, LineMode::String('\''));
        assert_eq!(highlight_line("<a b='1'", LineMode::Normal).1, LineMode::Tag);
    }
}
