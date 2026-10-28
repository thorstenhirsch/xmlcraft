//! What may be put at a position of a document.

use std::ops::Range;

use xmlcraft_grammar::{AttributeDefault, AttributeType, Content, EntityKind, Grammar, Name, Term};
use xmlcraft_syntax::{SyntaxKind, SyntaxNode};

/// An element that may come at the position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElementChoice {
    pub name: String,
    /// The enclosing element cannot be complete without it.
    pub mandatory: bool,
    /// It is declared EMPTY.
    pub empty: bool,
}

/// An attribute of the element at the position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttributeChoice {
    pub name: String,
    /// The element must have it (`#REQUIRED`).
    pub mandatory: bool,
    /// It is given already in the start tag.
    pub used: bool,
    /// The values it can have, when it is an enumeration.
    pub values: Vec<String>,
}

/// A general entity that can be referred to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityChoice {
    pub name: String,
    /// The text it stands for, for an internal entity.
    pub value: Option<String>,
}

/// What the grammar allows at a position.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Choices {
    /// The element whose content the position is in (none outside the root element).
    pub parent: Option<String>,
    /// The element whose attributes are offered: the one whose start tag the position is in, else the enclosing one.
    pub element: Option<String>,
    pub elements: Vec<ElementChoice>,
    pub attributes: Vec<AttributeChoice>,
    pub entities: Vec<EntityChoice>,
    /// Where a new attribute goes: the end of the name and attributes in the start tag of `element`.
    pub attribute_at: Option<usize>,
    /// The position is inside a start tag (so that elements and entities are not offered there).
    pub in_start_tag: bool,
}

fn range_of(node: &SyntaxNode) -> Range<usize> {
    usize::from(node.text_range().start())..usize::from(node.text_range().end())
}

/// The name in a start tag.
fn name_of(element: &SyntaxNode) -> Option<String> {
    let start = element.children().find(|c| c.kind() == SyntaxKind::StartTag)?;
    start.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name).map(|t| t.text().to_owned())
}

/// Where the position is in an element.
enum Place {
    StartTag,
    Content,
    /// In the end tag, or somewhere nothing is offered.
    Nowhere,
}

fn place_in(element: &SyntaxNode, offset: usize) -> Place {
    let Some(start) = element.children().find(|c| c.kind() == SyntaxKind::StartTag) else { return Place::Nowhere };
    let tag = range_of(&start);
    let self_closing = start.children_with_tokens().filter_map(|c| c.into_token()).any(|t| t.kind() == SyntaxKind::SlashGt);
    let closed = start.children_with_tokens().filter_map(|c| c.into_token()).any(|t| matches!(t.kind(), SyntaxKind::Gt | SyntaxKind::SlashGt));
    if offset <= tag.start {
        return Place::Nowhere;
    }
    if offset < tag.end || (!closed && offset == tag.end) {
        return Place::StartTag;
    }
    if self_closing {
        return Place::Nowhere;
    }
    match element.children().find(|c| c.kind() == SyntaxKind::EndTag) {
        Some(end) if offset > range_of(&end).start => Place::Nowhere,
        _ => Place::Content,
    }
}

/// The deepest element that has the position in its start tag or its content.
fn locate(root: &SyntaxNode, offset: usize) -> Option<(SyntaxNode, Place)> {
    let mut current = root.children().find(|c| c.kind() == SyntaxKind::Element)?;
    let mut place = place_in(&current, offset);
    loop {
        if !matches!(place, Place::Content) {
            return Some((current, place));
        }
        // A child element that has the position in it (or at the end of one that is not closed) is the one to look in.
        let inner = current.children().filter(|c| c.kind() == SyntaxKind::Element).find(|child| {
            let r = range_of(child);
            r.start < offset && (offset < r.end || (offset == r.end && !matches!(place_in(child, offset), Place::Nowhere)))
        });
        match inner {
            Some(child) => {
                let inner_place = place_in(&child, offset);
                if matches!(inner_place, Place::Nowhere) {
                    return Some((current, place));
                }
                current = child;
                place = inner_place;
            }
            None => return Some((current, place)),
        }
    }
}

