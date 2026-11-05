//! XML as data and data as XML: the natural mapping.
//!
//! An element is a member named by the element; its value is
//! - a string, when it has only text (or, with [`XmlToData::guess_types`], the number, boolean or null the text is);
//! - null, when it is empty and has no attributes;
//! - an object, when it has attributes (`@name`), child elements (grouped by name in the order each name first appears, repeated names
//!   becoming arrays) or both, with `#text` for text beside them.
//!
//! Comments and processing instructions are not data and are left out; entity and character references are replaced; CDATA is text.

use std::collections::HashMap;

use xmlcraft_json_syntax::{Data, Member, Value};
use xmlcraft_syntax::{SyntaxKind, SyntaxNode, decode_references, is_name_char, is_name_start_char, parse};

use crate::{ConvertError, MAX_INDENT, error, unquote};

/// Elements nested deeper than this are not read or written (the readers of XmlCraft stop long before).
const MAX_DEPTH: usize = 256;

/// How XML becomes data.
#[derive(Clone, Debug)]
pub struct XmlToData {
    /// What an attribute's name starts with in the object.
    pub attribute_prefix: String,
    /// The member that holds the text of an element that also has attributes or children.
    pub text_key: String,
    /// Text (of an element or an attribute) that is a number, `true`, `false` or `null` becomes that (otherwise it stays a string).
    pub guess_types: bool,
    /// White space between elements is dropped, and the text of an element is trimmed when it has children.
    pub trim: bool,
}

impl Default for XmlToData {
    fn default() -> Self {
        Self { attribute_prefix: "@".to_owned(), text_key: "#text".to_owned(), guess_types: false, trim: true }
    }
}

/// How data becomes XML.
#[derive(Clone, Debug)]
pub struct DataToXml {
    pub attribute_prefix: String,
    pub text_key: String,
    /// The element that holds everything when the data is not an object with one member.
    pub root: String,
    /// The element an array item that is itself an array or the root array's items are written as.
    pub item: String,
    /// Write `<?xml version="1.0" encoding="UTF-8"?>` first.
    pub declaration: bool,
}

impl Default for DataToXml {
    fn default() -> Self {
        Self { attribute_prefix: "@".to_owned(), text_key: "#text".to_owned(), root: "root".to_owned(), item: "item".to_owned(), declaration: true }
    }
}

fn val(data: Data) -> Value {
    Value { span: 0..0, data }
}

/// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`: the numbers JSON has.
pub(crate) fn is_json_number(t: &str) -> bool {
    let body = t.strip_prefix('-').unwrap_or(t);
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(at) => (body.get(..at).unwrap_or_default(), body.get(at + 1..)),
        None => (body, None),
    };
    if let Some(e) = exponent {
        let digits = e.strip_prefix(['+', '-']).unwrap_or(e);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
    }
    let (int, frac) = mantissa.split_once('.').map_or((mantissa, None), |(i, f)| (i, Some(f)));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    digits(int) && (int == "0" || !int.starts_with('0')) && frac.is_none_or(digits)
}

fn text_value(text: &str, guess: bool) -> Value {
    if !guess {
        return val(Data::String(text.to_owned()));
    }
    match text.trim() {
        "null" => val(Data::Null),
        "true" => val(Data::Bool(true)),
        "false" => val(Data::Bool(false)),
        t if is_json_number(t) => val(Data::Number(t.to_owned())),
        _ => val(Data::String(text.to_owned())),
    }
}

/// The qualified name of an element or attribute (the first `Name` token of the node's tag).
fn name_of(node: &SyntaxNode) -> Option<String> {
    node.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name).map(|t| t.text().to_owned())
}

struct Read<'a> {
    options: &'a XmlToData,
}

