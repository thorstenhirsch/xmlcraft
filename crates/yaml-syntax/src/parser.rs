//! Reading YAML into the value tree of the JSON reader.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use saphyr_parser::{Event, Marker, Parser, ScalarStyle, Span as YamlSpan, SpannedEventReceiver, Tag};
use xmlcraft_json_syntax::{Data, Member, Value};
use xmlcraft_syntax::{Code, Diagnostic};

/// What to read and how far.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Values nested deeper than this are not read.
    pub max_depth: usize,
    /// Aliases are expanded into copies; the copies together are no more than this many values (the defence against "billion laughs").
    /// The values the text itself spells out are not counted.
    pub max_nodes: usize,
    /// More problems than this are not listed; the last one says so.
    pub max_diagnostics: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self { max_depth: 256, max_nodes: 1_000_000, max_diagnostics: 1_000 }
    }
}

/// The result of reading: one value for each document of the stream (a text with no document has none), and the problems.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parse {
    pub documents: Vec<Value>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Read `text` as a YAML stream. A text that cannot be read to the end still gives the documents and the part of the one that was
/// being read, and a problem where it stopped.
pub fn parse(text: &str, options: &Options) -> Parse {
    let offsets = Offsets::new(text);
    let mut builder = Builder::new(text, &offsets, *options);
    let mut parser = Parser::new_from_str(text);
    let result = parser.load(&mut builder, true);
    if let Err(error) = result {
        let at = offsets.byte(*error.marker());
        let end = text.get(at..).and_then(|rest| rest.chars().next()).map_or(at, |c| at + c.len_utf8());
        builder.report(Code::YamlSyntax, at..end, vec![error.info().to_owned()]);
    }
    builder.finish()
}

/// Turns the character index of a marker into a byte offset of the text (the parser counts characters). A checkpoint every
/// [`CHECKPOINT`] characters keeps a conversion short whatever the line breaks are.
struct Offsets<'a> {
    text: &'a str,
    /// The byte offset of character 0, `CHECKPOINT`, `2 * CHECKPOINT` …
    checkpoints: Vec<usize>,
}

const CHECKPOINT: usize = 1024;

impl<'a> Offsets<'a> {
    fn new(text: &'a str) -> Self {
        let checkpoints = text.char_indices().step_by(CHECKPOINT).map(|(at, _)| at).collect();
        Self { text, checkpoints }
    }

    /// The offset a marker is at, kept inside the text.
    fn byte(&self, marker: Marker) -> usize {
        let index = marker.index();
        let Some(&start) = self.checkpoints.get(index / CHECKPOINT) else { return self.text.len() };
        let rest = self.text.get(start..).unwrap_or_default();
        rest.char_indices().nth(index % CHECKPOINT).map_or(self.text.len(), |(at, _)| start + at)
    }

    fn span(&self, span: YamlSpan) -> Range<usize> {
        let (start, end) = (self.byte(span.start), self.byte(span.end));
        start..end.max(start)
    }
}

enum Frame {
    Seq {
        start: usize,
        items: Vec<Value>,
        anchor: usize,
    },
    Map {
        start: usize,
        members: Vec<Member>,
        key: Option<(String, Range<usize>)>,
        anchor: usize,
        /// The values of `<<` keys: applied when the mapping ends, so that its own keys win whatever the order.
        merges: Vec<(Range<usize>, Value)>,
    },
    /// A container nested too deeply: its events are counted and dropped.
    Skipped {
        depth: usize,
    },
}

struct Builder<'a> {
    text: &'a str,
    offsets: &'a Offsets<'a>,
    options: Options,
    stack: Vec<Frame>,
    documents: Vec<Value>,
    diagnostics: Vec<Diagnostic>,
    full: bool,
    /// Finished values that have an anchor, with the number of values in each.
    anchors: HashMap<usize, (Value, usize)>,
    /// How many values the copies made by aliases have, together.
    expanded: usize,
    /// Anchors of values that were dropped for being nested too deeply: an alias to one is dropped too, with the depth problem as the reason.
    dropped_anchors: HashSet<usize>,
    too_deep_reported: bool,
    expansion_reported: bool,
}

