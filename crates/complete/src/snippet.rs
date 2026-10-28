//! The text to put for a choice, and where the caret goes in it.

use xmlcraft_grammar::{AttributeDefault, AttributeType, Content, Grammar, Name, Term};

/// How deep required children are written inside required children.
pub const MAX_SNIPPET_DEPTH: usize = 6;
/// The most text one snippet may be.
pub const MAX_SNIPPET_BYTES: usize = 16 * 1024;

/// Text to insert, and where the caret goes in it (a byte offset into the text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snippet {
    pub text: String,
    pub caret: usize,
}

fn escape_attribute(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;")
}

/// The attributes the element must have, written as ` name="value"` with a value where one is known.
fn required_attributes(grammar: &Grammar, name: &str) -> String {
    let Some(decl) = grammar.element(&Name::local(name)) else { return String::new() };
    let mut out = String::new();
    for attribute in &decl.attributes {
        if !matches!(attribute.default, AttributeDefault::Required) {
            continue;
        }
        let value = match &attribute.ty {
            AttributeType::Enumeration(values) | AttributeType::Notation(values) => values.first().cloned().unwrap_or_default(),
            _ => String::new(),
        };
        out.push_str(&format!(" {}=\"{}\"", attribute.name.local, escape_attribute(&value)));
    }
    out
}

/// The element `name` as it is written when it is put in: with the attributes it must have and, with `mandatory`, the children it must
/// have (and theirs, to a depth). The caret is in the first required attribute's value, else in the content, else after the tag.
pub fn element_snippet(grammar: &Grammar, name: &str, mandatory: bool) -> Snippet {
    let mut text = String::new();
    let mut caret = None;
    write_element(grammar, name, mandatory, 0, &mut text, &mut caret);
    let caret = caret.unwrap_or(text.len());
    Snippet { text, caret }
}

fn write_element(grammar: &Grammar, name: &str, mandatory: bool, depth: usize, out: &mut String, caret: &mut Option<usize>) {
    let attributes = if mandatory || depth == 0 { required_attributes(grammar, name) } else { String::new() };
    let decl = grammar.element(&Name::local(name));
    if matches!(decl.map(|d| &d.content), Some(Content::Empty)) {
        out.push_str(&format!("<{name}{attributes}/>"));
        if caret.is_none() {
            *caret = Some(first_value(out, name));
        }
        return;
    }
    out.push_str(&format!("<{name}{attributes}>"));
    if caret.is_none() {
        // In the first attribute that needs a value, else in the content.
        *caret = Some(first_value(out, name));
    }
    if mandatory
        && depth < MAX_SNIPPET_DEPTH
        && let Some(Content::Elements(model)) = decl.map(|d| &d.content)
    {
        for symbol in model.automaton.shortest_completion(&model.automaton.start()).unwrap_or_default() {
            if let Term::Element(child) = symbol.term {
                if out.len() >= MAX_SNIPPET_BYTES {
                    break;
                }
                let mut ignored = Some(0);
                write_element(grammar, &child.local, true, depth + 1, out, &mut ignored);
            }
        }
    }
    out.push_str(&format!("</{name}>"));
}

/// Where the caret goes in the tag just written: inside the first empty `""` of its attributes, else just after the tag name's end of
/// the start tag (for the content).
fn first_value(out: &str, name: &str) -> usize {
    let tag_start = out.rfind(&format!("<{name}")).unwrap_or(0);
    let tag = out.get(tag_start..).unwrap_or_default();
    let end = tag.find('>').map_or(tag.len(), |e| e + 1);
    match tag.get(..end).and_then(|t| t.find("=\"\"")) {
        Some(at) => tag_start + at + 2,
        None if tag.get(..end).is_some_and(|t| t.ends_with("/>")) => tag_start + end - 2,
        None => tag_start + end,
    }
}

/// ` name=""`, with the caret between the quotes (or the first value of an enumeration, which is then selected by the caller).
pub fn attribute_snippet(name: &str, first_value: Option<&str>) -> Snippet {
    let value = first_value.map(escape_attribute).unwrap_or_default();
    let text = format!(" {name}=\"{value}\"");
    let caret = text.len() - 1;
    Snippet { text, caret }
}

/// `&name;`
pub fn entity_snippet(name: &str) -> Snippet {
    let text = format!("&{name};");
    Snippet { caret: text.len(), text }
}
