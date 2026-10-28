//! A schema document as a tree of elements with namespaces resolved, built from the syntax tree.
//!
//! The reader works on this rather than on the syntax tree: an element has its expanded name, its attributes with decoded values, its
//! text, and the namespace declarations in scope (to read the prefixes of the qualified names in attribute values). The tree is stored
//! in an arena and built without recursion, so a hostile document cannot overflow the stack.

use std::ops::Range;

use rowan::WalkEvent;
use xmlcraft_syntax::{NsScope, SyntaxKind, SyntaxNode, XML_NAMESPACE, decode_references, split_qname};

/// Elements nested deeper than this are not read.
pub const MAX_DEPTH: usize = 256;
/// A document with more elements than this is not read.
pub const MAX_ELEMENTS: usize = 2_000_000;

#[derive(Clone, Debug)]
pub struct Attr {
    /// The namespace of the attribute (`None` for an unprefixed attribute).
    pub namespace: Option<String>,
    pub local: String,
    /// The value after the references are replaced and each tab and line end made a space.
    pub value: String,
    pub name_range: Range<usize>,
    pub value_range: Range<usize>,
}

#[derive(Clone, Debug)]
pub struct El {
    pub namespace: Option<String>,
    pub local: String,
    /// The name as written (with its prefix), for messages.
    pub qname: String,
    pub attrs: Vec<Attr>,
    pub children: Vec<usize>,
    pub parent: Option<usize>,
    pub range: Range<usize>,
    pub name_range: Range<usize>,
    /// The text directly in the element, references replaced.
    pub text: String,
    /// The namespace declarations this element makes: the prefix (`None` for the default namespace) and the namespace name.
    pub declarations: Vec<(Option<String>, String)>,
}

#[derive(Debug, Default)]
pub struct Dom {
    pub elements: Vec<El>,
    /// Prefixes that were used and not bound: the prefix and where.
    pub unbound: Vec<(String, Range<usize>)>,
    /// The tree was cut off because of a limit, and which.
    pub truncated: Option<&'static str>,
}

impl Dom {
    pub fn root(&self) -> Option<usize> {
        (!self.elements.is_empty()).then_some(0)
    }

    /// The namespace `prefix` (`None` for the default namespace) stands for at element `at`, looking outwards.
    pub fn lookup(&self, at: usize, prefix: Option<&str>) -> Option<String> {
        if prefix == Some("xml") {
            return Some(XML_NAMESPACE.to_owned());
        }
        let mut current = Some(at);
        while let Some(i) = current {
            let el = self.elements.get(i)?;
            if let Some((_, uri)) = el.declarations.iter().rev().find(|(p, _)| p.as_deref() == prefix) {
                return (!uri.is_empty()).then(|| uri.clone());
            }
            current = el.parent;
        }
        None
    }

    /// All the declarations in scope at element `at`, innermost first (a prefix once: the innermost binding).
    pub fn in_scope(&self, at: usize) -> Vec<(Option<String>, String)> {
        let mut out: Vec<(Option<String>, String)> = Vec::new();
        let mut current = Some(at);
        while let Some(i) = current {
            let Some(el) = self.elements.get(i) else { break };
            for (prefix, uri) in el.declarations.iter().rev() {
                if !out.iter().any(|(p, _)| p == prefix) {
                    out.push((prefix.clone(), uri.clone()));
                }
            }
            current = el.parent;
        }
        out.retain(|(_, uri)| !uri.is_empty());
        out
    }
}

fn range_of(node: &SyntaxNode) -> Range<usize> {
    let r = node.text_range();
    usize::from(r.start())..usize::from(r.end())
}

