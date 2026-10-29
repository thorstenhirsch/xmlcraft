//! What an XML Schema allows at a position of a document: the same answer as [`choices_at`](crate::choices_at) gives for a DTD, from
//! the declarations of an assembled schema. The element at the position is typed by walking down from the root element: each child is
//! matched to a declaration by the content model of its parent, after the siblings before it; `xsi:type` changes the type.

use std::sync::Arc;

use xmlcraft_datatypes::{Primitive, SimpleType, Value, Variety};
use xmlcraft_grammar::Name;
use xmlcraft_syntax::{SyntaxKind, SyntaxNode};
use xmlcraft_xsd::model::{ProcessContents, QName};
use xmlcraft_xsd::schema::{AttributeUse, ContentType, ElementId, Schema, TypeId, TypeKind};
use xmlcraft_xsd::{ContentModel, ModelState, Symbol};

use crate::choices::{AttributeChoice, Choices, ElementChoice, Place, attribute_end, locate, name_of, range_of};
use crate::scope::{Scope, XSI_NAMESPACE, attributes_of};

/// The most values of an enumeration that are offered.
pub const MAX_VALUES: usize = 200;
/// The most elements that are offered in one place (a model with more choices than this is cut).
pub const MAX_ELEMENTS: usize = 500;
/// The deepest chain of derivation or union that is looked through for values.
const MAX_DEPTH: usize = 64;

fn start_tag_of(element: &SyntaxNode) -> Option<SyntaxNode> {
    element.children().find(|c| c.kind() == SyntaxKind::StartTag)
}

fn grammar_name(name: &QName) -> Name {
    Name { namespace: name.namespace.clone(), local: name.local.clone() }
}

/// The values a simple type lists: the enumeration of the nearest derivation step that has one, `true` and `false` for a boolean, and
/// the lists of the members of a union.
pub(crate) fn values_of(ty: &SimpleType) -> Vec<String> {
    values_at(ty, 0)
}

fn values_at(ty: &SimpleType, depth: usize) -> Vec<String> {
    if depth > MAX_DEPTH {
        return Vec::new();
    }
    let mut at = Some(ty);
    for _ in 0..MAX_DEPTH {
        let Some(step) = at else { break };
        if let Some(values) = &step.facets().enumeration {
            return values.iter().map(Value::canonical).take(MAX_VALUES).collect();
        }
        at = step.base().map(Arc::as_ref);
    }
    match ty.variety() {
        Variety::Atomic(Primitive::Boolean) => vec!["true".to_owned(), "false".to_owned()],
        Variety::Union(members) => members.iter().flat_map(|m| values_at(m, depth + 1)).take(MAX_VALUES).collect(),
        _ => Vec::new(),
    }
}

/// Whether the element declared as `id` has no content at all (so that it is written as an empty tag).
pub(crate) fn is_empty(schema: &Schema, type_id: TypeId) -> bool {
    matches!(schema.type_entry(type_id).map(|t| &t.kind), Some(TypeKind::Complex(c)) if matches!(c.content, ContentType::Empty))
}

/// The element declared as `id`, as a choice at a place whose namespaces are `scope`. An abstract element is not one that may be
/// written.
fn element_choice(schema: &Schema, id: ElementId, mandatory: bool, scope: &Scope) -> Option<ElementChoice> {
    let decl = schema.element(id)?;
    if decl.abstract_ {
        return None;
    }
    let written = scope.write(decl.name.namespace.as_deref(), &decl.name.local, false);
    Some(ElementChoice { name: written.name, mandatory, empty: is_empty(schema, decl.type_id), id: Some(id.0), declare: written.declare })
}

