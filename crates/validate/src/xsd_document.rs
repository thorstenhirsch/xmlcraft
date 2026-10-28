//! Validating a document that has been parsed against a schema: the syntax tree is walked, the names are resolved with the namespace
//! declarations in scope, and the validator is told what is in it.

use std::ops::Range;

use rowan::NodeOrToken;
use rowan::api::SyntaxElementChildren;
use xmlcraft_datatypes::NamespaceContext;
use xmlcraft_syntax::{Diagnostic, SyntaxKind, SyntaxNode, XmlLanguage, char_ref_value};
use xmlcraft_xsd::model::QName;
use xmlcraft_xsd::schema::Schema;

use crate::xsd_validator::{XsdAttribute, XsdValidator};

const XML_NAMESPACE: &str = "http://www.w3.org/XML/1998/namespace";

/// The namespace declarations in scope: the innermost last. An empty namespace name undeclares the default namespace.
struct Scope(Vec<(Option<String>, String)>);

impl NamespaceContext for Scope {
    fn resolve(&self, prefix: Option<&str>) -> Option<String> {
        if prefix == Some("xml") {
            return Some(XML_NAMESPACE.to_owned());
        }
        let found = self.0.iter().rev().find(|(p, _)| p.as_deref() == prefix).map(|(_, uri)| uri.clone());
        found.filter(|uri| !uri.is_empty())
    }
}

impl Scope {
    /// The expanded name of an element (an unprefixed one is in the default namespace) or of an attribute (an unprefixed one is in
    /// none). A prefix that is not declared leaves the name as written, in no namespace; that is a mistake of the namespaces, not of
    /// the schema, and it is reported elsewhere.
    fn expand(&self, written: &str, default_namespace: bool) -> QName {
        match written.split_once(':') {
            Some((prefix, local)) => match self.resolve(Some(prefix)) {
                Some(namespace) => QName { namespace: Some(namespace), local: local.to_owned() },
                None => QName { namespace: None, local: written.to_owned() },
            },
            None => QName { namespace: if default_namespace { self.resolve(None) } else { None }, local: written.to_owned() },
        }
    }
}

fn range_of(node: &SyntaxNode) -> Range<usize> {
    usize::from(node.text_range().start())..usize::from(node.text_range().end())
}

struct Level {
    children: SyntaxElementChildren<XmlLanguage>,
    close: Range<usize>,
    /// How many namespace declarations the element made.
    declared: usize,
}

/// The validity problems of a parsed document against a schema, found by walking the tree. A document with no root element has none.
pub fn validate_against_schema(root: &SyntaxNode, schema: &Schema) -> Vec<Diagnostic> {
    let mut validator = XsdValidator::new(schema);
    let mut scope = Scope(Vec::new());
    let mut stack: Vec<Level> = Vec::new();
    let Some(first) = root.children().find(|c| c.kind() == SyntaxKind::Element) else { return Vec::new() };
    open(&first, &mut validator, &mut scope, &mut stack);
    while let Some(level) = stack.last_mut() {
        let Some(item) = level.children.next() else {
            if let Some(done) = stack.pop() {
                validator.end_element(done.close, &scope);
                let keep = scope.0.len().saturating_sub(done.declared);
                scope.0.truncate(keep);
            }
            continue;
        };
        match item {
            NodeOrToken::Token(token) => {
                let range = usize::from(token.text_range().start())..usize::from(token.text_range().end());
                match token.kind() {
                    SyntaxKind::Text => validator.characters(token.text(), range),
                    SyntaxKind::CharRef => {
                        if let Some(c) = char_ref_value(token.text()) {
                            validator.characters(c.encode_utf8(&mut [0; 4]), range);
                        }
                    }
                    SyntaxKind::EntityRef => match token.text() {
                        "&lt;" => validator.characters("<", range),
                        "&gt;" => validator.characters(">", range),
                        "&amp;" => validator.characters("&", range),
                        "&quot;" => validator.characters("\"", range),
                        "&apos;" => validator.characters("'", range),
                        _ => validator.unknown_text(),
                    },
                    _ => {}
                }
            }
            NodeOrToken::Node(node) => match node.kind() {
                SyntaxKind::Element => open(&node, &mut validator, &mut scope, &mut stack),
                SyntaxKind::CData => {
                    let text = node.text().to_string();
                    let inner = text.strip_prefix("<![CDATA[").map(|t| t.strip_suffix("]]>").unwrap_or(t)).unwrap_or(&text);
                    validator.characters(inner, range_of(&node));
                }
                _ => {}
            },
        }
    }
    validator.finish()
}

