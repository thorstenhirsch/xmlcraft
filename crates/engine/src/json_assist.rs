//! Completion and hover for JSON, JSONC, JSON5, JSON Lines and YAML documents, from the JSON Schema the document names (the same
//! `$schema` member or modeline that validation reads; see [`crate::json_validation`]).
//!
//! What the caret is at is found by reading the text up to it with a scanner that follows the brackets, keys and commas, not from the
//! syntax tree: a document that is being typed (an unclosed bracket, a key with no value yet) has a tree with nothing at the caret, and
//! the scanner still knows that a name or a value is expected there, which object or array it is in and which names it already has.
//! Hover reads the whole document and takes the value or name under the pointer.

use std::ops::Range;

use xmlcraft_json::{Docs, SchemaSet};
use xmlcraft_json_syntax::{Data, Dialect, Options, Segment, Value, parse};
use xmlcraft_resolve::Resolver;

use crate::json_validation::schema_set_for;
use crate::language::{JsonFlavor, Language};

/// The most completions returned.
pub const MAX_COMPLETIONS: usize = 200;
/// Text longer than this is not scanned.
const MAX_SCANNED_BYTES: usize = 8 * 1024 * 1024;
/// Brackets nested deeper than this end the scan (no completion).
const MAX_NESTING: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    /// A name of an object member.
    Property,
    /// A value.
    Value,
}

/// One thing to offer at the caret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    /// What the list shows: the name, or the value as JSON text.
    pub label: String,
    /// The text that replaces `replace`.
    pub insert: String,
    /// Where the caret goes in `insert`, in bytes from its start.
    pub cursor: usize,
    /// The part of the document that `insert` replaces: what has been typed of the name or value.
    pub replace: Range<usize>,
    pub kind: CompletionKind,
    /// The types of a name, or why a value is offered (`enum`, `default`, …).
    pub detail: String,
    pub documentation: Option<String>,
    pub required: bool,
    pub deprecated: bool,
}

/// What the schema says about the name or value under the pointer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hover {
    /// The name or value the words are about.
    pub range: Range<usize>,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Key,
    Value,
}

/// What is at the caret.
#[derive(Debug, PartialEq, Eq)]
struct Context {
    role: Role,
    /// The object (for a name) or the place (for a value) in the document.
    path: Vec<Segment>,
    /// The names the object already has.
    present: Vec<String>,
    replace: Range<usize>,
    /// What has been typed of the name or value.
    partial: String,
    /// The text after the replaced part starts with a colon: a name is replaced, its value stays.
    colon_follows: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// A name (object) or a value (array) is expected.
    Key,
    Colon,
    Value,
    Comma,
}

struct Frame {
    object: bool,
    state: State,
    keys: Vec<String>,
    key: Option<String>,
    index: usize,
}

impl Frame {
    fn new(object: bool) -> Frame {
        Frame { object, state: if object { State::Key } else { State::Value }, keys: Vec::new(), key: None, index: 0 }
    }

    /// The step from this container to the value it is reading.
    fn segment(&self) -> Option<Segment> {
        if self.object { self.key.clone().map(Segment::Key) } else { Some(Segment::Index(self.index)) }
    }
}

fn path_of(frames: &[Frame]) -> Option<Vec<Segment>> {
    frames.iter().map(Frame::segment).collect()
}

/// The end of the quoted string that starts at `start` and whether it is closed: it ends at its quote or, unclosed, at the end of the
/// line (a break is allowed after a backslash).
fn string_end(text: &str, start: usize, quote: char) -> (usize, bool) {
    let rest = text.get(start..).unwrap_or_default();
    let mut chars = rest.char_indices().skip(1);
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            c if c == quote => return (start + i + c.len_utf8(), true),
            '\n' | '\r' => return (start + i, false),
            _ => {}
        }
    }
    (text.len(), false)
}

fn unescape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

fn is_punctuation(c: char) -> bool {
    matches!(c, '{' | '}' | '[' | ']' | ':' | ',' | '"' | '\'')
}

fn word_end(text: &str, start: usize, comments: bool) -> usize {
    let rest = text.get(start..).unwrap_or_default();
    for (i, c) in rest.char_indices() {
        let comment = comments && i > 0 && c == '/' && rest.get(i..).is_some_and(|r| r.starts_with("//") || r.starts_with("/*"));
        if c.is_whitespace() || is_punctuation(c) || comment {
            return start + i;
        }
    }
    text.len()
}