/// Build the tree of the document `root`.
pub fn build(root: &SyntaxNode) -> Dom {
    let mut dom = Dom::default();
    let mut scope = NsScope::new();
    let mut open: Vec<usize> = Vec::new();
    for event in root.preorder_with_tokens() {
        match event {
            WalkEvent::Enter(rowan::NodeOrToken::Node(node)) if node.kind() == SyntaxKind::Element => {
                if dom.elements.len() >= MAX_ELEMENTS {
                    dom.truncated = Some("too many elements");
                    return dom;
                }
                if open.len() >= MAX_DEPTH {
                    dom.truncated = Some("elements nested too deeply");
                    return dom;
                }
                let Some(start) = node.children().find(|c| c.kind() == SyntaxKind::StartTag) else { continue };
                scope.push_scope();
                let name_token = start.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name);
                let qname = name_token.as_ref().map(|t| t.text().to_owned()).unwrap_or_default();
                let name_range = name_token.as_ref().map(|t| {
                    let r = t.text_range();
                    usize::from(r.start())..usize::from(r.end())
                });
                let mut raw: Vec<(String, String, Range<usize>, Range<usize>)> = Vec::new();
                for attribute in start.children().filter(|c| c.kind() == SyntaxKind::Attribute) {
                    let Some(name) = attribute.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name) else {
                        continue;
                    };
                    let Some(value) = attribute.children().find(|c| c.kind() == SyntaxKind::AttrValue) else { continue };
                    let text = value.text().to_string();
                    let literal = match (text.chars().next(), text.chars().last()) {
                        (Some(open @ ('"' | '\'')), Some(close)) if open == close && text.len() >= 2 => {
                            text.get(1..text.len() - 1).unwrap_or_default().to_owned()
                        }
                        _ => text,
                    };
                    let decoded: String =
                        decode_references(&literal).chars().map(|c| if matches!(c, '\t' | '\n' | '\r') { ' ' } else { c }).collect();
                    let nr = name.text_range();
                    raw.push((name.text().to_owned(), decoded, usize::from(nr.start())..usize::from(nr.end()), range_of(&value)));
                }
                // The namespace declarations come first, then the names can be resolved.
                let mut declarations = Vec::new();
                for (name, value, _, _) in &raw {
                    if name == "xmlns" {
                        scope.declare(None, value);
                        declarations.push((None, value.clone()));
                    } else if let Some(prefix) = name.strip_prefix("xmlns:") {
                        scope.declare(Some(prefix), value);
                        declarations.push((Some(prefix.to_owned()), value.clone()));
                    }
                }
                let (prefix, local) = match split_qname(&qname) {
                    Some((prefix, local)) => (prefix.map(str::to_owned), local.to_owned()),
                    None => (None, qname.clone()),
                };
                let namespace = scope.lookup(prefix.as_deref()).map(str::to_owned);
                if let Some(p) = &prefix
                    && namespace.is_none()
                    && p != "xml"
                {
                    dom.unbound.push((p.clone(), name_range.clone().unwrap_or_else(|| range_of(&node))));
                }
                let mut attrs = Vec::new();
                for (name, value, name_range, value_range) in raw {
                    if name == "xmlns" || name.starts_with("xmlns:") {
                        continue;
                    }
                    let (aprefix, alocal) = match split_qname(&name) {
                        Some((p, l)) => (p.map(str::to_owned), l.to_owned()),
                        None => (None, name.clone()),
                    };
                    let ans = match &aprefix {
                        Some(p) if p == "xml" => Some(XML_NAMESPACE.to_owned()),
                        Some(p) => {
                            let uri = scope.lookup(Some(p)).map(str::to_owned);
                            if uri.is_none() {
                                dom.unbound.push((p.clone(), name_range.clone()));
                            }
                            uri
                        }
                        None => None,
                    };
                    attrs.push(Attr { namespace: ans, local: alocal, value, name_range, value_range });
                }
                let parent = open.last().copied();
                let index = dom.elements.len();
                dom.elements.push(El {
                    namespace,
                    local,
                    qname,
                    attrs,
                    children: Vec::new(),
                    parent,
                    range: range_of(&node),
                    name_range: name_range.unwrap_or_else(|| range_of(&node)),
                    text: String::new(),
                    declarations,
                });
                if let Some(p) = parent
                    && let Some(parent_el) = dom.elements.get_mut(p)
                {
                    parent_el.children.push(index);
                }
                open.push(index);
            }
            WalkEvent::Leave(rowan::NodeOrToken::Node(node)) if node.kind() == SyntaxKind::Element => {
                if open.pop().is_some() {
                    scope.pop_scope();
                }
            }
            WalkEvent::Enter(rowan::NodeOrToken::Token(token)) => {
                // Text directly in an element (not in its attributes or its children, which are other nodes).
                if matches!(token.kind(), SyntaxKind::Text | SyntaxKind::CharRef | SyntaxKind::EntityRef | SyntaxKind::CdataText)
                    && let Some(&current) = open.last()
                    && token.parent().is_some_and(|p| p.kind() == SyntaxKind::Element || p.kind() == SyntaxKind::CData)
                    && let Some(el) = dom.elements.get_mut(current)
                {
                    match token.kind() {
                        SyntaxKind::CdataText => el.text.push_str(token.text()),
                        _ => el.text.push_str(&decode_references(token.text())),
                    }
                }
            }
            _ => {}
        }
    }
    dom
}