impl<'a> Builder<'a> {
    fn new(text: &'a str, offsets: &'a Offsets<'a>, options: Options) -> Self {
        Self {
            text,
            offsets,
            options,
            stack: Vec::new(),
            documents: Vec::new(),
            diagnostics: Vec::new(),
            full: false,
            anchors: HashMap::new(),
            expanded: 0,
            dropped_anchors: HashSet::new(),
            too_deep_reported: false,
            expansion_reported: false,
        }
    }

    fn report(&mut self, code: Code, span: Range<usize>, args: Vec<String>) {
        if self.full {
            return;
        }
        if self.diagnostics.len() >= self.options.max_diagnostics {
            self.full = true;
            self.diagnostics.push(Diagnostic::new(Code::TooManyDiagnostics, span, vec![]));
            return;
        }
        self.diagnostics.push(Diagnostic::new(code, span, args));
    }

    /// Close what is still open (the text ended in the middle of a node) and hand over the documents.
    fn finish(mut self) -> Parse {
        while let Some(frame) = self.stack.pop() {
            let end = self.text.len();
            let value = match frame {
                Frame::Seq { start, items, .. } => Value { span: start..end.max(start), data: Data::Array(items) },
                Frame::Map { start, members, merges, .. } => {
                    let members = self.merged(members, merges);
                    Value { span: start..end.max(start), data: Data::Object(members) }
                }
                Frame::Skipped { .. } => continue,
            };
            self.push_value(value, 0);
        }
        Parse { documents: self.documents, diagnostics: self.diagnostics }
    }

    /// A finished value goes into the container being built (as a key or a value of a mapping), or is a document.
    fn push_value(&mut self, value: Value, anchor: usize) {
        if anchor != 0 {
            let size = count(&value);
            self.anchors.insert(anchor, (value.clone(), size));
        }
        match self.stack.last_mut() {
            None => self.documents.push(value),
            Some(Frame::Seq { items, .. }) => items.push(value),
            Some(Frame::Map { members, key, merges, .. }) => match key.take() {
                None => {
                    // A key: a scalar's text, or the text of a node that is not one.
                    let span = value.span.clone();
                    let name = match &value.data {
                        Data::String(s) => s.clone(),
                        Data::Number(n) => n.clone(),
                        Data::Bool(b) => b.to_string(),
                        Data::Null => "null".to_owned(),
                        Data::Array(_) | Data::Object(_) => self.text.get(span.clone()).unwrap_or_default().trim().to_owned(),
                    };
                    *key = Some((name, span));
                }
                Some((name, key_span)) => {
                    if name == "<<" && key_span.len() == 2 {
                        merges.push((key_span, value));
                        return;
                    }
                    if members.iter().any(|m| m.key == name) {
                        let (span, shown) = (key_span.clone(), name.clone());
                        members.push(Member { key: name, key_span, value });
                        self.report(Code::YamlDuplicateKey, span, vec![shown]);
                    } else {
                        members.push(Member { key: name, key_span, value });
                    }
                }
            },
            Some(Frame::Skipped { .. }) => {}
        }
    }

    /// `members` with what the `<<` keys bring (the members they have that `members` does not).
    fn merged(&mut self, mut members: Vec<Member>, merges: Vec<(Range<usize>, Value)>) -> Vec<Member> {
        for (key_span, value) in merges {
            if !merge(&mut members, value) {
                self.report(Code::YamlBadMerge, key_span, vec![]);
            }
        }
        members
    }

    fn start_container(&mut self, map: bool, span: Range<usize>, anchor: usize) {
        if let Some(Frame::Skipped { depth }) = self.stack.last_mut() {
            *depth += 1;
            if anchor != 0 {
                self.dropped_anchors.insert(anchor);
            }
            return;
        }
        if self.stack.len() >= self.options.max_depth {
            if anchor != 0 {
                self.dropped_anchors.insert(anchor);
            }
            if !self.too_deep_reported {
                self.too_deep_reported = true;
                self.report(Code::YamlTooDeep, span, vec![self.options.max_depth.to_string()]);
            }
            self.stack.push(Frame::Skipped { depth: 1 });
            return;
        }
        let start = span.start;
        self.stack.push(if map {
            Frame::Map { start, members: Vec::new(), key: None, anchor, merges: Vec::new() }
        } else {
            Frame::Seq { start, items: Vec::new(), anchor }
        });
    }