/// An element: its start tag is told to the validator, and its content is read next.
fn open(node: &SyntaxNode, validator: &mut XsdValidator<'_>, scope: &mut Scope, stack: &mut Vec<Level>) {
    let Some(start) = node.children().find(|c| c.kind() == SyntaxKind::StartTag) else { return };
    let Some(name_token) = start.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name) else { return };
    let name_range = usize::from(name_token.text_range().start())..usize::from(name_token.text_range().end());
    let mut written: Vec<(String, String, Range<usize>)> = Vec::new();
    let mut declared = 0;
    for attribute in start.children().filter(|c| c.kind() == SyntaxKind::Attribute) {
        let Some(name) = attribute.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name) else { continue };
        let Some(value) = attribute.children().find(|c| c.kind() == SyntaxKind::AttrValue) else { continue };
        let text = value.text().to_string();
        let inner = match (text.chars().next(), text.chars().last()) {
            (Some(open @ ('"' | '\'')), Some(close)) if open == close && text.len() >= 2 => text.get(1..text.len() - 1).unwrap_or_default(),
            _ => text.as_str(),
        };
        let inner = unescape(inner);
        match name.text() {
            "xmlns" => {
                scope.0.push((None, inner));
                declared += 1;
            }
            other => match other.strip_prefix("xmlns:") {
                Some(prefix) => {
                    scope.0.push((Some(prefix.to_owned()), inner));
                    declared += 1;
                }
                None => written.push((other.to_owned(), inner, range_of(&attribute))),
            },
        }
    }
    let attributes: Vec<XsdAttribute<'_>> = written
        .iter()
        .map(|(name, value, range)| XsdAttribute { name: scope.expand(name, false), shown: name, value, range: range.clone() })
        .collect();
    let expanded = scope.expand(name_token.text(), true);
    validator.start_element(&expanded, name_token.text(), name_range.clone(), &attributes, scope);
    let close = node.children().find(|c| c.kind() == SyntaxKind::EndTag).map_or(name_range, |end| range_of(&end));
    stack.push(Level { children: node.children_with_tokens(), close, declared });
}

/// The value of an attribute with the references in it replaced: the predefined entities and the characters; any other reference is
/// kept as written, so the value is not valid for most types, which says it.
fn unescape(value: &str) -> String {
    if !value.contains('&') {
        return value.to_owned();
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find('&') {
        out.push_str(rest.get(..at).unwrap_or_default());
        let tail = rest.get(at..).unwrap_or_default();
        let Some(end) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        let reference = tail.get(..=end).unwrap_or_default();
        match reference {
            "&lt;" => out.push('<'),
            "&gt;" => out.push('>'),
            "&amp;" => out.push('&'),
            "&quot;" => out.push('"'),
            "&apos;" => out.push('\''),
            _ => match char_ref_value(reference) {
                Some(c) => out.push(c),
                None => out.push_str(reference),
            },
        }
        rest = tail.get(end + 1..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

/// A schema a document names for itself with `xsi:schemaLocation` or `xsi:noNamespaceSchemaLocation` on its root element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaHint {
    /// The namespace the schema is for; none for `noNamespaceSchemaLocation`.
    pub namespace: Option<String>,
    /// The location, as written (relative to the document).
    pub location: String,
}

/// The schemas the root element of a parsed document names: the pairs of `xsi:schemaLocation` (an odd word at the end is ignored) and
/// the `xsi:noNamespaceSchemaLocation`, in that order.
pub fn schema_hints(root: &SyntaxNode) -> Vec<SchemaHint> {
    let Some(element) = root.children().find(|c| c.kind() == SyntaxKind::Element) else { return Vec::new() };
    let Some(start) = element.children().find(|c| c.kind() == SyntaxKind::StartTag) else { return Vec::new() };
    let mut scope = Scope(Vec::new());
    let mut written: Vec<(String, String)> = Vec::new();
    for attribute in start.children().filter(|c| c.kind() == SyntaxKind::Attribute) {
        let Some(name) = attribute.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name) else { continue };
        let Some(value) = attribute.children().find(|c| c.kind() == SyntaxKind::AttrValue) else { continue };
        let text = value.text().to_string();
        let inner = match (text.chars().next(), text.chars().last()) {
            (Some(open @ ('"' | '\'')), Some(close)) if open == close && text.len() >= 2 => text.get(1..text.len() - 1).unwrap_or_default(),
            _ => text.as_str(),
        };
        let inner = unescape(inner);
        match name.text() {
            "xmlns" => scope.0.push((None, inner)),
            other => match other.strip_prefix("xmlns:") {
                Some(prefix) => scope.0.push((Some(prefix.to_owned()), inner)),
                None => written.push((other.to_owned(), inner)),
            },
        }
    }
    let mut pairs = Vec::new();
    let mut none = Vec::new();
    for (name, value) in &written {
        let expanded = scope.expand(name, false);
        if expanded.namespace.as_deref() != Some(crate::xsd_validator::XSI_NAMESPACE) {
            continue;
        }
        let words: Vec<&str> = value.split(|c: char| xmlcraft_syntax::is_xml_space(c)).filter(|w| !w.is_empty()).collect();
        match expanded.local.as_str() {
            "schemaLocation" => {
                pairs.extend(words.chunks_exact(2).map(|pair| SchemaHint {
                    namespace: pair.first().map(|w| (*w).to_owned()),
                    location: pair.get(1).map(|w| (*w).to_owned()).unwrap_or_default(),
                }));
            }
            "noNamespaceSchemaLocation" => {
                if let Some(word) = words.first() {
                    none.push(SchemaHint { namespace: None, location: (*word).to_owned() });
                }
            }
            _ => {}
        }
    }
    pairs.extend(none);
    pairs
}