/// A token was read: a name where one is expected, else a value.
fn token_done(stack: &mut [Frame], content: String) {
    if let Some(top) = stack.last_mut() {
        match top.state {
            State::Key if top.object => {
                top.keys.push(content.clone());
                top.key = Some(content);
                top.state = State::Colon;
            }
            State::Value => top.state = State::Comma,
            _ => {}
        }
    }
}

/// Read the text from `start` to `offset` and say what is at `offset`.
fn scan(text: &str, start: usize, offset: usize, dialect: Dialect) -> Option<Context> {
    let comments = matches!(dialect, Dialect::Jsonc | Dialect::Json5);
    let single = dialect == Dialect::Json5;
    let mut stack: Vec<Frame> = Vec::new();
    let mut i = start;
    while i < offset {
        let rest = text.get(i..)?;
        let c = rest.chars().next()?;
        let width = c.len_utf8();
        match c {
            c if c.is_whitespace() => i += width,
            '/' if comments && rest.starts_with("//") => {
                let end = rest.find(['\n', '\r']).map_or(text.len(), |n| i + n);
                if offset <= end {
                    return None;
                }
                i = end;
            }
            '/' if comments && rest.starts_with("/*") => {
                let end = rest.get(2..).and_then(|r| r.find("*/")).map(|n| i + 2 + n + 2)?;
                if offset < end {
                    return None;
                }
                i = end;
            }
            '{' | '[' => {
                if let Some(top) = stack.last()
                    && top.state != State::Value
                {
                    return None;
                }
                if stack.len() >= MAX_NESTING {
                    return None;
                }
                stack.push(Frame::new(c == '{'));
                i += width;
            }
            '}' | ']' => {
                let top = stack.pop()?;
                if top.object != (c == '}') {
                    return None;
                }
                if let Some(parent) = stack.last_mut() {
                    parent.state = State::Comma;
                }
                i += width;
            }
            ',' => {
                let top = stack.last_mut()?;
                if top.state != State::Comma {
                    return None;
                }
                if top.object {
                    top.state = State::Key;
                    top.key = None;
                } else {
                    top.index = top.index.saturating_add(1);
                    top.state = State::Value;
                }
                i += width;
            }
            ':' => {
                let top = stack.last_mut()?;
                if !(top.object && top.state == State::Colon) {
                    return None;
                }
                top.state = State::Value;
                i += width;
            }
            '\'' if !single => return None,
            '"' | '\'' => {
                let (end, closed) = string_end(text, i, c);
                if i < offset && (offset < end || (!closed && offset <= end)) {
                    let typed = text.get(i + width..offset)?;
                    return in_token(&stack, text, i, if closed { end } else { offset }, unescape(typed));
                }
                let inner_end = if closed { end - width } else { end };
                token_done(&mut stack, unescape(text.get(i + width..inner_end)?));
                i = end;
            }
            _ => {
                let end = word_end(text, i, comments).max(i + width);
                if offset <= end {
                    return in_token(&stack, text, i, end, text.get(i..offset)?.to_owned());
                }
                token_done(&mut stack, text.get(i..end)?.to_owned());
                i = end;
            }
        }
    }
    // Between two tokens.
    let top = stack.last()?;
    let at = stack.len() - 1;
    let (role, path) = match top.state {
        State::Key if top.object => (Role::Key, path_of(stack.get(..at)?)?),
        State::Key => return None,
        State::Value => {
            let mut path = path_of(stack.get(..at)?)?;
            path.push(top.segment()?);
            (Role::Value, path)
        }
        State::Colon | State::Comma => return None,
    };
    let present = if role == Role::Key { top.keys.clone() } else { Vec::new() };
    let colon_follows = role == Role::Key && text.get(offset..).is_some_and(|r| r.trim_start().starts_with(':'));
    Some(Context { role, path, present, replace: offset..offset, partial: String::new(), colon_follows })
}