impl Read<'_> {
    /// The value of the element `element` and its name.
    fn element(&self, element: &SyntaxNode, depth: usize) -> Result<(String, Value), ConvertError> {
        if depth > MAX_DEPTH {
            return Err(error(Some(usize::from(element.text_range().start())), "elements are nested too deeply"));
        }
        let start_tag = element.children().find(|n| n.kind() == SyntaxKind::StartTag);
        let name =
            start_tag.as_ref().and_then(name_of).ok_or_else(|| error(Some(usize::from(element.text_range().start())), "an element has no name"))?;
        let mut members: Vec<Member> = Vec::new();
        if let Some(tag) = &start_tag {
            for attribute in tag.children().filter(|n| n.kind() == SyntaxKind::Attribute) {
                let Some(attr_name) = name_of(&attribute) else { continue };
                let raw = attribute.children().find(|n| n.kind() == SyntaxKind::AttrValue).map(|v| v.text().to_string()).unwrap_or_default();
                let inner = unquote(&raw);
                members.push(Member {
                    key: format!("{}{attr_name}", self.options.attribute_prefix),
                    key_span: 0..0,
                    value: text_value(&decode_references(inner), self.options.guess_types),
                });
            }
        }
        let mut text = String::new();
        let mut children: Vec<(String, Value)> = Vec::new();
        for child in element.children_with_tokens() {
            match child {
                rowan::NodeOrToken::Token(t) if matches!(t.kind(), SyntaxKind::Text | SyntaxKind::CharRef | SyntaxKind::EntityRef) => {
                    text.push_str(&decode_references(t.text()));
                }
                rowan::NodeOrToken::Node(n) if n.kind() == SyntaxKind::Element => children.push(self.element(&n, depth + 1)?),
                rowan::NodeOrToken::Node(n) if n.kind() == SyntaxKind::CData => {
                    let cdata: String = n
                        .children_with_tokens()
                        .filter_map(|c| c.into_token())
                        .filter(|t| t.kind() == SyntaxKind::CdataText)
                        .map(|t| t.text().to_owned())
                        .collect();
                    text.push_str(&cdata);
                }
                _ => {}
            }
        }
        let has_structure = !members.is_empty() || !children.is_empty();
        let text = if self.options.trim && has_structure { text.trim().to_owned() } else { text };
        if !has_structure {
            let value = if text.is_empty() { val(Data::Null) } else { text_value(&text, self.options.guess_types) };
            return Ok((name, value));
        }
        // Children grouped by name, in the order each name first appears; a name that repeats is an array.
        let mut order: Vec<String> = Vec::new();
        let mut groups: HashMap<String, Vec<Value>> = HashMap::new();
        for (child_name, value) in children {
            if !groups.contains_key(&child_name) {
                order.push(child_name.clone());
            }
            groups.entry(child_name).or_default().push(value);
        }
        if !text.is_empty() {
            members.push(Member { key: self.options.text_key.clone(), key_span: 0..0, value: text_value(&text, self.options.guess_types) });
        }
        for child_name in order {
            let mut values = groups.remove(&child_name).unwrap_or_default();
            let value = if values.len() == 1 { values.remove(0) } else { val(Data::Array(values)) };
            members.push(Member { key: child_name, key_span: 0..0, value });
        }
        Ok((name, val(Data::Object(members))))
    }
}

/// The data an XML text stands for: an object with one member, the root element. A text that is not well-formed is refused with its
/// first problem.
pub fn xml_to_value(text: &str, options: &XmlToData) -> Result<Value, ConvertError> {
    let parsed = parse(text);
    if let Some(d) = parsed.diagnostics().iter().find(|d| d.severity() == xmlcraft_syntax::Severity::Error) {
        return Err(error(Some(d.span.start), format!("the XML is not well-formed: {}", d.message())));
    }
    let root = parsed.syntax();
    let element = root.children().find(|n| n.kind() == SyntaxKind::Element).ok_or_else(|| error(None, "the XML has no element"))?;
    let (name, value) = Read { options }.element(&element, 0)?;
    Ok(val(Data::Object(vec![Member { key: name, key_span: 0..0, value }])))
}

// --- data to XML -------------------------------------------------------------------------------------------------------------

/// `s` as XML text (or an attribute value); a character XML cannot hold is left out and `dropped` set.
fn escape(s: &str, attribute: bool, dropped: &mut bool) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            '\n' if attribute => out.push_str("&#10;"),
            '\t' if attribute => out.push_str("&#9;"),
            '\r' => out.push_str("&#13;"),
            c if !xmlcraft_syntax::is_xml_char(c) => *dropped = true,
            c => out.push(c),
        }
    }
    out
}