    fn end_container(&mut self, span: Range<usize>) {
        if let Some(Frame::Skipped { depth }) = self.stack.last_mut() {
            *depth -= 1;
            if *depth == 0 {
                self.stack.pop();
                self.push_value(Value { span, data: Data::Null }, 0);
            }
            return;
        }
        let Some(frame) = self.stack.pop() else { return };
        let (value, anchor) = match frame {
            Frame::Seq { start, items, anchor } => (Value { span: start..span.end.max(start), data: Data::Array(items) }, anchor),
            Frame::Map { start, members, anchor, merges, .. } => {
                let members = self.merged(members, merges);
                (Value { span: start..span.end.max(start), data: Data::Object(members) }, anchor)
            }
            Frame::Skipped { .. } => return,
        };
        self.push_value(value, anchor);
    }

    fn alias(&mut self, id: usize, span: Range<usize>) {
        if matches!(self.stack.last(), Some(Frame::Skipped { .. })) {
            return;
        }
        let Some((value, size)) = self.anchors.get(&id).cloned() else {
            // The parser knows which anchors exist, so this is an anchor that was dropped for being too deep (already reported), or one it
            // gave an id we do not have.
            if !self.dropped_anchors.contains(&id) {
                let name = self.text.get(span.clone()).unwrap_or_default().to_owned();
                self.report(Code::YamlSyntax, span.clone(), vec![format!("the alias {name} refers to a value that is not available")]);
            }
            self.push_value(Value { span, data: Data::Null }, 0);
            return;
        };
        self.expanded = self.expanded.saturating_add(size);
        if self.expanded > self.options.max_nodes {
            if !self.expansion_reported {
                self.expansion_reported = true;
                self.report(Code::YamlAliasTooLarge, span.clone(), vec![self.options.max_nodes.to_string()]);
            }
            self.push_value(Value { span, data: Data::Null }, 0);
            return;
        }
        let mut copy = value;
        place_at(&mut copy, &span);
        self.push_value(copy, 0);
    }

    fn scalar(&mut self, text: &str, style: ScalarStyle, anchor: usize, tag: Option<&Tag>, span: Range<usize>) {
        if matches!(self.stack.last(), Some(Frame::Skipped { .. })) {
            if anchor != 0 {
                self.dropped_anchors.insert(anchor);
            }
            return;
        }
        let data = resolve(text, style, tag);
        self.push_value(Value { span, data }, anchor);
    }
}

impl<'input> SpannedEventReceiver<'input> for Builder<'_> {
    fn on_event(&mut self, event: Event<'input>, span: YamlSpan) {
        let range = self.offsets.span(span);
        match event {
            Event::Scalar(text, style, anchor, tag) => self.scalar(&text, style, anchor, tag.as_deref(), range),
            Event::SequenceStart(anchor, _) => self.start_container(false, range, anchor),
            Event::MappingStart(anchor, _) => self.start_container(true, range, anchor),
            Event::SequenceEnd | Event::MappingEnd => self.end_container(range),
            Event::Alias(id) => self.alias(id, range),
            _ => {}
        }
    }
}

/// The number of values in a tree.
fn count(value: &Value) -> usize {
    1 + match &value.data {
        Data::Array(items) => items.iter().map(count).sum::<usize>(),
        Data::Object(members) => members.iter().map(|m| count(&m.value)).sum::<usize>(),
        _ => 0,
    }
}

/// Every place in the copy of an anchored value is the alias: that is where it is in this text.
fn place_at(value: &mut Value, span: &Range<usize>) {
    value.span = span.clone();
    match &mut value.data {
        Data::Array(items) => items.iter_mut().for_each(|v| place_at(v, span)),
        Data::Object(members) => {
            for m in members {
                m.key_span = span.clone();
                place_at(&mut m.value, span);
            }
        }
        _ => {}
    }
}

