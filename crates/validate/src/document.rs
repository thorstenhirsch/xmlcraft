//! Validating a document that has been parsed: the syntax tree is walked and the validator is told what is in it, with the entities that
//! the content refers to replaced by their text.

use std::collections::HashMap;
use std::ops::Range;

use rowan::NodeOrToken;
use rowan::api::SyntaxElementChildren;
use xmlcraft_grammar::{EntityKind, Grammar};
use xmlcraft_syntax::{Code, Diagnostic, SyntaxKind, SyntaxNode, XmlLanguage, char_ref_value, parse};

use crate::validator::{Attribute, Validator};

/// How many entities may be open in one another at a place.
pub const MAX_ENTITY_DEPTH: usize = 40;
/// How many tokens and nodes may be read in the replacement texts of entities, in all, counting each time an entity is referred to.
pub const MAX_EVENTS: usize = 5_000_000;
/// How many elements may be open at once, those that entities bring included.
pub const MAX_OPEN: usize = 10_000;

/// What validation may spend on the entities of a document.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Entities open in one another.
    pub entity_depth: usize,
    /// Tokens and nodes read in replacement texts, in all.
    pub entity_events: usize,
    /// Elements open at once.
    pub open_elements: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { entity_depth: MAX_ENTITY_DEPTH, entity_events: MAX_EVENTS, open_elements: MAX_OPEN }
    }
}

fn range_of(node: &SyntaxNode) -> Range<usize> {
    usize::from(node.text_range().start())..usize::from(node.text_range().end())
}

enum Level {
    /// The content of an element.
    Element { children: SyntaxElementChildren<XmlLanguage>, close: Range<usize> },
    /// The text of an entity, read where it was referred to.
    Entity { children: SyntaxElementChildren<XmlLanguage> },
}

struct Walker<'g> {
    grammar: &'g Grammar,
    validator: Validator<'g>,
    stack: Vec<Level>,
    /// Where the entity being read was referred to (the place all problems in it are reported at).
    sites: Vec<Range<usize>>,
    /// The parsed replacement texts, by entity name.
    parsed: HashMap<String, SyntaxNode>,
    limits: Limits,
    events_left: usize,
    /// Entities are still read (not once the budget is spent).
    expanding: bool,
    limit_reported: bool,
    diagnostics: Vec<Diagnostic>,
}

impl<'g> Walker<'g> {
    fn at(&self, range: Range<usize>) -> Range<usize> {
        self.sites.last().cloned().unwrap_or(range)
    }

    fn limit(&mut self, range: Range<usize>, what: &str) {
        if !self.limit_reported {
            self.limit_reported = true;
            self.diagnostics.push(Diagnostic::new(Code::DtdLimit, range, vec![what.to_owned()]));
        }
    }