/// The caret is in the token `start..end` (a string or a word) that follows the frames in `stack`.
fn in_token(stack: &[Frame], text: &str, start: usize, end: usize, partial: String) -> Option<Context> {
    let top = stack.last()?;
    let at = stack.len() - 1;
    let (role, path) = match top.state {
        State::Key if top.object => (Role::Key, path_of(stack.get(..at)?)?),
        State::Value => {
            let mut path = path_of(stack.get(..at)?)?;
            path.push(top.segment()?);
            (Role::Value, path)
        }
        _ => return None,
    };
    let present = if role == Role::Key { top.keys.clone() } else { Vec::new() };
    let colon_follows = role == Role::Key && text.get(end..).is_some_and(|r| r.trim_start().starts_with(':'));
    Some(Context { role, path, present, replace: start..end, partial, colon_follows })
}

/// The text that is a name of an object member written with `name`, with a value of the kind its schema says and the caret in it.
fn property_text(name: &str, types: &[String], default: Option<&str>, with_value: bool) -> (String, usize) {
    let mut key = String::new();
    key.push('"');
    for c in name.chars() {
        match c {
            '"' => key.push_str("\\\""),
            '\\' => key.push_str("\\\\"),
            c if (c as u32) < 0x20 => key.push_str(&format!("\\u{:04x}", c as u32)),
            c => key.push(c),
        }
    }
    key.push('"');
    if !with_value {
        let cursor = key.len();
        return (key, cursor);
    }
    key.push_str(": ");
    if let Some(default) = default {
        key.push_str(default);
        let cursor = key.len();
        return (key, cursor);
    }
    let (body, inside) = match types.first().map(String::as_str) {
        Some("string") => ("\"\"", 1),
        Some("object") => ("{}", 1),
        Some("array") => ("[]", 1),
        _ => ("", 0),
    };
    let cursor = key.len() + inside;
    key.push_str(body);
    (key, cursor)
}

fn matches_prefix(label: &str, partial: &str) -> Option<bool> {
    let (label, partial) = (label.trim_matches('"').to_lowercase(), partial.trim_matches('"').to_lowercase());
    if label.starts_with(&partial) {
        Some(true)
    } else if label.contains(&partial) {
        Some(false)
    } else {
        None
    }
}

fn dialect_of(language: Language) -> Option<Dialect> {
    match language {
        Language::Json(flavor) => Some(flavor.dialect()),
        _ => None,
    }
}

/// The start of the text the caret's value is in: the beginning of its line for JSON Lines, else the beginning.
fn scan_start(text: &str, offset: usize, language: Language) -> usize {
    if language == Language::Json(JsonFlavor::Lines) { text.get(..offset).and_then(|t| t.rfind(['\n', '\r'])).map_or(0, |n| n + 1) } else { 0 }
}

/// What may be written at `offset` of `text`, from the schema the document names; nothing when it names none, it cannot be read, or the
/// caret is not at a name or value.
pub fn complete(text: &str, language: Language, offset: usize, location: &str, resolver: &Resolver) -> Vec<Completion> {
    let Some(set) = schema_set_for(text, language, location, resolver) else { return Vec::new() };
    complete_with(&set, text, language, offset)
}

/// [`complete`] with the schema already read.
pub fn complete_with(set: &SchemaSet, text: &str, language: Language, offset: usize) -> Vec<Completion> {
    if text.len() > MAX_SCANNED_BYTES || !text.is_char_boundary(offset) {
        return Vec::new();
    }
    let context = match language {
        Language::Yaml => crate::yaml_assist::context(text, offset).map(|c| Context {
            role: if c.key { Role::Key } else { Role::Value },
            path: c.path,
            present: c.present,
            replace: c.replace,
            partial: c.partial,
            colon_follows: c.colon_follows,
        }),
        _ => dialect_of(language).and_then(|d| scan(text, scan_start(text, offset, language), offset, d)),
    };
    let Some(context) = context else { return Vec::new() };
    let yaml = language == Language::Yaml;
    let mut ranked: Vec<(bool, Completion)> = Vec::new();
    match context.role {
        Role::Key => {
            for hint in set.properties_at(&context.path, &context.present) {
                let Some(prefix) = matches_prefix(&hint.name, &context.partial) else { continue };
                let (insert, cursor) = if yaml {
                    crate::yaml_assist::property_text(&hint.name, &hint.types, hint.default.as_deref(), !context.colon_follows)
                } else {
                    property_text(&hint.name, &hint.types, hint.default.as_deref(), !context.colon_follows)
                };
                ranked.push((
                    prefix,
                    Completion {
                        label: hint.name.clone(),
                        insert,
                        cursor,
                        replace: context.replace.clone(),
                        kind: CompletionKind::Property,
                        detail: hint.types.join(" | "),
                        documentation: hint.description,
                        required: hint.required,
                        deprecated: hint.deprecated,
                    },
                ));
            }
        }
        Role::Value => {
            for value in set.values_at(&context.path) {
                let shown = if yaml { crate::yaml_assist::value_text(&value.text) } else { value.text.clone() };
                let Some(prefix) = matches_prefix(&shown, &context.partial) else { continue };
                ranked.push((
                    prefix,
                    Completion {
                        cursor: shown.len(),
                        label: shown.clone(),
                        insert: shown,
                        replace: context.replace.clone(),
                        kind: CompletionKind::Value,
                        detail: value.source.to_owned(),
                        documentation: None,
                        required: false,
                        deprecated: false,
                    },
                ));
            }
        }
    }
    // Those that start with what was typed come before those that only contain it; the schema's order is kept within each.
    ranked.sort_by_key(|(starts, _)| !*starts);
    ranked.into_iter().map(|(_, c)| c).take(MAX_COMPLETIONS).collect()
}

