//! Where the caret is in a block-style YAML document: a name or a value, and the path of keys and list items above it.
//!
//! YAML has no brackets to follow, so the place is read from the lines: a line is a list of levels from left to right (a `-` of a list,
//! and a `key:`), each at its column, and what is above the caret and further left is its parent. Flow collections (`[a, b]`, `{a: 1}`),
//! multi-line scalars and anchors are not followed: there is no completion inside them.

use std::ops::Range;

use xmlcraft_json_syntax::Segment;

/// The most lines looked through above and below the caret.
const MAX_LINES: usize = 5_000;

/// What the caret is at.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct YamlContext {
    /// A name is expected (else a value).
    pub key: bool,
    /// The mapping (for a name) or the place (for a value).
    pub path: Vec<Segment>,
    pub present: Vec<String>,
    pub replace: Range<usize>,
    pub partial: String,
    pub colon_follows: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Level {
    Dash(usize),
    Key(usize, String),
}

struct Line {
    /// Where the line starts in the text.
    start: usize,
    levels: Vec<Level>,
    /// Blank, a comment, or a document marker that ends what is above.
    blank: bool,
    marker: bool,
}

fn unquote(key: &str) -> String {
    let t = key.trim();
    let quoted = t.len() >= 2 && ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')));
    if quoted { t.get(1..t.len() - 1).unwrap_or_default().to_owned() } else { t.to_owned() }
}

/// The end of the key that starts `rest` (at the colon that ends it), when it is one.
fn key_end(rest: &str) -> Option<usize> {
    if rest.starts_with(['[', '{', '|', '>', '&', '*', '!', '%', '@', '`', '#', '?']) {
        return None;
    }
    let mut quote: Option<char> = None;
    for (i, c) in rest.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') if i == 0 => quote = Some(c),
            (None, ':') if rest.get(i + 1..).is_none_or(|r| r.is_empty() || r.starts_with([' ', '\t'])) => return Some(i),
            (None, '#') if i > 0 && rest.get(..i).is_some_and(|b| b.ends_with(' ')) => return None,
            _ => {}
        }
    }
    None
}

fn parse_line(raw: &str, start: usize) -> Line {
    let trimmed_end = raw.trim_end_matches(['\r', '\n']);
    let blank = |marker: bool| Line { start, levels: Vec::new(), blank: true, marker };
    if trimmed_end.starts_with("---") || trimmed_end.starts_with("...") {
        return blank(true);
    }
    let mut pos = 0;
    let mut levels = Vec::new();
    let bytes = trimmed_end.as_bytes();
    loop {
        while bytes.get(pos) == Some(&b' ') {
            pos += 1;
        }
        let rest = trimmed_end.get(pos..).unwrap_or_default();
        if rest == "-" || rest.starts_with("- ") {
            levels.push(Level::Dash(pos));
            pos += 1;
        } else {
            break;
        }
    }
    let rest = trimmed_end.get(pos..).unwrap_or_default();
    if rest.is_empty() || rest.starts_with('#') {
        return if levels.is_empty() { blank(false) } else { Line { start, levels, blank: false, marker: false } };
    }
    if let Some(end) = key_end(rest) {
        levels.push(Level::Key(pos, unquote(rest.get(..end).unwrap_or_default())));
    }
    Line { start, levels, blank: false, marker: false }
}

fn lines_of(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut start = 0;
    for raw in text.split_inclusive('\n') {
        if out.len() >= MAX_LINES * 2 {
            break;
        }
        out.push(parse_line(raw, start));
        start += raw.len();
    }
    if text.is_empty() || text.ends_with('\n') {
        out.push(parse_line("", start));
    }
    out
}

fn min_column(line: &Line) -> Option<usize> {
    line.levels
        .iter()
        .map(|l| match l {
            Level::Dash(c) | Level::Key(c, _) => *c,
        })
        .min()
}

