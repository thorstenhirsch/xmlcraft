//! Completion in an HTML document: after `<` the elements, after the name in a start tag the attributes the element takes, and inside the
//! quotes of an attribute the values it lists. The place of the caret is found by reading the text before it (a tag that is being typed
//! has no tree yet); what may be offered is the table of `xmlcraft_html::data`.

use std::ops::Range;

use xmlcraft_html::data::{ELEMENTS, GLOBAL_ATTRIBUTES, attribute_values, attributes_of};

/// The most items offered.
pub const MAX_ITEMS: usize = 300;
/// How far back from the caret the start of the tag is looked for.
const LOOK_BACK: usize = 8 * 1024;

/// What is offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HtmlKind {
    Element,
    Attribute,
    Value,
}

/// One thing to offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HtmlItem {
    pub label: String,
    /// What is written in place of `replace`, and where the caret goes in it (bytes from its start).
    pub insert: String,
    pub cursor: usize,
    pub detail: String,
}

/// What is offered at a place and what it replaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HtmlCompletion {
    pub kind: HtmlKind,
    pub replace: Range<usize>,
    pub items: Vec<HtmlItem>,
}

/// Attributes that are there or not and have no value to write.
const BOOLEAN: &[&str] = &[
    "allowfullscreen",
    "async",
    "autofocus",
    "autoplay",
    "checked",
    "controls",
    "default",
    "defer",
    "disabled",
    "formnovalidate",
    "hidden",
    "inert",
    "ismap",
    "itemscope",
    "loop",
    "multiple",
    "muted",
    "nomodule",
    "novalidate",
    "open",
    "playsinline",
    "readonly",
    "required",
    "reversed",
    "selected",
];

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.')
}

/// The start of the tag the caret is in (the `<` of it), when it is inside one: the last `<` before `offset` with no `>` after it.
fn tag_start(text: &str, offset: usize) -> Option<usize> {
    let from = offset.saturating_sub(LOOK_BACK);
    let from = (from..=offset).find(|o| text.is_char_boundary(*o))?;
    let before = text.get(from..offset)?;
    let lt = before.rfind('<')?;
    // A `>` between ends the tag, unless it is inside a quoted value (`<a title="a>b" `): then the quotes tell.
    let tag = before.get(lt..)?;
    let mut quote: Option<char> = None;
    for c in tag.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return None,
            _ => {}
        }
    }
    Some(from + lt)
}