/// What the grammar allows at `offset` of the document `root` is the tree of.
pub fn choices_at(root: &SyntaxNode, offset: usize, grammar: &Grammar) -> Choices {
    let mut out = Choices::default();
    // Before the root element (or where there is none): the root element may be written, if the DOCTYPE names it.
    let before_root = root.children().find(|c| c.kind() == SyntaxKind::Element).is_none_or(|e| offset <= range_of(&e).start);
    if before_root {
        if let Some(name) = &grammar.root
            && let Some(decl) = grammar.element(name)
        {
            out.elements.push(ElementChoice { name: name.local.clone(), mandatory: false, empty: matches!(decl.content, Content::Empty) });
        }
        return out;
    }
    let Some((element, place)) = locate(root, offset) else { return out };
    let Some(name) = name_of(&element) else { return out };
    let decl = grammar.element(&Name::local(&name));
    out.element = Some(name.clone());
    out.in_start_tag = matches!(place, Place::StartTag);
    if matches!(place, Place::Nowhere) {
        out.element = None;
        return out;
    }
    // The attributes of the element: those it declares, and which of them the start tag has.
    let start = element.children().find(|c| c.kind() == SyntaxKind::StartTag);
    if let (Some(decl), Some(start)) = (decl, &start) {
        let given: Vec<String> = start
            .children()
            .filter(|c| c.kind() == SyntaxKind::Attribute)
            .filter_map(|a| {
                a.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name).map(|t| t.text().to_owned())
            })
            .collect();
        out.attributes = decl
            .attributes
            .iter()
            .map(|a| AttributeChoice {
                name: a.name.local.clone(),
                mandatory: matches!(a.default, AttributeDefault::Required),
                used: given.contains(&a.name.local),
                values: match &a.ty {
                    AttributeType::Enumeration(values) | AttributeType::Notation(values) => values.clone(),
                    _ => Vec::new(),
                },
            })
            .collect();
        // After the name and the attributes, before `>` or `/>`.
        let tag = range_of(start);
        let last = start
            .children_with_tokens()
            .filter(|c| c.kind() != SyntaxKind::Gt && c.kind() != SyntaxKind::SlashGt && c.kind() != SyntaxKind::Whitespace)
            .last()
            .map(|c| usize::from(c.text_range().end()))
            .unwrap_or(tag.end);
        out.attribute_at = Some(last);
    }
    if matches!(place, Place::StartTag) {
        return out;
    }
    out.parent = Some(name.clone());
    out.elements = elements_in(&element, offset, decl.map(|d| &d.content), grammar);
    out.entities = entities(grammar);
    out
}

/// The elements the content model allows at `offset` of `element`: after the child elements that come before it.
fn elements_in(element: &SyntaxNode, offset: usize, content: Option<&Content>, grammar: &Grammar) -> Vec<ElementChoice> {
    let choice = |name: &str, mandatory: bool| ElementChoice {
        name: name.to_owned(),
        mandatory,
        empty: grammar.element(&Name::local(name)).is_some_and(|d| matches!(d.content, Content::Empty)),
    };
    match content {
        None | Some(Content::Empty) => Vec::new(),
        Some(Content::Any) => grammar.elements().iter().map(|d| choice(&d.name.local, false)).collect(),
        Some(Content::Mixed(names)) => names.iter().map(|n| choice(&n.local, false)).collect(),
        Some(Content::Elements(model)) => {
            // Follow the children before the position as far as they fit; a child that does not fit ends the following.
            let mut state = model.automaton.start();
            for child in element.children().filter(|c| c.kind() == SyntaxKind::Element) {
                if range_of(&child).end > offset {
                    break;
                }
                let Some(child_name) = name_of(&child) else { continue };
                match model.automaton.step(&state, &Name::local(&child_name)) {
                    Some(step) => state = step.state,
                    None => break,
                }
            }
            model
                .automaton
                .expected(&state)
                .iter()
                .filter_map(|e| match e.term {
                    Term::Element(n) => Some(choice(&n.local, e.mandatory)),
                    Term::Wildcard(_) => None,
                })
                .collect()
        }
    }
}

/// The general entities that can be referred to in text: the parsed ones the grammar declares.
fn entities(grammar: &Grammar) -> Vec<EntityChoice> {
    grammar
        .entities()
        .iter()
        .filter_map(|e| match &e.kind {
            EntityKind::Internal(value) => Some(EntityChoice { name: e.name.clone(), value: Some(value.clone()) }),
            EntityKind::External { .. } => Some(EntityChoice { name: e.name.clone(), value: None }),
            EntityKind::Unparsed { .. } => None,
        })
        .collect()
}