/// How many items of the list whose dashes are at column `column` come before line `index`.
fn items_before(lines: &[Line], index: usize, column: usize) -> usize {
    let mut count = 0;
    for line in lines.get(..index).unwrap_or_default().iter().rev().take(MAX_LINES) {
        if line.marker {
            break;
        }
        if line.blank {
            continue;
        }
        if min_column(line).is_some_and(|c| c < column) {
            break;
        }
        if line.levels.contains(&Level::Dash(column)) {
            count += 1;
        }
    }
    count
}

/// The path to the mapping whose keys are at `limit`, seen from the lines before `end` (the caret's line included).
fn path_above(lines: &[Line], end: usize, mut limit: usize) -> Vec<Segment> {
    let mut reversed: Vec<Segment> = Vec::new();
    for index in (0..end.min(lines.len())).rev().take(MAX_LINES) {
        let Some(line) = lines.get(index) else { continue };
        if line.marker {
            break;
        }
        if line.blank || limit == 0 {
            continue;
        }
        for level in line.levels.iter().rev() {
            match level {
                Level::Key(c, name) if *c < limit => {
                    reversed.push(Segment::Key(name.clone()));
                    limit = *c;
                }
                Level::Dash(c) if *c < limit => {
                    reversed.push(Segment::Index(items_before(lines, index, *c)));
                    limit = *c;
                }
                _ => {}
            }
        }
    }
    reversed.reverse();
    reversed
}

/// The names of the mapping whose keys are at `column`, around line `index` (that line's own name is left out).
fn siblings(lines: &[Line], index: usize, column: usize) -> Vec<String> {
    let mut names = Vec::new();
    // Upwards the first key of a list item is the item's own line (its dash is at the left of it): that line is the last one. Downwards
    // a dash further left is the next item, and ends the mapping before it.
    let mut walk = |range: &mut dyn Iterator<Item = &Line>, upward: bool| {
        for line in range.take(MAX_LINES) {
            if line.marker {
                break;
            }
            if line.blank {
                continue;
            }
            let key_left = line.levels.iter().any(|l| matches!(l, Level::Key(c, _) if *c < column));
            let dash_left = line.levels.iter().any(|l| matches!(l, Level::Dash(c) if *c < column));
            if key_left || (dash_left && !upward) {
                break;
            }
            if let Some(Level::Key(_, name)) = line.levels.iter().find(|l| matches!(l, Level::Key(c, _) if *c == column)) {
                names.push(name.clone());
            }
            if dash_left {
                break;
            }
        }
    };
    // (A line that starts a list item is the first of its mapping: nothing above it belongs to it.)
    let starts_item = lines.get(index).is_some_and(|l| l.levels.iter().any(|v| matches!(v, Level::Dash(c) if *c < column)));
    if !starts_item {
        walk(&mut lines.get(..index).unwrap_or_default().iter().rev(), true);
    }
    walk(&mut lines.get(index + 1..).unwrap_or_default().iter(), false);
    names
}

/// The end of the value on a line from `start`: before the trailing white space and a comment.
fn value_end(line: &str, start: usize) -> usize {
    let rest = line.get(start..).unwrap_or_default();
    let mut quote: Option<char> = None;
    let mut end = 0;
    for (i, c) in rest.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') if i == 0 => quote = Some(c),
            (None, '#') if i > 0 && rest.get(..i).is_some_and(|b| b.ends_with(' ')) => break,
            _ => {}
        }
        if !c.is_whitespace() || quote.is_some() {
            end = i + c.len_utf8();
        }
    }
    start + end
}

