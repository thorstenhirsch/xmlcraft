//! The tree two documents are compared as: elements, attributes and text for XML; objects, arrays and scalars for JSON and YAML. Every
//! node keeps its place in the text, so a difference can be shown where it is.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;

use xmlcraft_json_syntax::{Data, Dialect, Options as JsonOptions, Severity, Value, parse};
use xmlcraft_syntax::{SyntaxKind, SyntaxNode, decode_references};

use crate::lines::Whitespace;

/// Elements nested deeper than this are not compared (the readers of XmlCraft stop long before).
const MAX_DEPTH: usize = 256;

/// What a node is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NodeKind {
    /// The whole XML text: comments and processing instructions before and after the root element, and the root element.
    Document,
    Element,
    Attribute,
    Text,
    Comment,
    ProcessingInstruction,
    Object,
    Array,
    String,
    Number,
    Bool,
    Null,
}

impl NodeKind {
    pub(crate) fn is_leaf(self) -> bool {
        !matches!(self, NodeKind::Document | NodeKind::Element | NodeKind::Object | NodeKind::Array)
    }
}

/// A node and where it is in its text.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub kind: NodeKind,
    /// The name: of an element, an attribute or a processing instruction's target; the key of a member of an object (empty for the
    /// items of an array and for the root).
    pub name: String,
    /// The text of a leaf: the value of an attribute, text, a comment, a string or number written as it is (strings decoded).
    pub text: String,
    /// The attributes (first) and the content of an element; the members of an object; the items of an array.
    pub children: Vec<Node>,
    /// The node in the text.
    pub span: Range<usize>,
    /// The name in the text, for what has one in the text (an attribute, an element's start tag, a member).
    pub name_span: Range<usize>,
    /// The namespace of an element or attribute name (empty: none), as declared where it stands.
    pub namespace: String,
    pub(crate) key: String,
    pub(crate) value: String,
    pub(crate) hash: u64,
}

/// Why a text was not read as a tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffError {
    /// Where, as a byte offset into the text that was read.
    pub offset: Option<usize>,
    pub message: String,
}

