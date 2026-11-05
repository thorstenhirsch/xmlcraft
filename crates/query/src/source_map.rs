//! Where each node of an XML text is. A node is named by its way down the tree as the data model has it: the number of each child
//! among its parent's children (adjacent text, references and CDATA are one text node; the XML declaration and the DOCTYPE are not
//! nodes) and `@n` for the n-th attribute: `0/2/@1`.

use std::collections::HashMap;
use std::ops::Range;

use xmlcraft_syntax::{SyntaxKind, SyntaxNode, parse};

/// Elements nested deeper than this are not mapped.
const MAX_DEPTH: usize = 256;

/// The places of the nodes of a text.
#[derive(Debug, Default)]
pub struct SourceMap {
    places: HashMap<String, Range<usize>>,
}

impl SourceMap {
    /// The map of a text. A text that is not well-formed has the map of what could be read.
    pub fn of(text: &str) -> SourceMap {
        let parsed = parse(text);
        let root = parsed.syntax();
        let mut map = SourceMap::default();
        // The document node is the whole text.
        map.places.insert(String::new(), 0..text.len());
        let mut index = 0;
        for child in root.children() {
            if matches!(child.kind(), SyntaxKind::Element | SyntaxKind::Comment | SyntaxKind::ProcessingInstruction) {
                let key = index.to_string();
                index += 1;
                if child.kind() == SyntaxKind::Element {
                    map.element(&child, &key, 0);
                } else {
                    map.places.insert(key, range(&child));
                }
            }
        }
        map
    }

    /// The bytes of the node at this way down (see the module): `way` is the numbers of the children, and for an attribute its number.
    pub fn place(&self, key: &str) -> Option<Range<usize>> {
        self.places.get(key).cloned()
    }

    fn element(&mut self, element: &SyntaxNode, key: &str, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        self.places.insert(key.to_owned(), range(element));
        if let Some(tag) = element.children().find(|n| n.kind() == SyntaxKind::StartTag) {
            // (Namespace declarations are not attributes in the data model, so they are not counted.)
            let attributes = tag.children().filter(|a| a.kind() == SyntaxKind::Attribute).filter(|a| !is_declaration(a));
            for (i, attribute) in attributes.enumerate() {
                self.places.insert(format!("{key}/@{i}"), range(&attribute));
            }
        }
        let mut index = 0;
        let mut in_text: Option<Range<usize>> = None;
        let close = |map: &mut SourceMap, in_text: &mut Option<Range<usize>>, index: &mut usize| {
            if let Some(r) = in_text.take() {
                map.places.insert(format!("{key}/{index}"), r);
                *index += 1;
            }
        };
        for child in element.children_with_tokens() {
            match child {
                rowan::NodeOrToken::Token(t) if matches!(t.kind(), SyntaxKind::Text | SyntaxKind::CharRef | SyntaxKind::EntityRef) => {
                    let r = usize::from(t.text_range().start())..usize::from(t.text_range().end());
                    match &mut in_text {
                        Some(open) => open.end = r.end,
                        None => in_text = Some(r),
                    }
                }
                rowan::NodeOrToken::Node(n) => match n.kind() {
                    SyntaxKind::CData => {
                        let r = range(&n);
                        match &mut in_text {
                            Some(open) => open.end = r.end,
                            None => in_text = Some(r),
                        }
                    }
                    SyntaxKind::Element => {
                        close(self, &mut in_text, &mut index);
                        self.element(&n, &format!("{key}/{index}"), depth + 1);
                        index += 1;
                    }
                    SyntaxKind::Comment | SyntaxKind::ProcessingInstruction => {
                        close(self, &mut in_text, &mut index);
                        self.places.insert(format!("{key}/{index}"), range(&n));
                        index += 1;
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        close(self, &mut in_text, &mut index);
    }
}

/// Whether an attribute is a namespace declaration (`xmlns` or `xmlns:p`).
fn is_declaration(attribute: &SyntaxNode) -> bool {
    let name = attribute.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name);
    name.is_some_and(|t| t.text() == "xmlns" || t.text().starts_with("xmlns:"))
}

fn range(n: &SyntaxNode) -> Range<usize> {
    usize::from(n.text_range().start())..usize::from(n.text_range().end())
}
