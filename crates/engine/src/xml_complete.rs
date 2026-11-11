//! Completion in an XML document: where the caret is in the markup (an element name after `<`, an end tag after `</`, an attribute name
//! or value in a start tag, an entity after `&`), and the names the document itself already uses.

use std::collections::{BTreeMap, BTreeSet};

use xmlcraft_syntax::{Highlighter, Mode, Quote, SyntaxKind, is_name_char, parse};
use xmlcraft_text::Buffer;

/// The most characters before the caret looked at to find the name of an attribute whose value is open.
const LOOK_BACK: usize = 4096;
/// The most elements, attributes or entities offered from a document.
pub const MAX_NAMES: usize = 500;
/// The most values offered for one attribute.
pub const MAX_VALUES: usize = 200;

/// What is being completed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Element,
    EndTag,
    Attribute,
    Value(String),
    Entity,
}

/// Where the caret is in the text, as far as completion is concerned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    pub kind: Kind,
    /// The byte offset in the document where what the choice replaces starts.
    pub start: usize,
    /// What is typed of the name so far.
    pub partial: String,
}

/// The names a document uses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocumentChoices {
    pub elements: Vec<String>,
    pub attributes: Vec<String>,
    pub values: BTreeMap<String, Vec<String>>,
    pub entities: Vec<String>,
}

/// What to complete at `offset`, after the typing that put it there, or when completion was asked for (`explicit`).
pub fn context(text: &str, offset: usize, explicit: bool) -> Option<Context> {
    if offset > text.len() || !text.is_char_boundary(offset) {
        return None;
    }
    let buffer = Buffer::from_text(text);
    let head = offset;
    let before_caret = buffer.slice(buffer.line_start(buffer.line_of(head))?..head).ok()?;
    let word_len: usize = before_caret.chars().rev().take_while(|c| is_name_char(*c)).map(char::len_utf8).sum();
    let word_start = head.checked_sub(word_len)?;
    let partial = buffer.slice(word_start..head).ok()?;
    let previous = if word_start > 0 { buffer.char_at(buffer.prev_char(word_start)) } else { None };
    let mut highlighter = Highlighter::new();
    match previous {
        Some('/')
            if word_start >= 2 && buffer.char_at(word_start - 2) == Some('<') && highlighter.mode_at(&buffer, word_start - 2) == Mode::Content =>
        {
            Some(Context { kind: Kind::EndTag, start: word_start, partial })
        }
        Some('<') => {
            let at = word_start - 1;
            (highlighter.mode_at(&buffer, at) == Mode::Content).then_some(Context { kind: Kind::Element, start: at, partial })
        }
        Some('&') => {
            let at = word_start - 1;
            matches!(highlighter.mode_at(&buffer, at), Mode::Content | Mode::AttrValue(_)).then_some(Context {
                kind: Kind::Entity,
                start: at,
                partial,
            })
        }
        _ => match highlighter.mode_at(&buffer, word_start) {
            Mode::AttrValue(quote) => value_context(&buffer, head, word_start, quote),
            Mode::Tag => {
                let after_space = previous.is_some_and(char::is_whitespace);
                let in_end_tag = tag_start(&buffer, word_start).is_some_and(|lt| buffer.char_at(lt + 1) == Some('/'));
                (after_space && (explicit || !partial.is_empty()) && !in_end_tag).then_some(Context {
                    kind: Kind::Attribute,
                    start: word_start,
                    partial,
                })
            }
            Mode::Content if explicit && partial.is_empty() && previous.is_none_or(|c| c.is_whitespace() || c == '>') => {
                Some(Context { kind: Kind::Element, start: head, partial })
            }
            _ => None,
        },
    }
}

fn tag_start(buffer: &Buffer, at: usize) -> Option<usize> {
    let from = at.saturating_sub(LOOK_BACK);
    let from = (from..=at).find(|o| buffer.is_boundary(*o))?;
    let before = buffer.slice(from..at).ok()?;
    before.rfind('<').map(|i| from + i)
}