impl std::fmt::Display for DiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.offset {
            Some(at) => write!(f, "at byte {at}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for DiffError {}

fn node(kind: NodeKind, name: impl Into<String>, text: impl Into<String>, span: Range<usize>) -> Node {
    Node {
        kind,
        name: name.into(),
        text: text.into(),
        children: Vec::new(),
        name_span: span.start..span.start,
        span,
        namespace: String::new(),
        key: String::new(),
        value: String::new(),
        hash: 0,
    }
}

fn start_of(n: &SyntaxNode) -> usize {
    usize::from(n.text_range().start())
}

fn range_of(n: &SyntaxNode) -> Range<usize> {
    usize::from(n.text_range().start())..usize::from(n.text_range().end())
}

fn name_token(n: &SyntaxNode) -> Option<(String, Range<usize>)> {
    n.children_with_tokens()
        .filter_map(|c| c.into_token())
        .find(|t| t.kind() == SyntaxKind::Name)
        .map(|t| (t.text().to_owned(), usize::from(t.text_range().start())..usize::from(t.text_range().end())))
}

impl Node {
    /// The tree of an XML text. A text that is not well-formed is refused with its first problem (the lines of it can still be compared).
    pub fn from_xml(text: &str) -> Result<Node, DiffError> {
        let parsed = xmlcraft_syntax::parse(text);
        if let Some(d) = parsed.diagnostics().iter().find(|d| d.severity() == Severity::Error) {
            return Err(DiffError { offset: Some(d.span.start), message: format!("the XML is not well-formed: {}", d.message()) });
        }
        let root = parsed.syntax();
        let mut document = node(NodeKind::Document, "", "", 0..text.len());
        for child in root.children() {
            match child.kind() {
                SyntaxKind::Element => document.children.push(xml_element(&child, 0, &mut Vec::new())?),
                SyntaxKind::Comment => document.children.push(xml_comment(&child)),
                SyntaxKind::ProcessingInstruction => document.children.push(xml_pi(&child)),
                _ => {}
            }
        }
        Ok(document)
    }

    /// The tree of a JSON text (comments and trailing commas are accepted). A text that is not valid is refused with its first problem.
    pub fn from_json(text: &str) -> Result<Node, DiffError> {
        let read = parse(text, &JsonOptions { dialect: Dialect::Json5, ..JsonOptions::default() });
        if let Some(d) = read.diagnostics.iter().find(|d| d.severity() == Severity::Error) {
            return Err(DiffError { offset: Some(d.span.start), message: d.message() });
        }
        let root = read.root.ok_or(DiffError { offset: None, message: "the text has no value".to_owned() })?;
        Ok(Node::from_value(&root))
    }

    /// The tree of a YAML text; several documents are an array of them.
    pub fn from_yaml(text: &str) -> Result<Node, DiffError> {
        let read = xmlcraft_yaml_syntax::parse(text, &xmlcraft_yaml_syntax::Options::default());
        if let Some(d) = read.diagnostics.iter().find(|d| d.severity() == Severity::Error) {
            return Err(DiffError { offset: Some(d.span.start), message: d.message() });
        }
        let mut documents = read.documents;
        Ok(match documents.len() {
            1 => Node::from_value(&documents.remove(0)),
            _ => {
                let mut array = node(NodeKind::Array, "", "", 0..text.len());
                array.children = documents.iter().map(Node::from_value).collect();
                array
            }
        })
    }

    /// The tree of a value read from JSON or YAML.
    pub fn from_value(value: &Value) -> Node {
        value_node(value, String::new(), 0..0)
    }

    /// The number of nodes in the tree.
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(Node::count).sum::<usize>()
    }
}

fn value_node(value: &Value, name: String, name_span: Range<usize>) -> Node {
    let mut n = match &value.data {
        Data::Null => node(NodeKind::Null, name, "null", value.span.clone()),
        Data::Bool(b) => node(NodeKind::Bool, name, b.to_string(), value.span.clone()),
        Data::Number(s) => node(NodeKind::Number, name, s.clone(), value.span.clone()),
        Data::String(s) => node(NodeKind::String, name, s.clone(), value.span.clone()),
        Data::Array(items) => {
            let mut n = node(NodeKind::Array, name, "", value.span.clone());
            n.children = items.iter().map(|v| value_node(v, String::new(), v.span.start..v.span.start)).collect();
            n
        }
        Data::Object(members) => {
            let mut n = node(NodeKind::Object, name, "", value.span.clone());
            n.children = members.iter().map(|m| value_node(&m.value, m.key.clone(), m.key_span.clone())).collect();
            n
        }
    };
    n.name_span = name_span;
    n
}

fn xml_comment(c: &SyntaxNode) -> Node {
    let text: String = c
        .children_with_tokens()
        .filter_map(|x| x.into_token())
        .filter(|t| t.kind() == SyntaxKind::CommentText)
        .map(|t| t.text().to_owned())
        .collect();
    node(NodeKind::Comment, "", text, range_of(c))
}

fn xml_pi(p: &SyntaxNode) -> Node {
    let token = |kind| p.children_with_tokens().filter_map(|x| x.into_token()).find(|t| t.kind() == kind).map(|t| t.text().to_owned());
    node(NodeKind::ProcessingInstruction, token(SyntaxKind::PiTarget).unwrap_or_default(), token(SyntaxKind::PiText).unwrap_or_default(), range_of(p))
}

/// The namespace a prefix (empty: the default) stands for in the declarations that are open, innermost last.
fn namespace_of(scope: &[(String, String)], prefix: &str) -> String {
    scope.iter().rev().find(|(p, _)| p == prefix).map(|(_, uri)| uri.clone()).unwrap_or_default()
}

fn xml_element(element: &SyntaxNode, depth: usize, scope: &mut Vec<(String, String)>) -> Result<Node, DiffError> {
    if depth > MAX_DEPTH {
        return Err(DiffError { offset: Some(start_of(element)), message: "elements are nested too deeply to compare".to_owned() });
    }
    let start_tag = element.children().find(|n| n.kind() == SyntaxKind::StartTag);
    let (name, name_span) = start_tag.as_ref().and_then(name_token).unwrap_or_default();
    let mut out = node(NodeKind::Element, name, "", range_of(element));
    out.name_span = name_span;
    let opened = scope.len();
    if let Some(tag) = &start_tag {
        for attribute in tag.children().filter(|n| n.kind() == SyntaxKind::Attribute) {
            let Some((attr_name, attr_span)) = name_token(&attribute) else { continue };
            let raw = attribute.children().find(|n| n.kind() == SyntaxKind::AttrValue).map(|v| v.text().to_string()).unwrap_or_default();
            let inner = raw.trim_matches(|c| c == '"' || c == '\'');
            let value = decode_references(inner);
            if attr_name == "xmlns" {
                scope.push((String::new(), value.clone()));
            } else if let Some(prefix) = attr_name.strip_prefix("xmlns:") {
                scope.push((prefix.to_owned(), value.clone()));
            }
            let mut a = node(NodeKind::Attribute, attr_name, value, range_of(&attribute));
            a.name_span = attr_span;
            out.children.push(a);
        }
    }
    // (Now that the element's own declarations are known:) the namespaces of its name and its attributes' names.
    out.namespace = namespace_of(scope, out.name.rsplit_once(':').map_or("", |(p, _)| p));
    for a in out.children.iter_mut().filter(|c| c.kind == NodeKind::Attribute) {
        a.namespace = match a.name.split_once(':') {
            Some((prefix, _)) if prefix != "xmlns" && prefix != "xml" => namespace_of(scope, prefix),
            _ => String::new(),
        };
    }
    // Text, character references, entity references and CDATA are one run of text until something else comes.
    let mut pending: Option<(String, Range<usize>)> = None;
    let flush = |pending: &mut Option<(String, Range<usize>)>, children: &mut Vec<Node>| {
        if let Some((text, span)) = pending.take() {
            children.push(node(NodeKind::Text, "", text, span));
        }
    };
    let add = |pending: &mut Option<(String, Range<usize>)>, text: String, span: Range<usize>| match pending {
        Some((t, r)) => {
            t.push_str(&text);
            r.end = span.end;
        }
        None => *pending = Some((text, span)),
    };
    for child in element.children_with_tokens() {
        match child {
            rowan::NodeOrToken::Token(t) if matches!(t.kind(), SyntaxKind::Text | SyntaxKind::CharRef | SyntaxKind::EntityRef) => {
                let span = usize::from(t.text_range().start())..usize::from(t.text_range().end());
                add(&mut pending, decode_references(t.text()), span);
            }
            rowan::NodeOrToken::Node(n) => match n.kind() {
                SyntaxKind::Element => {
                    flush(&mut pending, &mut out.children);
                    out.children.push(xml_element(&n, depth + 1, scope)?);
                }
                SyntaxKind::CData => {
                    let cdata: String = n
                        .children_with_tokens()
                        .filter_map(|c| c.into_token())
                        .filter(|t| t.kind() == SyntaxKind::CdataText)
                        .map(|t| t.text().to_owned())
                        .collect();
                    add(&mut pending, cdata, range_of(&n));
                }
                SyntaxKind::Comment => {
                    flush(&mut pending, &mut out.children);
                    out.children.push(xml_comment(&n));
                }
                SyntaxKind::ProcessingInstruction => {
                    flush(&mut pending, &mut out.children);
                    out.children.push(xml_pi(&n));
                }
                _ => {}
            },
            _ => {}
        }
    }
    flush(&mut pending, &mut out.children);
    scope.truncate(opened);
    Ok(out)
}

// --- how the trees are compared --------------------------------------------------------------------------------------------

/// What counts as the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeOptions {
    /// White space in the text and attribute values of XML (text that is nothing but white space is then left out).
    pub whitespace: Whitespace,
    /// Letters of different case in text, attribute values and strings are the same.
    pub ignore_case: bool,
    pub ignore_comments: bool,
    pub ignore_processing_instructions: bool,
    /// The children of an element may be in any order (the order of the members of an object never matters).
    pub ignore_child_order: bool,
    /// Numbers are the same when they are equal (`1.0` and `1`, `1e2` and `100`).
    pub numbers_by_value: bool,
    /// Names are the same when their namespaces and local parts are (`a:item` and `b:item` for one namespace; the declarations of
    /// prefixes are then not compared).
    pub ignore_prefixes: bool,
    /// How much work the comparison of a list of children may do (see [`crate::seq::DEFAULT_BUDGET`]).
    pub budget: usize,
}