/// Say what is at `offset` of a YAML `text`.
pub(crate) fn context(text: &str, offset: usize) -> Option<YamlContext> {
    let line_start = text.get(..offset)?.rfind('\n').map_or(0, |n| n + 1);
    let line_end = text.get(offset..)?.find(['\n', '\r']).map_or(text.len(), |n| offset + n);
    let line = text.get(line_start..line_end)?;
    let column = offset - line_start;
    let lines = lines_of(text);
    let index = lines.iter().position(|l| l.start == line_start)?;
    if lines.get(index)?.marker || line.contains('\t') && line.get(..column)?.trim_start_matches([' ', '\t']).is_empty() {
        return None;
    }
    // After the dashes and spaces at the start of the line.
    let mut content = 0;
    loop {
        let rest = line.get(content..)?;
        let spaces = rest.len() - rest.trim_start_matches(' ').len();
        let after = rest.get(spaces..)?;
        if after == "-" || after.starts_with("- ") {
            content += spaces + 1;
        } else {
            content += spaces;
            break;
        }
    }
    if column < content {
        // In the indentation or on a dash itself.
        return None;
    }
    let before = line.get(content..column)?;
    let body = line.get(content..)?;
    if before.starts_with('#') || before.contains(" #") {
        return None;
    }
    // The caret follows a list dash with nothing after it: a new item.
    let key_column = content;
    match key_end(body) {
        // The caret is after the colon of `key:`: a value.
        Some(colon) if before.len() > colon => {
            let name = unquote(body.get(..colon)?);
            let value_from = {
                let after = body.get(colon + 1..)?;
                content + colon + 1 + (after.len() - after.trim_start_matches(' ').len())
            };
            if column < value_from {
                return None;
            }
            let typed = line.get(value_from..column)?;
            if typed.starts_with(['[', '{', '|', '>', '&', '*', '!']) {
                return None;
            }
            let end = value_end(line, value_from).max(column);
            let mut path = path_above(&lines, index + 1, key_column);
            path.push(Segment::Key(name));
            Some(YamlContext {
                key: false,
                path,
                present: Vec::new(),
                replace: line_start + value_from..line_start + end,
                partial: typed.trim_start_matches(['"', '\'']).to_owned(),
                colon_follows: false,
            })
        }
        // The caret is in a name (or where one starts).
        found => {
            if before.starts_with(['[', '{', '|', '>', '&', '*', '!', '%', '@', '`']) || before.contains(": ") {
                return None;
            }
            let end_in_body = found.unwrap_or_else(|| value_end(body, 0));
            let end_in_body = end_in_body.max(before.len());
            let path = path_above(&lines, index + 1, key_column);
            let present = siblings(&lines, index, key_column);
            Some(YamlContext {
                key: true,
                path,
                present,
                replace: line_start + content..line_start + content + end_in_body,
                partial: before.trim_start_matches(['"', '\'']).to_owned(),
                colon_follows: found.is_some(),
            })
        }
    }
}