/// What the schema says about the name or value at `offset`.
pub fn hover(text: &str, language: Language, offset: usize, location: &str, resolver: &Resolver) -> Option<Hover> {
    let set = schema_set_for(text, language, location, resolver)?;
    hover_with(&set, text, language, offset)
}

/// [`hover`] with the schema already read.
pub fn hover_with(set: &SchemaSet, text: &str, language: Language, offset: usize) -> Option<Hover> {
    if text.len() > MAX_SCANNED_BYTES || !text.is_char_boundary(offset) {
        return None;
    }
    let roots: Vec<Value> = match language {
        Language::Json(JsonFlavor::Lines) => {
            let line = scan_start(text, offset, language);
            let end = text.get(line..).and_then(|t| t.find(['\n', '\r'])).map_or(text.len(), |n| line + n);
            let read = parse(text.get(line..end)?, &Options { dialect: Dialect::Json, ..Options::default() });
            // (The values are placed from the start of the line: shift them to the text.)
            read.root.map(|v| shifted(v, line)).into_iter().collect()
        }
        Language::Json(flavor) => parse(text, &Options { dialect: flavor.dialect(), ..Options::default() }).root.into_iter().collect(),
        Language::Yaml => xmlcraft_yaml_syntax::parse(text, &xmlcraft_yaml_syntax::Options::default()).documents,
        Language::Xml | Language::Bson | Language::Html => return None,
    };
    let root = roots.iter().find(|r| r.span.start <= offset && offset <= r.span.end)?;
    let path = root.path_at(offset)?;
    // The pointer is over a name when it is in the key of the last member, else over the value.
    let range = range_of(root, &path, offset)?;
    let docs = set.describe_at(&path)?;
    Some(Hover { range, text: describe(&docs) })
}

/// The name or the value at the end of `path`, whichever `offset` is in.
fn range_of(root: &Value, path: &[Segment], offset: usize) -> Option<Range<usize>> {
    let mut current = root;
    let mut key_span: Option<Range<usize>> = None;
    for segment in path {
        match (&current.data, segment) {
            (Data::Object(members), Segment::Key(k)) => {
                let m = members.iter().find(|m| &m.key == k && m.key_span.start <= offset && offset <= m.value.span.end.max(m.key_span.end))?;
                key_span = Some(m.key_span.clone());
                current = &m.value;
            }
            (Data::Array(items), Segment::Index(n)) => {
                key_span = None;
                current = items.get(*n)?;
            }
            _ => return None,
        }
    }
    match key_span {
        Some(k) if offset <= k.end => Some(k),
        _ => Some(current.span.clone()),
    }
}

fn shifted(mut value: Value, by: usize) -> Value {
    fn walk(v: &mut Value, by: usize) {
        v.span = v.span.start.saturating_add(by)..v.span.end.saturating_add(by);
        match &mut v.data {
            Data::Array(items) => items.iter_mut().for_each(|i| walk(i, by)),
            Data::Object(members) => {
                for m in members {
                    m.key_span = m.key_span.start.saturating_add(by)..m.key_span.end.saturating_add(by);
                    walk(&mut m.value, by);
                }
            }
            _ => {}
        }
    }
    walk(&mut value, by);
    value
}