fn value_context(buffer: &Buffer, head: usize, caret_word_start: usize, quote: Quote) -> Option<Context> {
    let q = if quote == Quote::Double { '"' } else { '\'' };
    let from = caret_word_start.saturating_sub(LOOK_BACK);
    let from = (from..=caret_word_start).find(|o| buffer.is_boundary(*o)).unwrap_or(caret_word_start);
    let window = buffer.slice(from..head).ok()?;
    let quote_at = from + window.rfind(q)?;
    let value_start = quote_at + q.len_utf8();
    let before_quote = buffer.slice(from..quote_at).ok()?;
    let name_part = before_quote.trim_end().strip_suffix('=')?.trim_end();
    let name_len: usize = name_part.chars().rev().take_while(|c| is_name_char(*c)).map(char::len_utf8).sum();
    let (rest, name) = name_part.split_at(name_part.len() - name_len);
    if name.is_empty() || !rest.ends_with(char::is_whitespace) {
        return None;
    }
    let name = name.to_owned();
    let partial = buffer.slice(value_start..head).ok()?;
    if partial.contains('<') {
        return None;
    }
    Some(Context { kind: Kind::Value(name), start: value_start, partial })
}

/// The element, attribute, value and entity names the document uses, in order.
pub fn document_choices(text: &str) -> DocumentChoices {
    let parsed = parse(text);
    let root = parsed.syntax();
    let mut elements = BTreeSet::new();
    let mut attributes = BTreeSet::new();
    let mut values: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut entities = BTreeSet::new();
    for node in root.descendants() {
        if node.kind() == SyntaxKind::StartTag {
            if let Some(name) = node.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name) {
                insert_name(&mut elements, name.text());
            }
            for attribute in node.children().filter(|n| n.kind() == SyntaxKind::Attribute) {
                let Some(name) = attribute.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name) else {
                    continue;
                };
                let name = name.text().to_owned();
                insert_name(&mut attributes, &name);
                let Some(value) = attribute.children().find(|n| n.kind() == SyntaxKind::AttrValue) else { continue };
                let value = unquote(value.text().to_string());
                if value.is_empty() {
                    continue;
                }
                // (At most MAX_NAMES attributes keep values, as at most that many names are offered.)
                if !values.contains_key(&name) && values.len() >= MAX_NAMES {
                    continue;
                }
                let seen = values.entry(name).or_default();
                if seen.len() < MAX_VALUES {
                    seen.insert(value);
                }
            }
        }
    }
    for token in root.descendants_with_tokens().filter_map(|c| c.into_token()) {
        if token.kind() != SyntaxKind::EntityRef {
            continue;
        }
        let name = token.text().strip_prefix('&').and_then(|t| t.strip_suffix(';')).unwrap_or_default();
        insert_name(&mut entities, name);
    }
    DocumentChoices {
        elements: elements.into_iter().collect(),
        attributes: attributes.into_iter().collect(),
        values: values.into_iter().map(|(name, values)| (name, values.into_iter().collect())).collect(),
        entities: entities.into_iter().collect(),
    }
}

fn insert_name(names: &mut BTreeSet<String>, name: &str) {
    if !name.is_empty() && names.len() < MAX_NAMES {
        names.insert(name.to_owned());
    }
}