fn plain_safe(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'))
        && !s.starts_with(['-', '.'])
        && s.parse::<f64>().is_err()
        && !matches!(s.to_lowercase().as_str(), "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "y" | "n" | "~")
}

/// A name as YAML writes it: plain when it can be, else double-quoted (which is JSON's string).
fn name_text(name: &str) -> String {
    if plain_safe(name) {
        return name.to_owned();
    }
    let mut out = String::from("\"");
    for c in name.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A JSON value as YAML text: a string that is safe plain stays plain, anything else is JSON, which YAML reads as it is.
pub(crate) fn value_text(json: &str) -> String {
    match json.strip_prefix('"').and_then(|j| j.strip_suffix('"')) {
        Some(inner) if !inner.contains('\\') && plain_safe(inner) => inner.to_owned(),
        _ => json.to_owned(),
    }
}

/// `name: ` with the default value of the property when it has one, and where the caret goes.
pub(crate) fn property_text(name: &str, _types: &[String], default: Option<&str>, with_value: bool) -> (String, usize) {
    let mut text = name_text(name);
    if with_value {
        text.push_str(": ");
        if let Some(default) = default {
            text.push_str(&value_text(default));
        }
    }
    let cursor = text.len();
    (text, cursor)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn at(marked: &str) -> (String, usize) {
        let offset = marked.find('|').expect("a caret");
        (marked.replacen('|', "", 1), offset)
    }

    fn ctx(marked: &str) -> Option<YamlContext> {
        let (text, offset) = at(marked);
        context(&text, offset)
    }

    fn key(name: &str) -> Segment {
        Segment::Key(name.to_owned())
    }

    #[test]
    fn a_name_at_the_top_and_in_nested_mappings_has_the_keys_above_it_as_its_path_and_its_siblings_are_present() {
        let c = ctx("a: 1\nb: 2\n|").expect("context");
        assert_eq!((c.key, c.path, c.present), (true, vec![], vec!["b".to_owned(), "a".to_owned()]));
        let c = ctx("home:\n  city: x\n  z|\nother: 1\n").expect("context");
        assert_eq!((c.key, c.path, c.present, c.partial.as_str()), (true, vec![key("home")], vec!["city".to_owned()], "z"));
        let c = ctx("a:\n  b:\n    c: 1\n    |\n").expect("context");
        assert_eq!(c.path, vec![key("a"), key("b")]);
    }

    #[test]
    fn a_value_after_a_colon_has_the_key_in_its_path_and_the_value_is_replaced_whole() {
        let c = ctx("kind: a|b # note\n").expect("context");
        assert_eq!((c.key, c.path, c.partial.as_str()), (false, vec![key("kind")], "a"));
        assert_eq!(&"kind: ab # note\n"[c.replace.clone()], "ab");
        let c = ctx("a:\n  b: |\n").expect("context");
        assert_eq!((c.path, c.replace), (vec![key("a"), key("b")], 8..8));
        let c = ctx("t: \"x|y\"\n").expect("context");
        assert_eq!((c.partial.as_str(), &"t: \"xy\"\n"[c.replace.clone()]), ("x", "\"xy\""));
    }

    #[test]
    fn list_items_are_counted_and_a_new_item_is_a_mapping_of_the_list() {
        let c = ctx("items:\n  - name: a\n    age: 1\n  - name: b\n    |\n").expect("context");
        assert_eq!((c.key, c.path, c.present), (true, vec![key("items"), Segment::Index(1)], vec!["name".to_owned()]));
        let c = ctx("items:\n  - name: a\n  - |\n").expect("context");
        assert_eq!((c.key, c.path), (true, vec![key("items"), Segment::Index(1)]));
        let c = ctx("items:\n  - name: a\n    age: |\n").expect("context");
        assert_eq!(c.path, vec![key("items"), Segment::Index(0), key("age")]);
        let c = ctx("a:\n  b:\n    - x: 1\n    - y: |\n").expect("context");
        assert_eq!(c.path, vec![key("a"), key("b"), Segment::Index(1), key("y")]);
    }

    #[test]
    fn a_document_marker_ends_what_is_above_and_comments_and_flow_collections_have_no_completion() {
        let c = ctx("a:\n  b: 1\n---\n|\n").expect("context");
        assert_eq!((c.path, c.present), (vec![], vec![]));
        assert_eq!(ctx("# note |\n"), None);
        assert_eq!(ctx("a: 1 # no|te\n"), None);
        assert_eq!(ctx("a: [1, |]\n"), None);
        assert_eq!(ctx("a: {b: |}\n"), None);
        assert_eq!(ctx("a:\n|  b: 1\n"), None, "in the indentation");
        assert_eq!(ctx("---|\n"), None);
    }

    #[test]
    fn a_name_that_has_its_colon_is_replaced_without_it() {
        let c = ctx("na|me: 1\n").expect("context");
        assert_eq!((c.colon_follows, &"name: 1\n"[c.replace.clone()]), (true, "name"));
    }

    #[test]
    fn names_and_values_are_written_as_yaml() {
        assert_eq!(property_text("name", &[], None, true), ("name: ".to_owned(), 6));
        assert_eq!(property_text("a b: c", &[], Some("\"x y\""), true).0, "\"a b: c\": \"x y\"");
        assert_eq!(property_text("true", &[], None, false).0, "\"true\"");
        assert_eq!(value_text("\"abc\""), "abc");
        assert_eq!(value_text("\"yes\""), "\"yes\"");
        assert_eq!(value_text("\"12\""), "\"12\"");
        assert_eq!(value_text("3"), "3");
    }
}