impl Default for TreeOptions {
    fn default() -> Self {
        Self {
            whitespace: Whitespace::Exact,
            ignore_case: false,
            ignore_comments: false,
            ignore_processing_instructions: false,
            ignore_child_order: false,
            numbers_by_value: false,
            ignore_prefixes: false,
            budget: crate::seq::DEFAULT_BUDGET,
        }
    }
}

fn normal_text(text: &str, options: &TreeOptions) -> String {
    let base = match options.whitespace {
        Whitespace::Exact => text.to_owned(),
        Whitespace::Ends => text.trim().to_owned(),
        Whitespace::Collapse => text.split_whitespace().collect::<Vec<_>>().join(" "),
        Whitespace::All => text.chars().filter(|c| !c.is_whitespace()).collect(),
    };
    if options.ignore_case { base.to_lowercase() } else { base }
}

/// The value of a number written in JSON as a string that is the same for equal numbers: sign, digits without leading or trailing
/// zeros, and the exponent of the first digit. Anything that is not a plain decimal number is itself.
fn canonical_number(text: &str) -> String {
    let t = text.trim();
    let (negative, rest) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let (mantissa, exponent) = match rest.split_once(['e', 'E']) {
        Some((m, e)) => (m, e.parse::<i64>().ok()),
        None => (rest, Some(0)),
    };
    let Some(exponent) = exponent else { return text.to_owned() };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if (int.is_empty() && frac.is_empty()) || !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) {
        return text.to_owned();
    }
    let digits: String = format!("{int}{frac}");
    let trimmed_start = digits.trim_start_matches('0');
    if trimmed_start.is_empty() {
        return "0".to_owned();
    }
    let leading = digits.len() - trimmed_start.len();
    let significant = trimmed_start.trim_end_matches('0');
    // The exponent of the first significant digit.
    let point = i64::try_from(int.len()).unwrap_or(0) - i64::try_from(leading).unwrap_or(0) - 1;
    let sign = if negative { "-" } else { "" };
    format!("{sign}{significant}e{}", point.saturating_add(exponent))
}