/// The words of a hover: title, description and what the schema says of the value.
fn describe(docs: &Docs) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(title) = &docs.title {
        parts.push(title.clone());
    }
    if let Some(description) = &docs.description {
        parts.push(description.clone());
    }
    let mut facts: Vec<String> = Vec::new();
    if !docs.types.is_empty() {
        facts.push(format!("Type: {}", docs.types.join(" | ")));
    }
    if !docs.values.is_empty() {
        let shown: Vec<&str> = docs.values.iter().take(12).map(String::as_str).collect();
        let more = if docs.values.len() > 12 { ", …" } else { "" };
        facts.push(format!("Values: {}{more}", shown.join(", ")));
    }
    if let Some(default) = &docs.default {
        facts.push(format!("Default: {default}"));
    }
    if docs.deprecated {
        facts.push("Deprecated".to_owned());
    }
    if !facts.is_empty() {
        parts.push(facts.join("\n"));
    }
    parts.join("\n\n")
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn at(text: &str) -> (String, usize) {
        let offset = text.find('|').expect("a caret");
        (text.replacen('|', "", 1), offset)
    }

    fn context(marked: &str, dialect: Dialect) -> Option<Context> {
        let (text, offset) = at(marked);
        scan(&text, 0, offset, dialect)
    }

    fn key(name: &str) -> Segment {
        Segment::Key(name.to_owned())
    }

    #[test]
    fn a_name_is_expected_after_a_brace_and_after_a_comma_and_the_names_present_are_known() {
        let c = context(r#"{|"#, Dialect::Json).expect("context");
        assert_eq!((c.role, c.path, c.present, c.replace), (Role::Key, vec![], vec![], 1..1));
        let c = context(r#"{"a": 1, "b": [1, 2], |"#, Dialect::Json).expect("context");
        assert_eq!((c.role, c.present), (Role::Key, vec!["a".to_owned(), "b".to_owned()]));
        let c = context(r#"{"a": {"x": 1, |}}"#, Dialect::Json).expect("context");
        assert_eq!((c.role, c.path, c.present), (Role::Key, vec![key("a")], vec!["x".to_owned()]));
    }

    #[test]
    fn a_value_is_expected_after_a_colon_and_in_an_array() {
        let c = context(r#"{"a": |"#, Dialect::Json).expect("context");
        assert_eq!((c.role, c.path), (Role::Value, vec![key("a")]));
        let c = context(r#"{"a": [1, {"b": 2}, |"#, Dialect::Json).expect("context");
        assert_eq!((c.role, c.path), (Role::Value, vec![key("a"), Segment::Index(2)]));
        let c = context(r#"{"a": [|"#, Dialect::Json).expect("context");
        assert_eq!(c.path, vec![key("a"), Segment::Index(0)]);
    }

    #[test]
    fn a_name_or_value_being_typed_is_replaced_whole_and_what_is_typed_is_known() {
        let c = context(r#"{"na|"#, Dialect::Json).expect("context");
        assert_eq!((c.role, c.replace, c.partial.as_str()), (Role::Key, 1..4, "na"));
        let c = context(r#"{"na|me": 1}"#, Dialect::Json).expect("context");
        assert_eq!((c.replace, c.partial.as_str(), c.colon_follows), (1..7, "na", true));
        let c = context(r#"{"a": tr|"#, Dialect::Json).expect("context");
        assert_eq!((c.role, c.replace, c.partial.as_str()), (Role::Value, 6..8, "tr"));
        let c = context(r#"{"a": "x|y"}"#, Dialect::Json).expect("context");
        assert_eq!((c.role, c.replace, c.partial.as_str()), (Role::Value, 6..10, "x"));
    }

    #[test]
    fn nothing_is_offered_in_comments_between_a_name_and_its_colon_or_outside_a_document() {
        assert_eq!(context("{\"a\": 1 // |\n}", Dialect::Jsonc), None);
        assert_eq!(context("{\"a\": 1, /* | */ }", Dialect::Jsonc).map(|c| c.role), None);
        assert_eq!(context(r#"{"a" |"#, Dialect::Json), None);
        assert_eq!(context(r#"{"a": 1 |"#, Dialect::Json), None);
        assert_eq!(context("|", Dialect::Json), None);
        assert_eq!(context(r#"{"a": 1}|"#, Dialect::Json), None);
        assert_eq!(context(r#"{"a": ]|"#, Dialect::Json), None);
    }

    #[test]
    fn comments_are_skipped_and_single_quotes_and_bare_names_belong_to_json5() {
        let c = context("{ // note, with a comma\n /* {[ */ \"a\": 1, |", Dialect::Jsonc).expect("context");
        assert_eq!((c.role, c.present), (Role::Key, vec!["a".to_owned()]));
        let c = context("{a: 1, 'b': 2, c|", Dialect::Json5).expect("context");
        assert_eq!((c.role, c.present, c.partial.as_str()), (Role::Key, vec!["a".to_owned(), "b".to_owned()], "c"));
        assert_eq!(context("{'a': |", Dialect::Json).map(|c| c.role), None, "single quotes are not JSON");
    }

    #[test]
    fn deeply_nested_brackets_end_the_scan() {
        let deep = format!("{}|", "[".repeat(MAX_NESTING + 10));
        assert_eq!(context(&deep, Dialect::Json), None);
    }

    #[test]
    fn a_property_is_written_with_a_value_of_its_type_and_the_caret_in_it() {
        let t = |types: &[&str], default: Option<&str>, with_value| {
            let types: Vec<String> = types.iter().map(|t| (*t).to_owned()).collect();
            property_text("a\"b", &types, default, with_value)
        };
        assert_eq!(t(&["string"], None, true), ("\"a\\\"b\": \"\"".to_owned(), 9));
        assert_eq!(t(&["object"], None, true), ("\"a\\\"b\": {}".to_owned(), 9));
        assert_eq!(t(&["integer"], None, true), ("\"a\\\"b\": ".to_owned(), 8));
        assert_eq!(t(&["string"], Some("\"x\""), true).1, 11);
        assert_eq!(t(&["string"], None, false), ("\"a\\\"b\"".to_owned(), 6));
    }

    const SCHEMA: &str = r##"{
      "$defs": {"address": {"type": "object", "description": "Where.", "properties": {"city": {"type": "string"}, "zip": {"type": "string", "default": "00000"}}, "required": ["city"]}},
      "type": "object",
      "properties": {
        "name": {"type": "string", "title": "Name", "description": "The full name."},
        "age": {"type": "integer", "deprecated": true},
        "home": {"$ref": "#/$defs/address"},
        "kind": {"enum": ["alpha", "beta", 3]},
        "active": {"type": "boolean"},
        "list": {"type": "array", "items": {"$ref": "#/$defs/address"}}
      },
      "required": ["name", "kind"]
    }"##;

    fn schema() -> SchemaSet {
        let root = parse(SCHEMA, &Options::default()).root.expect("schema");
        SchemaSet::new(root, "", xmlcraft_json::Draft::D2020_12, &xmlcraft_json::NoLoader)
    }

    fn offer(language: Language, marked: &str) -> Vec<Completion> {
        let (text, offset) = at(marked);
        complete_with(&schema(), &text, language, offset)
    }

    fn labels(items: &[Completion]) -> Vec<&str> {
        items.iter().map(|c| c.label.as_str()).collect()
    }

    const JSON: Language = Language::Json(JsonFlavor::Json);

    #[test]
    fn names_are_offered_required_first_without_those_present_and_replace_what_is_typed() {
        let items = offer(JSON, r#"{"name": "x", |}"#);
        assert_eq!(labels(&items), ["kind", "age", "home", "active", "list"]);
        assert!(items[0].required && !items[1].required && items[1].deprecated);
        assert_eq!((items[0].insert.as_str(), items[0].cursor, items[0].replace.clone()), ("\"kind\": ", 8, 14..14));
        let items = offer(JSON, r#"{"a|"}"#);
        assert_eq!(labels(&items), ["age", "active", "name", "kind", "list"].iter().copied().filter(|n| n.contains('a')).collect::<Vec<_>>());
        assert_eq!(items[0].replace, 1..4);
        let items = offer(JSON, r#"{"hom|e": 1}"#);
        assert_eq!((items[0].insert.as_str(), items[0].replace.clone()), ("\"home\"", 1..7), "the colon and value stay");
    }

    #[test]
    fn nested_names_follow_references_and_values_come_from_enums_booleans_and_defaults() {
        assert_eq!(labels(&offer(JSON, r#"{"home": {|}}"#)), ["city", "zip"]);
        let zip = offer(JSON, r#"{"home": {"zip": |}}"#);
        assert_eq!(labels(&zip), ["\"00000\""]);
        assert_eq!(zip[0].detail, "default");
        assert_eq!(labels(&offer(JSON, r#"{"kind": |}"#)), ["\"alpha\"", "\"beta\"", "3"]);
        assert_eq!(labels(&offer(JSON, r#"{"kind": "b|"}"#)), ["\"beta\""], "typing filters, and the whole string is replaced");
        assert_eq!(labels(&offer(JSON, r#"{"active": t|}"#)), ["true"]);
        assert_eq!(labels(&offer(JSON, r#"{"list": [{"city": "a"}, {|}]}"#)), ["city", "zip"]);
    }

    #[test]
    fn json_lines_are_read_one_line_at_a_time_and_jsonc_has_comments() {
        let items = offer(Language::Json(JsonFlavor::Lines), "{\"name\": \"a\"}\n{|}\n");
        assert_eq!(labels(&items)[0], "name", "the second line has no name yet: {:?}", labels(&items));
        let items = offer(Language::Json(JsonFlavor::Jsonc), "{\n  // a note, with a comma\n  \"name\": \"x\",\n  |\n}");
        assert_eq!(labels(&items)[0], "kind");
    }

    #[test]
    fn yaml_offers_names_and_values_by_indentation() {
        let items = offer(Language::Yaml, "name: x\nhome:\n  |\n");
        assert_eq!(labels(&items), ["city", "zip"]);
        assert_eq!(items[0].insert, "city: ");
        assert_eq!(labels(&offer(Language::Yaml, "kind: |\n")), ["alpha", "beta", "3"]);
        assert_eq!(labels(&offer(Language::Yaml, "list:\n  - city: a\n  - |\n")), ["city", "zip"]);
        // (A string that reads as a number stays quoted: plain it would be an integer.)
        assert_eq!(labels(&offer(Language::Yaml, "home:\n  zip: |\n")), ["\"00000\""]);
    }

    #[test]
    fn hover_says_what_the_schema_says_of_a_name_or_a_value() {
        let (text, offset) = at(r#"{"na|me": "x", "kind": "beta", "home": {"city": "c"}}"#);
        let h = hover_with(&schema(), &text, JSON, offset).expect("hover");
        assert_eq!(&text[h.range.clone()], "\"name\"");
        assert_eq!(h.text, "Name\n\nThe full name.\n\nType: string");
        let (text, offset) = at(r#"{"kind": "be|ta"}"#);
        let h = hover_with(&schema(), &text, JSON, offset).expect("hover");
        assert_eq!(&text[h.range.clone()], "\"beta\"");
        assert!(h.text.contains("Values: \"alpha\", \"beta\", 3"), "{}", h.text);
        let (text, offset) = at(r#"{"home": {"ci|ty": "c"}}"#);
        assert!(hover_with(&schema(), &text, JSON, offset).is_some_and(|h| h.text.contains("Type: string")));
        let (text, offset) = at("home:\n  ci|ty: c\n");
        assert!(hover_with(&schema(), &text, Language::Yaml, offset).is_some());
        let (text, offset) = at(r#"{"unknown|": 1}"#);
        assert_eq!(hover_with(&schema(), &text, JSON, offset), None);
        assert_eq!(hover_with(&schema(), "<a/>", Language::Xml, 1), None);
    }

    #[test]
    fn no_caret_in_any_text_makes_it_panic() {
        let texts = [
            "{\"name\": \"x\", \"home\": {\"zip\": [1, {\"a\": tru",
            "// c\n{ \"a\": /* x */ \"b\", 'c': [",
            "name: x\nhome:\n  - a\n  -\n    b: [1, {\n",
            "{\"ä\": \"ü\", \"k\n\": \"\\\"}",
            "- - - a: 1\n    - b:\n---\n\t x: y",
            "",
        ];
        let set = schema();
        for text in texts {
            for language in [JSON, Language::Json(JsonFlavor::Json5), Language::Json(JsonFlavor::Lines), Language::Yaml] {
                for offset in 0..=text.len() + 1 {
                    let _ = complete_with(&set, text, language, offset);
                    let _ = hover_with(&set, text, language, offset);
                }
            }
        }
    }
}