/// A name part with no colon: characters that are not name characters become `_`, and one that does not start right gets a `_` in
/// front.
fn ncname(part: &str) -> String {
    let mut out = String::with_capacity(part.len());
    for (i, c) in part.chars().enumerate() {
        let fine = c != ':' && if i == 0 { is_name_start_char(c) } else { is_name_char(c) };
        if fine {
            out.push(c);
        } else if i == 0 && c != ':' && is_name_char(c) {
            out.push('_');
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

fn scalar_text(value: &Value) -> String {
    match &value.data {
        Data::Null => String::new(),
        Data::Bool(b) => b.to_string(),
        Data::Number(n) | Data::String(n) => n.clone(),
        Data::Array(_) | Data::Object(_) => String::new(),
    }
}

struct Write<'a> {
    options: &'a DataToXml,
    indent: Option<usize>,
    warnings: &'a mut Vec<String>,
    out: String,
    /// The namespace prefixes declared by the elements around (`@xmlns:p`): a name may use only these, `xml` and `xmlns`.
    prefixes: Vec<String>,
    /// A character XML cannot hold was left out.
    dropped: bool,
}

impl Write<'_> {
    /// A name XML accepts: a prefix is kept only where it is declared; the rest is repaired, and said so once.
    fn name(&mut self, name: &str, attribute: bool) -> String {
        let out = match name.split_once(':') {
            Some((prefix, local))
                if !prefix.is_empty()
                    && !local.is_empty()
                    && ncname(prefix) == prefix
                    // (`xmlns:p` declares a prefix: only an attribute does.)
                    && (prefix == "xml" || (prefix == "xmlns" && attribute) || self.prefixes.iter().any(|p| p == prefix)) =>
            {
                format!("{prefix}:{}", ncname(local))
            }
            _ => ncname(name),
        };
        if out != name {
            let note = format!("'{name}' is not a name in XML, written as '{out}'");
            if !self.warnings.contains(&note) {
                self.warnings.push(note);
            }
        }
        out
    }

    fn pad(&mut self, depth: usize) {
        if let Some(n) = self.indent {
            self.out.push('\n');
            self.out.push_str(&" ".repeat(n.saturating_mul(depth)));
        }
    }

    /// The element `name` for `value` (an array writes one element for each item).
    fn element(&mut self, name: &str, value: &Value, depth: usize) -> Result<(), ConvertError> {
        if depth > MAX_DEPTH {
            return Err(error(None, "the data is nested too deeply"));
        }
        match &value.data {
            Data::Array(items) => {
                if items.len() < 2 {
                    let note = "a list of fewer than two items is written as no element or as one, which reads back as nothing or as a single value"
                        .to_owned();
                    if !self.warnings.contains(&note) {
                        self.warnings.push(note);
                    }
                }
                for item in items {
                    // An array inside an array has no name of its own: its items are `item` elements in an element called `name`.
                    if matches!(item.data, Data::Array(_)) {
                        let wrapper = Value {
                            span: 0..0,
                            data: Data::Object(vec![Member { key: self.options.item.clone(), key_span: 0..0, value: item.clone() }]),
                        };
                        self.element(name, &wrapper, depth)?;
                    } else {
                        self.element(name, item, depth)?;
                    }
                }
                Ok(())
            }
            Data::Object(members) => {
                let declared = self.prefixes.len();
                for m in members {
                    if let Some(p) = m.key.strip_prefix(&format!("{}xmlns:", self.options.attribute_prefix)) {
                        self.prefixes.push(p.to_owned());
                    }
                }
                let tag = self.name(name, false);
                self.pad_if(depth);
                self.out.push('<');
                self.out.push_str(&tag);
                let mut seen = std::collections::HashSet::new();
                let mut text = String::new();
                let mut children: Vec<&Member> = Vec::new();
                for m in members {
                    if let Some(attr) = m.key.strip_prefix(&self.options.attribute_prefix).filter(|_| !self.options.attribute_prefix.is_empty()) {
                        if matches!(m.value.data, Data::Array(_) | Data::Object(_)) {
                            return Err(error(None, format!("the attribute '{attr}' has a list or an object for a value")));
                        }
                        let attr = self.name(attr, true);
                        if seen.insert(attr.clone()) {
                            let value = escape(&scalar_text(&m.value), true, &mut self.dropped);
                            self.out.push_str(&format!(" {attr}=\"{value}\""));
                        } else {
                            let note = format!("the attribute '{attr}' was given more than once; only the first is written");
                            if !self.warnings.contains(&note) {
                                self.warnings.push(note);
                            }
                        }
                    } else if m.key == self.options.text_key {
                        if matches!(m.value.data, Data::Array(_) | Data::Object(_)) {
                            return Err(error(None, format!("the text '{}' has a list or an object for a value", m.key)));
                        }
                        text.push_str(&scalar_text(&m.value));
                    } else {
                        children.push(m);
                    }
                }
                if text.is_empty() && children.is_empty() {
                    self.out.push_str("/>");
                    self.prefixes.truncate(declared);
                    return Ok(());
                }
                self.out.push('>');
                let escaped = escape(&text, false, &mut self.dropped);
                self.out.push_str(&escaped);
                let before = self.out.len();
                for m in &children {
                    self.element(&m.key, &m.value, depth + 1)?;
                }
                if self.out.len() > before {
                    self.pad_if(depth);
                }
                self.out.push_str(&format!("</{tag}>"));
                self.prefixes.truncate(declared);
                Ok(())
            }
            scalar => {
                let tag = self.name(name, false);
                self.pad_if(depth);
                let text = scalar_text(&Value { span: 0..0, data: scalar.clone() });
                if text.is_empty() {
                    self.out.push_str(&format!("<{tag}/>"));
                } else {
                    let escaped = escape(&text, false, &mut self.dropped);
                    self.out.push_str(&format!("<{tag}>{escaped}</{tag}>"));
                }
                Ok(())
            }
        }
    }

    /// A new line and the indentation, except at the very start.
    fn pad_if(&mut self, depth: usize) {
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.pad(depth);
        }
    }
}