fn local(name: &str) -> &str {
    name.rsplit_once(':').map_or(name, |(_, l)| l)
}

fn hash_of(parts: impl FnOnce(&mut DefaultHasher)) -> u64 {
    let mut h = DefaultHasher::new();
    parts(&mut h);
    h.finish()
}

impl Node {
    /// Apply the options: leave out what is ignored, work out what each node is compared by, and its hash (nodes with equal hashes are
    /// the same; the hash is 64 bits of SipHash over the kind, name, text and children).
    pub(crate) fn seal(&mut self, options: &TreeOptions) {
        self.children.retain(|c| {
            !(options.ignore_comments && c.kind == NodeKind::Comment
                || options.ignore_processing_instructions && c.kind == NodeKind::ProcessingInstruction
                || options.whitespace != Whitespace::Exact && c.kind == NodeKind::Text && normal_text(&c.text, options).is_empty()
                || options.ignore_prefixes && c.kind == NodeKind::Attribute && (c.name == "xmlns" || c.name.starts_with("xmlns:")))
        });
        for c in &mut self.children {
            c.seal(options);
        }
        self.key = if options.ignore_prefixes && matches!(self.kind, NodeKind::Element | NodeKind::Attribute) {
            format!("{{{}}}{}", self.namespace, local(&self.name))
        } else {
            self.name.clone()
        };
        self.value = match self.kind {
            NodeKind::Number if options.numbers_by_value => canonical_number(&self.text),
            NodeKind::Text | NodeKind::Attribute | NodeKind::String => normal_text(&self.text, options),
            NodeKind::Comment | NodeKind::ProcessingInstruction => self.text.clone(),
            _ => self.text.clone(),
        };
        let unordered = matches!(self.kind, NodeKind::Object);
        let mut attributes: Vec<u64> = Vec::new();
        let mut content: Vec<u64> = Vec::new();
        for c in &self.children {
            if c.kind == NodeKind::Attribute || unordered {
                attributes.push(hash_of(|h| (c.key.as_str(), c.hash).hash(h)));
            } else {
                content.push(c.hash);
            }
        }
        attributes.sort_unstable();
        if options.ignore_child_order {
            content.sort_unstable();
        }
        let (kind, key, value) = (self.kind, self.key.clone(), self.value.clone());
        // (A leaf's name matters only for the member of an object it is, which the parent's hash includes.)
        self.hash = hash_of(|h| {
            kind.hash(h);
            if !kind.is_leaf() || kind == NodeKind::Attribute || kind == NodeKind::ProcessingInstruction {
                key.hash(h);
            }
            value.hash(h);
            attributes.hash(h);
            content.hash(h);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_that_are_equal_have_one_form() {
        for (a, b) in [("1", "1.0"), ("1e2", "100"), ("0.5", ".5e0"), ("-0", "0"), ("100", "1.00E+2"), ("0.0010", "1e-3"), ("12.30", "1.23e1")] {
            assert_eq!(canonical_number(a), canonical_number(b), "{a} {b}");
        }
        assert_ne!(canonical_number("1"), canonical_number("2"));
        assert_ne!(canonical_number("1e2"), canonical_number("1e3"));
        assert_ne!(canonical_number("12"), canonical_number("1.2"));
        assert_eq!(canonical_number("0xFF"), "0xFF", "not a decimal number: as written");
        assert_eq!(canonical_number("Infinity"), "Infinity");
    }

    #[test]
    fn xml_becomes_nodes_with_places() {
        let text = "<a x=\"1\">hi<!--c--><b/>&amp;<![CDATA[z]]></a>";
        let root = Node::from_xml(text).expect("reads");
        let a = &root.children[0];
        assert_eq!((a.kind, a.name.as_str(), a.children.len()), (NodeKind::Element, "a", 5));
        assert_eq!(a.children[0].kind, NodeKind::Attribute);
        assert_eq!((a.children[1].text.as_str(), &text[a.children[1].span.clone()]), ("hi", "hi"));
        assert_eq!(a.children[2].kind, NodeKind::Comment);
        assert_eq!(a.children[3].name, "b");
        assert_eq!(a.children[4].text, "&z", "references and CDATA are one run of text");
        assert!(Node::from_xml("<a><b></a>").is_err());
    }

    #[test]
    fn json_and_yaml_become_the_same_nodes() {
        let j = Node::from_json("{\"a\": [1, \"x\", null], \"b\": true}").expect("reads");
        let y = Node::from_yaml("a: [1, x, null]\nb: true\n").expect("reads");
        let shape = |n: &Node| -> Vec<(NodeKind, String)> { n.children.iter().map(|c| (c.kind, c.name.clone())).collect() };
        assert_eq!(shape(&j), shape(&y));
        assert!(Node::from_json("{\"a\": ").is_err());
        assert_eq!(Node::from_yaml("a: 1\n---\nb: 2\n").expect("reads").kind, NodeKind::Array);
    }
}