    /// An element: its start tag is told to the validator, and its content is read next.
    fn element(&mut self, node: &SyntaxNode) {
        if self.stack.len() >= self.limits.open_elements {
            let open = self.limits.open_elements;
            self.limit(self.at(range_of(node)), &format!("more than {open} elements open at once"));
            return;
        }
        let Some(start) = node.children().find(|c| c.kind() == SyntaxKind::StartTag) else { return };
        let name_token = start.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name);
        let Some(name_token) = name_token else { return };
        let name_range = self.at(usize::from(name_token.text_range().start())..usize::from(name_token.text_range().end()));
        let mut owned: Vec<(String, String, Range<usize>)> = Vec::new();
        for attribute in start.children().filter(|c| c.kind() == SyntaxKind::Attribute) {
            let Some(name) = attribute.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name) else { continue };
            let Some(value) = attribute.children().find(|c| c.kind() == SyntaxKind::AttrValue) else { continue };
            let text = value.text().to_string();
            // The text between the quotes (an unquoted value is a well-formedness error, and is read as written).
            let inner = match (text.chars().next(), text.chars().last()) {
                (Some(open @ ('"' | '\'')), Some(close)) if open == close && text.len() >= 2 => text.get(1..text.len() - 1).unwrap_or_default(),
                _ => text.as_str(),
            };
            owned.push((name.text().to_owned(), inner.to_owned(), self.at(range_of(&attribute))));
        }
        let attributes: Vec<Attribute<'_>> = owned.iter().map(|(name, value, range)| Attribute { name, value, range: range.clone() }).collect();
        self.validator.start_element(name_token.text(), name_range.clone(), &attributes);
        let close = node.children().find(|c| c.kind() == SyntaxKind::EndTag).map_or(name_range, |end| self.at(range_of(&end)));
        self.stack.push(Level::Element { children: node.children_with_tokens(), close });
    }

    /// A reference to a general entity in content.
    fn entity(&mut self, name: &str, range: Range<usize>) {
        let range = self.at(range);
        let ch = match name {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => None,
        };
        if ch.is_some() {
            self.validator.character_reference(range);
            return;
        }
        let kind = self.grammar.entity(name).map(|e| &e.kind);
        if !self.expanding && kind.is_some() {
            self.validator.unknown_content(range);
            return;
        }
        if kind.is_none() {
            self.validator.undeclared_entity(name, range);
            return;
        }
        let Some(EntityKind::Internal(text)) = kind else {
            self.validator.unknown_content(range);
            return;
        };
        self.validator.other_content(range.clone());
        let entity_depth = self.stack.iter().filter(|l| matches!(l, Level::Entity { .. })).count();
        if entity_depth >= self.limits.entity_depth {
            let depth = self.limits.entity_depth;
            self.limit(range.clone(), &format!("entities nested more than {depth} deep"));
            self.validator.unknown_content(range);
            return;
        }
        let wrapper = match self.parsed.get(name) {
            Some(node) => node.clone(),
            None => {
                let parsed = parse(&format!("<_>{text}</_>"));
                let Some(node) = parsed.syntax().children().find(|c| c.kind() == SyntaxKind::Element) else {
                    self.validator.unknown_content(range);
                    return;
                };
                self.parsed.insert(name.to_owned(), node.clone());
                node
            }
        };
        self.sites.push(range);
        self.stack.push(Level::Entity { children: wrapper.children_with_tokens() });
    }

    fn run(&mut self, root: &SyntaxNode) {
        let Some(first) = root.children().find(|c| c.kind() == SyntaxKind::Element) else { return };
        self.element(&first);
        while let Some(level) = self.stack.last_mut() {
            let next = match level {
                Level::Element { children, .. } | Level::Entity { children } => children.next(),
            };
            let Some(item) = next else {
                match self.stack.pop() {
                    Some(Level::Element { close, .. }) => self.validator.end_element(close),
                    Some(Level::Entity { .. }) => {
                        self.sites.pop();
                    }
                    None => {}
                }
                continue;
            };
            // The document itself is read once; only what entities bring in is counted.
            let in_entity = !self.sites.is_empty();
            if in_entity && self.events_left == 0 {
                let range = self.sites.last().cloned().unwrap_or(0..0);
                let events = self.limits.entity_events;
                self.limit(range, &format!("the replacement texts of entities were read more than {events} times over"));
                // The entity being read is given up, with the elements it opened, and no entity is read after it: the content of the
                // document goes on being checked, and what an entity would have brought is taken as unknown.
                let cut = self.stack.iter().position(|l| matches!(l, Level::Entity { .. })).unwrap_or(self.stack.len());
                while self.stack.len() > cut {
                    if let Some(Level::Element { close, .. }) = self.stack.pop() {
                        self.validator.end_element(close);
                    }
                }
                self.sites.clear();
                self.expanding = false;
                continue;
            }
            if in_entity {
                self.events_left -= 1;
            }
            match item {
                NodeOrToken::Token(token) => {
                    let range = self.at(usize::from(token.text_range().start())..usize::from(token.text_range().end()));
                    match token.kind() {
                        SyntaxKind::Text => self.validator.characters(token.text(), range),
                        SyntaxKind::CharRef => {
                            if char_ref_value(token.text()).is_some() {
                                self.validator.character_reference(range);
                            }
                        }
                        SyntaxKind::EntityRef => {
                            let text = token.text();
                            let name = text.strip_prefix('&').and_then(|t| t.strip_suffix(';')).unwrap_or(text);
                            self.entity(name, range);
                        }
                        _ => {}
                    }
                }
                NodeOrToken::Node(node) => match node.kind() {
                    SyntaxKind::Element => self.element(&node),
                    SyntaxKind::Comment | SyntaxKind::ProcessingInstruction => {
                        let range = self.at(range_of(&node));
                        self.validator.other_content(range);
                    }
                    SyntaxKind::CData => {
                        let range = self.at(range_of(&node));
                        self.validator.cdata(range);
                    }
                    _ => {}
                },
            }
        }
    }
}

/// The validity problems of a parsed document against the grammar of its DTD (`standalone` is its `standalone="yes"`). Problems in the
/// replacement text of an entity are reported at the reference to it.
pub fn validate_document(root: &SyntaxNode, grammar: &Grammar, standalone: bool) -> Vec<Diagnostic> {
    validate_document_with(root, grammar, standalone, &Limits::default())
}

/// [`validate_document`] with other limits.
pub fn validate_document_with(root: &SyntaxNode, grammar: &Grammar, standalone: bool, limits: &Limits) -> Vec<Diagnostic> {
    let mut walker = Walker {
        grammar,
        validator: Validator::new(grammar, standalone),
        stack: Vec::new(),
        sites: Vec::new(),
        parsed: HashMap::new(),
        limits: limits.clone(),
        events_left: limits.entity_events,
        expanding: true,
        limit_reported: false,
        diagnostics: Vec::new(),
    };
    walker.run(root);
    let mut all = walker.diagnostics;
    all.extend(walker.validator.finish());
    all.sort_by_key(|d| (d.span.start, d.span.end));
    all
}