/// XML for data: an object with one member is that element; anything else is the content of `options.root`. `indent` is the spaces for
/// each level (none: one line). Names XML does not accept are changed and said so in `warnings`.
pub fn value_to_xml(value: &Value, options: &DataToXml, indent: Option<usize>, warnings: &mut Vec<String>) -> Result<String, ConvertError> {
    let indent = indent.map(|n| n.min(MAX_INDENT));
    let mut w = Write { options, indent, warnings, out: String::new(), prefixes: Vec::new(), dropped: false };
    if options.declaration {
        w.out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    }
    let rooted = match &value.data {
        Data::Object(members) => {
            members.len() == 1
                && members.first().is_some_and(|m| {
                    !m.key.starts_with(&options.attribute_prefix) && m.key != options.text_key && !matches!(m.value.data, Data::Array(_))
                })
        }
        _ => false,
    };
    if rooted {
        if let Data::Object(members) = &value.data
            && let Some(m) = members.first()
        {
            w.element(&m.key, &m.value, 0)?;
        }
    } else {
        // A list at the top is the items of the root; any other value is its content.
        let content = match &value.data {
            Data::Array(items) => Value {
                span: 0..0,
                data: Data::Object(vec![Member { key: options.item.clone(), key_span: 0..0, value: val(Data::Array(items.clone())) }]),
            },
            Data::Object(_) => value.clone(),
            scalar => {
                Value { span: 0..0, data: Data::Object(vec![Member { key: options.text_key.clone(), key_span: 0..0, value: val(scalar.clone()) }]) }
            }
        };
        let root = options.root.clone();
        w.element(&root, &content, 0)?;
    }
    if indent.is_some() {
        w.out.push('\n');
    }
    if w.dropped {
        w.warnings.push("characters that XML cannot hold (such as most control characters) were left out".to_owned());
    }
    Ok(w.out)
}