/// The attributes the type has: which the start tag has, which are required, what values they can take.
fn attribute_choices(schema: &Schema, uses: &[AttributeUse], start: Option<&SyntaxNode>, scope: &Scope) -> Vec<AttributeChoice> {
    let given: Vec<QName> = start.map(attributes_of).unwrap_or_default().into_iter().map(|(name, _)| scope.expand(&name, false)).collect();
    uses.iter()
        .filter_map(|attribute_use| {
            let decl = schema.attribute(attribute_use.attribute)?;
            let written = scope.write(decl.name.namespace.as_deref(), &decl.name.local, true);
            let constraint = attribute_use.value_constraint.as_ref().or(decl.value_constraint.as_ref());
            let mut values = match constraint {
                // A fixed value is the only one there is.
                Some(c) if c.fixed => vec![c.lexical.clone()],
                _ => values_of(&decl.type_),
            };
            // A default is what the attribute has without being written: offered first.
            if let Some(c) = constraint.filter(|c| !c.fixed) {
                values.retain(|v| *v != c.lexical);
                values.insert(0, c.lexical.clone());
            }
            Some(AttributeChoice {
                name: written.name,
                mandatory: attribute_use.required,
                used: given.contains(&decl.name),
                values,
                declare: written.declare,
            })
        })
        .collect()
}

/// What the walk down to an element found out about it.
struct Typed {
    /// The namespaces in scope inside the element (its own declarations included).
    scope: Scope,
    /// Its type: the one `xsi:type` names, else that of its declaration; none when it is not declared or not known.
    type_id: Option<TypeId>,
}

/// Read the child elements of `parent` that come before `stop` says to stop (it is given each child, in order, and is asked before it is
/// read), as far as `model` takes them. The state after the last that fitted, and whether all of them did (when one did not, what comes
/// after it is not known, though the state is where the following left off).
fn advance(parent: &SyntaxNode, parent_scope: &Scope, model: &ContentModel, stop: impl Fn(&SyntaxNode) -> bool) -> (ModelState, bool) {
    let mut state = model.start();
    for sibling in parent.children().filter(|c| c.kind() == SyntaxKind::Element) {
        if stop(&sibling) {
            break;
        }
        let Some(written) = name_of(&sibling) else { continue };
        let mut scope = parent_scope.clone();
        if let Some(start) = start_tag_of(&sibling) {
            scope.declare_in(&start);
        }
        match model.step(&state, &grammar_name(&scope.expand(&written, true))) {
            Some((next, _)) => state = next,
            None => return (state, false),
        }
    }
    (state, true)
}

/// The type an `xsi:type` in the start tag names, if there is one that is declared.
fn xsi_type(schema: &Schema, start: &SyntaxNode, scope: &Scope) -> Option<TypeId> {
    let (_, value) = attributes_of(start).into_iter().find(|(name, _)| {
        let qname = scope.expand(name, false);
        qname.namespace.as_deref() == Some(XSI_NAMESPACE) && qname.local == "type"
    })?;
    // The value is a name of a type: an unprefixed one is in the default namespace.
    schema.type_by_name(&scope.expand(value.trim(), true))
}

/// Type the last element of `chain` (the elements from the root element down to one).
fn type_chain(chain: &[SyntaxNode], schema: &Schema) -> Typed {
    let mut scope = Scope::default();
    let mut typed = Typed { scope: Scope::default(), type_id: None };
    // What is known of the content of the element that was typed last: its model, and the element it belongs to.
    let mut parent: Option<(&SyntaxNode, TypeId)> = None;
    for (depth, node) in chain.iter().enumerate() {
        let outer = scope.clone();
        let start = start_tag_of(node);
        if let Some(start) = &start {
            scope.declare_in(start);
        }
        let qname = name_of(node).map(|n| scope.expand(&n, true));
        // The declaration: a root element is a global one; a child is what its parent's content model says it is.
        let decl: Option<ElementId> = match (&qname, parent) {
            (None, _) => None,
            (Some(q), None) if depth == 0 => schema.element_by_name(q),
            (Some(q), Some((parent_node, parent_type))) => {
                // What the siblings before it left the model at; a sibling that did not fit leaves it unknown.
                let model = schema.content_model(parent_type);
                let state = model.map(|m| advance(parent_node, &outer, m, |sibling| *sibling == *node)).filter(|(_, fitted)| *fitted);
                match (model, state) {
                    (Some(model), Some((state, _))) => match model.step(&state, &grammar_name(q)) {
                        Some((_, Symbol::Element(id))) => Some(*id),
                        Some((_, Symbol::Wildcard(w))) if w.process_contents != ProcessContents::Skip => schema.element_by_name(q),
                        _ => None,
                    },
                    _ => None,
                }
            }
            _ => None,
        };
        let declared = decl.and_then(|id| schema.element(id)).map(|d| d.type_id);
        let type_id = start.as_ref().and_then(|s| xsi_type(schema, s, &scope)).or(declared);
        parent = type_id.map(|t| (node, t));
        typed = Typed { scope: scope.clone(), type_id };
    }
    typed
}

