//! Namespace declarations as the DTD makes them. The parser compares the namespace names of the prefixes as they are written; but an
//! attribute value is what remains after its entities are replaced and, for a type such as NMTOKEN, its spaces trimmed, so two prefixes
//! that are bound to different texts may be bound to the same namespace name, and two attributes `a:x` and `b:x` then have the same
//! expanded name (Namespaces in XML §6.3).

use std::collections::HashMap;
use std::ops::Range;

use rowan::WalkEvent;
use xmlcraft_grammar::{AttributeType, Grammar, Name};
use xmlcraft_syntax::{Code, Diagnostic, NsScope, SyntaxKind, SyntaxNode, decode_references, split_qname};

use crate::attribute::ValueReader;
use crate::model::Dtd;
use crate::parse::DtdDiagnostic;

struct Attr {
    name: String,
    /// The text between the quotes.
    literal: String,
    name_range: Range<usize>,
}

fn attributes_of(start_tag: &SyntaxNode) -> Vec<Attr> {
    let mut out = Vec::new();
    for attribute in start_tag.children().filter(|c| c.kind() == SyntaxKind::Attribute) {
        let Some(name) = attribute.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name) else { continue };
        let Some(value) = attribute.children().find(|c| c.kind() == SyntaxKind::AttrValue) else { continue };
        let text = value.text().to_string();
        let literal = match (text.chars().next(), text.chars().last()) {
            (Some(open @ ('"' | '\'')), Some(close)) if open == close && text.len() >= 2 => {
                text.get(1..text.len() - 1).unwrap_or_default().to_owned()
            }
            _ => text,
        };
        let range = name.text_range();
        out.push(Attr { name: name.text().to_owned(), literal, name_range: usize::from(range.start())..usize::from(range.end()) });
    }
    out
}

/// Which attributes of an element have an expanded name that an earlier one has (and which earlier one), with the namespace names that
/// `scope` gives the prefixes.
fn duplicates(attrs: &[Attr], scope: &NsScope) -> HashMap<usize, usize> {
    let mut seen: HashMap<(String, &str), usize> = HashMap::new();
    let mut out = HashMap::new();
    for (i, attr) in attrs.iter().enumerate() {
        if attr.name == "xmlns" || attr.name.starts_with("xmlns:") {
            continue;
        }
        if let Some((Some(prefix), local)) = split_qname(&attr.name)
            && let Some(uri) = scope.lookup(Some(prefix))
            && let Some(&first) = seen.get(&(uri.to_owned(), local))
        {
            out.insert(i, first);
        } else if let Some((Some(prefix), local)) = split_qname(&attr.name)
            && let Some(uri) = scope.lookup(Some(prefix))
        {
            seen.insert((uri.to_owned(), local), i);
        }
    }
    out
}

/// The namespace problems that only show once the values of the declarations are read the way the DTD says: two attributes of an element
/// with the same expanded name. `grammar` is the DTD compiled (for the types of the declarations and the entities).
pub fn check_namespaces(root: &SyntaxNode, dtd: &Dtd, grammar: &Grammar) -> Vec<DtdDiagnostic> {
    // The two readings of a declaration can differ only when entities are replaced or a type normalises the value.
    let relevant =
        !dtd.general_entities.is_empty() || dtd.attlists.iter().any(|a| a.defs.iter().any(|d| d.name == "xmlns" || d.name.starts_with("xmlns:")));
    if !relevant {
        return Vec::new();
    }
    let reader = ValueReader::new(grammar);
    let mut written = NsScope::new();
    let mut read = NsScope::new();
    let mut out = Vec::new();
    for event in root.preorder() {
        match event {
            WalkEvent::Enter(node) if node.kind() == SyntaxKind::Element => {
                written.push_scope();
                read.push_scope();
                let Some(start) = node.children().find(|c| c.kind() == SyntaxKind::StartTag) else { continue };
                let element =
                    start.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name).map(|t| t.text().to_owned());
                let attrs = attributes_of(&start);
                let decl = element.as_deref().and_then(|name| grammar.element(&Name::local(name)));
                for attr in &attrs {
                    let prefix = if attr.name == "xmlns" {
                        None
                    } else if let Some(prefix) = attr.name.strip_prefix("xmlns:") {
                        Some(prefix)
                    } else {
                        continue;
                    };
                    written.declare(prefix, &decode_references(&attr.literal));
                    let ty = decl.and_then(|d| d.attribute(&Name::local(&attr.name))).map_or(AttributeType::Cdata, |a| a.ty.clone());
                    let value = reader.read(&attr.literal, &ty).unwrap_or_else(|_| decode_references(&attr.literal));
                    read.declare(prefix, &value);
                }
                let before = duplicates(&attrs, &written);
                for (i, first) in duplicates(&attrs, &read) {
                    if before.contains_key(&i) {
                        continue;
                    }
                    let (Some(attr), Some(first)) = (attrs.get(i), attrs.get(first)) else { continue };
                    out.push(DtdDiagnostic {
                        source: 0,
                        diagnostic: Diagnostic::new(
                            Code::DuplicateExpandedAttribute,
                            attr.name_range.clone(),
                            vec![first.name.clone(), attr.name.clone()],
                        ),
                    });
                }
            }
            WalkEvent::Leave(node) if node.kind() == SyntaxKind::Element => {
                written.pop_scope();
                read.pop_scope();
            }
            _ => {}
        }
    }
    out.sort_by_key(|d| d.diagnostic.span.start);
    out
}