fn unquote(text: String) -> String {
    for quote in ['"', '\''] {
        if let Some(inner) = text.strip_prefix(quote).and_then(|t| t.strip_suffix(quote)) {
            return inner.to_owned();
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(marked: &str, explicit: bool) -> Option<(Kind, String, usize)> {
        let caret = marked.find('¦').expect("a marker");
        let text = marked.replacen('¦', "", 1);
        context(&text, caret, explicit).map(|c| (c.kind, c.partial, c.start))
    }

    #[test]
    fn a_name_after_an_open_bracket_is_an_element_and_after_an_ampersand_an_entity() {
        assert_eq!(at("<r><¦</r>", false), Some((Kind::Element, String::new(), 3)));
        assert_eq!(at("<r>  <bo¦</r>", false), Some((Kind::Element, "bo".to_owned(), 5)), "the replacement starts at the bracket");
        assert_eq!(at("<r>a &¦</r>", false), Some((Kind::Entity, String::new(), 5)));
        assert_eq!(at("<r>x &am¦</r>", false), Some((Kind::Entity, "am".to_owned(), 5)));
        assert_eq!(at("<r><é́x¦</r>", false).map(|c| c.1), Some("é́x".to_owned()), "combining marks are name characters");
    }

    #[test]
    fn an_end_tag_offers_the_name_after_the_slash() {
        assert_eq!(at("<r><b></¦</r>", false), Some((Kind::EndTag, String::new(), 8)));
        assert_eq!(at("<r><b></b¦</r>", false), Some((Kind::EndTag, "b".to_owned(), 8)));
        assert_eq!(at("<!-- </¦ -->", false), None, "not in a comment");
    }

    #[test]
    fn letters_after_white_space_in_a_start_tag_are_an_attribute_even_over_several_lines() {
        assert_eq!(at("<a i¦", false).map(|c| (c.0, c.1)), Some((Kind::Attribute, "i".to_owned())));
        assert_eq!(at("<a x=\"1\" ki¦", false).map(|c| c.1), Some("ki".to_owned()));
        assert_eq!(at("<a x=\"1>2\" ki¦", false).map(|c| c.0), Some(Kind::Attribute), "a > inside a value does not end the tag");
        assert_eq!(at("<a\n   x=\"1\"\n   ki¦", false).map(|c| c.0), Some(Kind::Attribute), "the tag goes on over lines");
        assert_eq!(at("<a ¦", false), None);
        assert_eq!(at("<a ¦", true), Some((Kind::Attribute, String::new(), 3)));
        assert_eq!(at("<a></a ki¦", false), None, "an end tag has no attributes");
        assert_eq!(at("<a x=\"1\"> te¦", false), None);
        assert_eq!(at("text te¦", false), None);
    }

    #[test]
    fn markup_that_is_not_a_tag_offers_nothing() {
        for text in ["<!-- <¦", "<!-- &am¦", "<r><![CDATA[ <¦", "<r><![CDATA[ &am¦", "<?pi <¦", "<!-- <a i¦"] {
            assert_eq!(at(text, false), None, "{text}");
        }
        assert!(at("<r><!-- a --><¦", false).is_some(), "after the comment it is a tag again");
    }

    #[test]
    fn inside_the_quotes_of_an_attribute_its_values_are_offered_whole() {
        assert_eq!(at("<a kind=\"¦", false), Some((Kind::Value("kind".to_owned()), String::new(), 9)));
        assert_eq!(at("<a kind=\"p¦", false).map(|c| c.1), Some("p".to_owned()));
        assert_eq!(at("<a kind='q¦", false).map(|c| (c.0, c.1)), Some((Kind::Value("kind".to_owned()), "q".to_owned())));
        assert_eq!(at("<a id=\"1\" kind = \"¦", false).map(|c| c.0), Some(Kind::Value("kind".to_owned())));
        let (kind, partial, start) = at("<a state=\"not ap¦", false).expect("a value");
        assert_eq!((kind, partial.as_str(), start), (Kind::Value("state".to_owned()), "not ap", 10));
        assert_eq!(at("<a kind=\"p\" x¦", false).map(|c| c.0), Some(Kind::Attribute));
        assert_eq!(at("<a kind=\"it's¦", false).map(|c| c.1), Some("it's".to_owned()));
        assert_eq!(at("<a =\"¦", false), None);
    }

    #[test]
    fn an_explicit_request_offers_an_element_in_content() {
        assert_eq!(at("<r><b/>¦</r>", true), Some((Kind::Element, String::new(), 7)));
        assert_eq!(at("<r><b/> x¦</r>", true), None, "not in the middle of text");
        assert_eq!(at("<r><b/>¦</r>", false), None, "typing alone does not open it");
    }

    #[test]
    fn the_names_a_document_uses_are_collected_once() {
        let choices = document_choices("<a id=\"1\" kind=\"p\">&e;</a><b id=\"2\" kind=\"q\"/>");
        assert_eq!(choices.elements, ["a", "b"]);
        assert_eq!(choices.attributes, ["id", "kind"]);
        assert_eq!(choices.values.get("id").map(|v| v.iter().map(String::as_str).collect::<Vec<_>>()), Some(vec!["1", "2"]));
        assert_eq!(choices.values.get("kind").map(|v| v.iter().map(String::as_str).collect::<Vec<_>>()), Some(vec!["p", "q"]));
        assert_eq!(choices.entities, ["e"]);
    }
}
