//! Where the caret is in a start tag that is being written: in the element's name, among its attributes, or in the value of one. A tag
//! that is being typed has no tree yet, so the place is found by reading the text before the caret. Names are as typed (the callers
//! fold case when their language does).

use std::ops::Range;

/// How far back from the caret the start of the tag is looked for.
const LOOK_BACK: usize = 8 * 1024;

/// What the caret is at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    /// The name of the element: `typed` is the text after `<` up to the caret and `replace` the whole name (to be replaced).
    Name { typed: String, replace: Range<usize> },
    /// The value of an attribute: what is typed of it and the range of the whole value (inside its quotes).
    Value { attribute: String, typed: String, replace: Range<usize> },
    /// The name of an attribute, started or not: the names the tag has already.
    Attribute { typed: String, replace: Range<usize>, given: Vec<String> },
}

/// The tag the caret is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TagContext {
    /// Where the `<` is.
    pub lt: usize,
    /// The element's name as typed so far (all of it when the caret is after it).
    pub element: String,
    pub place: Place,
}

pub(crate) fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | ':' | '.')
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

/// The place of the caret at `offset` of `text`; none when it is not at an element name, an attribute name or an attribute value of a
/// start tag being written (text, an end tag, a comment, a declaration or a processing instruction).
pub(crate) fn tag_context(text: &str, offset: usize) -> Option<TagContext> {
    if offset > text.len() || !text.is_char_boundary(offset) {
        return None;
    }
    let lt = tag_start(text, offset)?;
    let inside = text.get(lt + 1..offset)?;
    if inside.starts_with(['/', '!', '?']) || text.get(..lt)?.rfind("<!--").is_some_and(|c| !text.get(c..lt).is_some_and(|s| s.contains("-->"))) {
        return None;
    }
    let name_len = inside.find(|c: char| !is_name_char(c)).unwrap_or(inside.len());
    if name_len == inside.len() {
        let end = offset + text.get(offset..)?.find(|c: char| !is_name_char(c)).unwrap_or(text.len() - offset);
        return Some(TagContext { lt, element: inside.to_owned(), place: Place::Name { typed: inside.to_owned(), replace: lt..end } });
    }
    let element = inside.get(..name_len)?.to_owned();
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
                    attribute = text.get(s..at)?.trim().to_owned();
                    given.push(attribute.clone());
                }
                state_after_equals = true;
            }
            (None, c) if c.is_whitespace() => {
                if let Some(s) = name_start.take() {
                    let n = text.get(s..at)?.to_owned();
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
    if quote.is_some() || state_after_equals || value_start.is_some() {
        let start = value_start.unwrap_or(offset);
        let typed = text.get(start..offset)?.to_owned();
        let end = match quote {
            Some(q) => {
                offset + text.get(offset..)?.find([q, '\n', '>']).filter(|n| text.get(offset + n..).is_some_and(|r| r.starts_with(q))).unwrap_or(0)
            }
            None => offset + text.get(offset..)?.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(text.len() - offset),
        };
        return Some(TagContext { lt, element, place: Place::Value { attribute, typed, replace: start..end } });
    }
    // An attribute name: some letters of it after white space (or none, straight after the element's name and a space).
    let start = name_start.unwrap_or(offset);
    if name_start.is_none() && !after_name.ends_with(char::is_whitespace) {
        return None;
    }
    let typed = text.get(start..offset)?.to_owned();
    let end = offset + text.get(offset..)?.find(|c: char| !is_name_char(c)).unwrap_or(text.len() - offset);
    Some(TagContext { lt, element, place: Place::Attribute { typed, replace: start..end, given } })
}