/// What to offer at `offset` of an HTML `text`; none when the caret is not at an element name, an attribute name or an attribute value
/// of a tag being written.
pub fn complete(text: &str, offset: usize) -> Option<HtmlCompletion> {
    if !text.is_char_boundary(offset) || offset > text.len() {
        return None;
    }
    let lt = tag_start(text, offset)?;
    // Not inside a comment, a declaration, an end tag or a processing instruction.
    let inside = text.get(lt + 1..offset)?;
    if inside.starts_with(['/', '!', '?']) || text.get(..lt)?.rfind("<!--").is_some_and(|c| !text.get(c..lt).is_some_and(|s| s.contains("-->"))) {
        return None;
    }
    let name_len = inside.find(|c: char| !is_name_char(c)).unwrap_or(inside.len());
    if name_len == inside.len() {
        // The name of the element is being typed.
        let end = offset + text.get(offset..)?.find(|c: char| !is_name_char(c)).unwrap_or(text.len() - offset);
        let partial = inside.to_ascii_lowercase();
        let mut items: Vec<HtmlItem> = ELEMENTS
            .iter()
            .filter(|e| !e.obsolete && e.name.starts_with(&partial))
            .map(|e| {
                let (insert, cursor) =
                    if e.void { (format!("<{}>", e.name), e.name.len() + 2) } else { (format!("<{0}></{0}>", e.name), e.name.len() + 2) };
                HtmlItem { label: e.name.to_owned(), insert, cursor, detail: if e.void { "void".to_owned() } else { String::new() } }
            })
            .collect();
        items.truncate(MAX_ITEMS);
        return Some(HtmlCompletion { kind: HtmlKind::Element, replace: lt..end, items });
    }
    let element = inside.get(..name_len)?.to_ascii_lowercase();
    let after_name = inside.get(name_len..)?;
    // The last attribute: its name, whether a value has begun (and how) and what is typed of it.
    let mut quote: Option<char> = None;
    let mut value_start: Option<usize> = None;
    let mut attribute = String::new();
    let mut name_start: Option<usize> = None;
    let mut given: Vec<String> = Vec::new();
    let mut state_after_equals = false;
    let base = lt + 1 + name_len;
    for (i, c) in after_name.char_indices() {
        let at = base + i;
        match (quote, c) {
            (Some(q), c) if c == q => {
                quote = None;
                value_start = None;
            }
            (Some(_), _) => {}
            (None, '"' | '\'') if state_after_equals => {
                quote = Some(c);
                value_start = Some(at + 1);
                state_after_equals = false;
            }
            (None, '=') => {
                if let Some(s) = name_start.take() {
                    attribute = text.get(s..at)?.trim().to_ascii_lowercase();
                    given.push(attribute.clone());
                }
                state_after_equals = true;
            }
            (None, c) if c.is_whitespace() => {
                if let Some(s) = name_start.take() {
                    let n = text.get(s..at)?.to_ascii_lowercase();
                    if !n.is_empty() {
                        given.push(n);
                    }
                }
                // (White space after `=` keeps the value to come.)
            }
            (None, _) => {
                if state_after_equals {
                    // An unquoted value starts here.
                    value_start = Some(at);
                    state_after_equals = false;
                } else if name_start.is_none() && value_start.is_none() {
                    name_start = Some(at);
                }
            }
        }
    }
    // A value: in quotes (up to the closing quote) or unquoted (up to white space).
    let in_value = quote.is_some() || state_after_equals || value_start.is_some();
    if in_value {
        let start = value_start.unwrap_or(offset);
        let partial = text.get(start..offset)?.to_owned();
        let end = match quote {
            Some(q) => {
                offset + text.get(offset..)?.find([q, '\n', '>']).filter(|n| text.get(offset + n..).is_some_and(|r| r.starts_with(q))).unwrap_or(0)
            }
            None => offset + text.get(offset..)?.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(text.len() - offset),
        };
        let lower = partial.to_ascii_lowercase();
        let mut items: Vec<HtmlItem> = attribute_values(&element, &attribute)
            .iter()
            .filter(|v| v.to_ascii_lowercase().starts_with(&lower))
            .map(|v| HtmlItem { label: (*v).to_owned(), insert: (*v).to_owned(), cursor: v.len(), detail: String::new() })
            .collect();
        items.truncate(MAX_ITEMS);
        return Some(HtmlCompletion { kind: HtmlKind::Value, replace: start..end, items });
    }
    // An attribute name: some letters of it after white space (or none, straight after the element's name and a space).
    let start = name_start.unwrap_or(offset);
    if name_start.is_none() && !after_name.ends_with(char::is_whitespace) {
        return None;
    }
    let partial = text.get(start..offset)?.to_ascii_lowercase();
    let end = offset + text.get(offset..)?.find(|c: char| !is_name_char(c)).unwrap_or(text.len() - offset);
    let mut names: Vec<&str> = attributes_of(&element).iter().chain(GLOBAL_ATTRIBUTES.iter()).copied().collect();
    names.dedup();
    let mut items: Vec<HtmlItem> = names
        .into_iter()
        .filter(|n| n.starts_with(&partial) && !given.iter().any(|g| g == n))
        .map(|n| {
            let (insert, cursor) = if BOOLEAN.contains(&n) { (n.to_owned(), n.len()) } else { (format!("{n}=\"\""), n.len() + 2) };
            let detail = if attributes_of(&element).contains(&n) { String::new() } else { "global".to_owned() };
            HtmlItem { label: n.to_owned(), insert, cursor, detail }
        })
        .collect();
    items.truncate(MAX_ITEMS);
    Some(HtmlCompletion { kind: HtmlKind::Attribute, replace: start..end, items })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn at(marked: &str) -> Option<HtmlCompletion> {
        let offset = marked.find('|').expect("a caret");
        complete(&marked.replacen('|', "", 1), offset)
    }

    fn labels(c: &HtmlCompletion) -> Vec<&str> {
        c.items.iter().map(|i| i.label.as_str()).collect()
    }

    #[test]
    fn after_an_open_bracket_the_elements_that_fit_what_was_typed_are_offered_with_their_text() {
        let c = at("<p>x</p><|").expect("completion");
        assert_eq!((c.kind, c.replace.clone()), (HtmlKind::Element, 8..9));
        assert!(labels(&c).contains(&"div") && labels(&c).contains(&"br") && !labels(&c).contains(&"font"), "obsolete elements are not offered");
        let c = at("<b|").expect("completion");
        assert_eq!(labels(&c), ["b", "base", "bdi", "bdo", "blockquote", "body", "br", "button"]);
        let br = c.items.iter().find(|i| i.label == "br").expect("br");
        assert_eq!((br.insert.as_str(), br.cursor, br.detail.as_str()), ("<br>", 4, "void"));
        let b = c.items.iter().find(|i| i.label == "b").expect("b");
        assert_eq!((b.insert.as_str(), b.cursor), ("<b></b>", 3));
        let c = at("<DI|V>").expect("completion");
        // (Any case; the rest of the name is replaced; the obsolete `dir` is not offered.)
        assert_eq!((labels(&c), c.replace.clone()), (vec!["dialog", "div"], 0..4));
    }

    #[test]
    fn in_a_start_tag_the_attributes_the_element_takes_are_offered_without_those_it_has() {
        let c = at("<a hr|").expect("completion");
        assert_eq!((c.kind, c.replace.clone()), (HtmlKind::Attribute, 3..5));
        assert_eq!(labels(&c), ["href", "hreflang"]);
        assert_eq!(c.items[0].insert, "href=\"\"");
        assert_eq!(c.items[0].cursor, 6);
        let c = at("<a href=\"x\" |").expect("completion");
        assert!(labels(&c).contains(&"target") && labels(&c).contains(&"id") && !labels(&c).contains(&"href"), "{:?}", labels(&c));
        let id = c.items.iter().find(|i| i.label == "id").expect("id");
        assert_eq!(id.detail, "global");
        let c = at("<input dis|").expect("completion");
        assert_eq!(c.items[0].insert, "disabled", "a boolean attribute has no value to write");
        assert_eq!(labels(&at("<div cl|").expect("completion")), ["class"]);
    }

    #[test]
    fn inside_the_quotes_of_an_attribute_its_values_are_offered_and_replaced_whole() {
        let c = at("<input type=\"ch|\">").expect("completion");
        assert_eq!((c.kind, labels(&c)), (HtmlKind::Value, vec!["checkbox"]));
        assert_eq!(c.replace, 13..15);
        let c = at("<input type=\"|\">").expect("completion");
        assert!(labels(&c).contains(&"radio") && labels(&c).len() > 10);
        let c = at("<a target=_b|").expect("completion");
        assert_eq!((labels(&c), c.replace.clone()), (vec!["_blank"], 10..12));
        let c = at("<div dir=\"l|tr\">").expect("completion");
        assert_eq!((labels(&c), c.replace.clone()), (vec!["ltr"], 10..13), "the rest of the value is replaced too");
        assert!(labels(&at("<div id=\"x|\">").expect("completion")).is_empty(), "no values listed for id");
    }

    #[test]
    fn nothing_is_offered_in_text_end_tags_comments_declarations_or_between_attribute_and_name() {
        for marked in ["<p>text|</p>", "<p>a</|", "<!-- <b| -->", "<!DOCTYPE h|", "<p>1 > 2 |", "<a href=\"x\"|"] {
            assert_eq!(at(marked), None, "{marked}");
        }
        assert_eq!(complete("é", 1), None, "not a boundary");
        assert_eq!(complete("<a", 99), None);
        // A `>` inside a quoted value does not end the tag.
        assert!(at("<a title=\"a>b\" hr|").is_some());
    }

    #[test]
    fn no_text_and_no_caret_makes_it_panic() {
        for text in ["<", "<a", "<a ", "<a =", "<a b=\"", "<a b='c", "<!--", "<é ü=\"", "<<<", "<a b=c d=\"e", ""] {
            for offset in 0..=text.len() + 1 {
                let _ = complete(text, offset);
            }
        }
    }
}