/// Where the root element may be written: the global elements.
fn root_choices(schema: &Schema) -> Vec<ElementChoice> {
    let scope = Scope::default();
    (0..schema.elements.len())
        .filter(|&i| schema.element(ElementId(i)).is_some_and(|d| d.scope.is_none()))
        .filter_map(|i| element_choice(schema, ElementId(i), false, &scope))
        .take(MAX_ELEMENTS)
        .collect()
}

/// What the schema allows at `offset` of the document `root` is the tree of: the elements that may come there (and which must), the
/// attributes of the element, and the values its text may have. An element that is not declared, or that the schema does not let be
/// where it is, offers nothing.
pub fn xsd_choices_at(root: &SyntaxNode, offset: usize, schema: &Schema) -> Choices {
    let mut out = Choices::default();
    let first = root.children().find(|c| c.kind() == SyntaxKind::Element);
    // Before the root element (or where there is none): any global element may be the root.
    if first.as_ref().is_none_or(|e| offset <= range_of(e).start) {
        out.elements = root_choices(schema);
        return out;
    }
    let Some((element, place)) = locate(root, offset) else { return out };
    let Some(shown) = name_of(&element) else { return out };
    if matches!(place, Place::Nowhere) {
        return out;
    }
    out.element = Some(shown.clone());
    out.in_start_tag = matches!(place, Place::StartTag);
    let mut chain: Vec<SyntaxNode> = element.ancestors().filter(|n| n.kind() == SyntaxKind::Element).collect();
    chain.reverse();
    let typed = type_chain(&chain, schema);
    out.scope = typed.scope.declarations().to_vec();
    let Some(type_id) = typed.type_id else { return out };
    let Some(entry) = schema.type_entry(type_id) else { return out };
    let start = start_tag_of(&element);
    let complex = match &entry.kind {
        TypeKind::Complex(c) => Some(c),
        TypeKind::Simple(_) => None,
    };
    if let (Some(complex), Some(start)) = (complex, &start) {
        out.attributes = attribute_choices(schema, &complex.attributes, Some(start), &typed.scope);
        out.attribute_at = Some(attribute_end(start));
    }
    if matches!(place, Place::StartTag) {
        return out;
    }
    out.parent = Some(shown);
    match complex.map(|c| &c.content) {
        // Text of a simple type: the values it lists.
        None => {
            if let TypeKind::Simple(t) = &entry.kind {
                out.values = values_of(t);
            }
        }
        Some(ContentType::Simple(t)) => out.values = values_of(t),
        Some(ContentType::Empty) => {}
        Some(ContentType::ElementOnly(_) | ContentType::Mixed(_)) => {
            let Some(model) = schema.content_model(type_id) else { return out };
            // The children before the position, as far as they fit; a child that does not fit ends the following.
            let (state, _) = advance(&element, &typed.scope, model, |child| range_of(child).end > offset);
            out.elements = model
                .expected(&state)
                .into_iter()
                .filter_map(|next| match next.symbol {
                    Symbol::Element(id) => element_choice(schema, *id, next.mandatory, &typed.scope),
                    Symbol::Wildcard(_) => None,
                })
                .take(MAX_ELEMENTS)
                .collect();
        }
    }
    out
}
