//! The text to put for an element of an XML Schema: its start tag with the attributes it must have, the namespace declarations it needs
//! where it is put, and (when asked) the children it must have, and theirs, to a depth.

use xmlcraft_xsd::schema::{AttributeUse, ContentType, ElementId, Schema, TypeKind};
use xmlcraft_xsd::{ContentModel, Symbol};

use crate::choices::ElementChoice;
use crate::scope::{Declaration, Scope, declaration_text};
use crate::snippet::{MAX_SNIPPET_BYTES, MAX_SNIPPET_DEPTH, Snippet, escape_attribute, first_value};
use crate::xsd::{is_empty, values_of};

/// The value an attribute is written with: the one it is fixed to, else its default, else the first of those it can have, else none.
fn attribute_value(schema: &Schema, attribute_use: &AttributeUse) -> String {
    let Some(decl) = schema.attribute(attribute_use.attribute) else { return String::new() };
    match attribute_use.value_constraint.as_ref().or(decl.value_constraint.as_ref()) {
        Some(c) => c.lexical.clone(),
        None => values_of(&decl.type_).into_iter().next().unwrap_or_default(),
    }
}

/// The element `choice` (from the choices at a place whose namespace declarations are `scope`) as it is written when it is put in: with
/// the attributes it must have and, with `mandatory`, the children it must have (and theirs, to a depth). The caret is in the first
/// required attribute's value, else in the content, else after the tag. An element that is not in the schema is written as a tag.
pub fn xsd_element_snippet(schema: &Schema, scope: &[Declaration], choice: &ElementChoice, mandatory: bool) -> Snippet {
    let mut text = String::new();
    let mut caret = None;
    let scope = Scope::from_declarations(scope);
    match choice.id {
        Some(id) => write_element(schema, ElementId(id), &choice.name, choice.declare.as_ref(), &scope, mandatory, 0, &mut text, &mut caret),
        None => {
            let declare = choice.declare.as_ref().map(declaration_text).unwrap_or_default();
            text = format!("<{}{declare}></{}>", choice.name, choice.name);
            caret = Some(text.len().saturating_sub(choice.name.len() + 3));
        }
    }
    let caret = caret.unwrap_or(text.len());
    Snippet { text, caret }
}

#[allow(clippy::too_many_arguments)] // the state of one recursive walk: what is written, where, in which scope
fn write_element(
    schema: &Schema,
    id: ElementId,
    name: &str,
    declare: Option<&Declaration>,
    scope: &Scope,
    mandatory: bool,
    depth: usize,
    out: &mut String,
    caret: &mut Option<usize>,
) {
    let Some(decl) = schema.element(id) else {
        out.push_str(&format!("<{name}></{name}>"));
        return;
    };
    let type_entry = schema.type_entry(decl.type_id);
    let complex = match type_entry.map(|t| &t.kind) {
        Some(TypeKind::Complex(c)) => Some(c),
        _ => None,
    };
    // The declarations of the start tag: the one the element's own name needs, then those of the attributes it must have.
    let mut inner = scope.with(declare);
    let mut declarations = declare.map(declaration_text).unwrap_or_default();
    let mut attributes = String::new();
    if mandatory || depth == 0 {
        for attribute_use in complex.iter().flat_map(|c| c.attributes.iter()).filter(|u| u.required) {
            let Some(attribute) = schema.attribute(attribute_use.attribute) else { continue };
            let written = inner.write(attribute.name.namespace.as_deref(), &attribute.name.local, true);
            if let Some(extra) = &written.declare {
                declarations.push_str(&declaration_text(extra));
                inner = inner.with(Some(extra));
            }
            attributes.push_str(&format!(" {}=\"{}\"", written.name, escape_attribute(&attribute_value(schema, attribute_use))));
        }
    }
    let tag = format!("{name}{declarations}{attributes}");
    if is_empty(schema, decl.type_id) {
        out.push_str(&format!("<{tag}/>"));
        if caret.is_none() {
            *caret = Some(first_value(out, name));
        }
        return;
    }
    out.push_str(&format!("<{tag}>"));
    if caret.is_none() {
        // In the first attribute that needs a value, else in the content.
        *caret = Some(first_value(out, name));
    }
    if mandatory {
        // A fixed value is the content it has to have; otherwise the children that cannot be left out.
        let fixed = decl.value_constraint.as_ref().filter(|c| c.fixed);
        if let Some(fixed) = fixed {
            out.push_str(&escape_text(&fixed.lexical));
        } else if depth < MAX_SNIPPET_DEPTH
            && let Some(model) = complex.and_then(|c| match c.content {
                ContentType::ElementOnly(_) | ContentType::Mixed(_) => schema.content_model(decl.type_id),
                _ => None,
            })
        {
            write_required_children(schema, model, &inner, depth, out);
        }
    }
    out.push_str(&format!("</{name}>"));
}

fn write_required_children(schema: &Schema, model: &ContentModel, scope: &Scope, depth: usize, out: &mut String) {
    for symbol in model.shortest_completion(&model.start()).unwrap_or_default() {
        let Symbol::Element(child) = symbol else { continue };
        if out.len() >= MAX_SNIPPET_BYTES {
            break;
        }
        let Some(child_decl) = schema.element(*child) else { continue };
        let written = scope.write(child_decl.name.namespace.as_deref(), &child_decl.name.local, false);
        write_element(schema, *child, &written.name, written.declare.as_ref(), scope, true, depth + 1, out, &mut Some(0));
    }
}

fn escape_text(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;")
}