/// `<<: value`: the members of the mapping (or of each mapping of a list) that `members` does not have are added. False when the value is
/// neither.
fn merge(members: &mut Vec<Member>, value: Value) -> bool {
    let sources: Vec<Value> = match value.data {
        Data::Object(_) => vec![value],
        Data::Array(items) if items.iter().all(|v| matches!(v.data, Data::Object(_))) => items,
        _ => return false,
    };
    let mut have: HashSet<String> = members.iter().map(|m| m.key.clone()).collect();
    for source in sources {
        if let Data::Object(list) = source.data {
            for m in list {
                if have.insert(m.key.clone()) {
                    members.push(m);
                }
            }
        }
    }
    true
}

/// What a scalar is, by the YAML 1.2 core schema (and its tag, if it has one).
fn resolve(text: &str, style: ScalarStyle, tag: Option<&Tag>) -> Data {
    if let Some(tag) = tag.filter(|t| t.handle == "!!" || t.handle == "tag:yaml.org,2002:") {
        return match tag.suffix.as_str() {
            "str" => Data::String(text.to_owned()),
            "null" => Data::Null,
            "bool" => plain(text).filter(|d| matches!(d, Data::Bool(_))).unwrap_or_else(|| Data::String(text.to_owned())),
            "int" | "float" => plain(text).filter(|d| matches!(d, Data::Number(_))).unwrap_or_else(|| Data::String(text.to_owned())),
            _ => Data::String(text.to_owned()),
        };
    }
    if tag.is_some() || style != ScalarStyle::Plain {
        return Data::String(text.to_owned());
    }
    plain(text).unwrap_or_else(|| Data::String(text.to_owned()))
}

/// A plain scalar that is null, a boolean or a number.
pub(crate) fn plain(text: &str) -> Option<Data> {
    match text {
        "" | "~" | "null" | "Null" | "NULL" => return Some(Data::Null),
        "true" | "True" | "TRUE" => return Some(Data::Bool(true)),
        "false" | "False" | "FALSE" => return Some(Data::Bool(false)),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => return Some(Data::Number("Infinity".to_owned())),
        "-.inf" | "-.Inf" | "-.INF" => return Some(Data::Number("-Infinity".to_owned())),
        ".nan" | ".NaN" | ".NAN" => return Some(Data::Number("NaN".to_owned())),
        _ => {}
    }
    if let Some(hex) = text.strip_prefix("0x").filter(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_hexdigit())) {
        return u128::from_str_radix(hex, 16).ok().map(|v| Data::Number(v.to_string()));
    }
    if let Some(octal) = text.strip_prefix("0o").filter(|o| !o.is_empty() && o.bytes().all(|b| (b'0'..=b'7').contains(&b))) {
        return u128::from_str_radix(octal, 8).ok().map(|v| Data::Number(v.to_string()));
    }
    is_number(text).then(|| Data::Number(normal_number(text)))
}

/// A number as JSON writes it: no `+`, no leading zeros of an integer (`012` is twelve; a zero before the point stays: `0.5`).
fn normal_number(text: &str) -> String {
    let (sign, body) = match text.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", text.strip_prefix('+').unwrap_or(text)),
    };
    let plain_integer = body.bytes().all(|b| b.is_ascii_digit());
    let body = if plain_integer { body.trim_start_matches('0') } else { body };
    format!("{sign}{}", if body.is_empty() { "0" } else { body })
}

/// `[-+]? ( [0-9]+ | ( \.[0-9]+ | [0-9]+ (\.[0-9]*)? ) ([eE][-+]?[0-9]+)? )`
fn is_number(text: &str) -> bool {
    let body = text.strip_prefix(['-', '+']).unwrap_or(text);
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(at) => (body.get(..at).unwrap_or_default(), body.get(at + 1..)),
        None => (body, None),
    };
    if let Some(exp) = exponent {
        let digits = exp.strip_prefix(['+', '-']).unwrap_or(exp);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
    }
    let (int, frac) = mantissa.split_once('.').map_or((mantissa, None), |(i, f)| (i, Some(f)));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    match frac {
        None => !int.is_empty() && digits(int),
        Some(f) => digits(int) && digits(f) && !(int.is_empty() && f.is_empty()),
    }
}
