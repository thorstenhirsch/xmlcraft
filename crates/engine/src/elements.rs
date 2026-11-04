//! What can be done with the elements of a document that the outline knows: which one the caret is in, renaming one (both its tags),
//! taking its tags away, its path, and the one around it. Every function reads the outline of an analysis (the elements in document
//! order with their depth and ranges) and returns the edits or the text, and changes nothing.

use std::ops::Range;

use xmlcraft_syntax::{is_name_char, is_name_start_char};
use xmlcraft_text::Edit;

use crate::analysis::OutlineItem;

/// The index of the innermost element whose start tag and end tag (or its whole empty-element tag) enclose `offset`.
pub fn innermost(outline: &[OutlineItem], offset: usize) -> Option<usize> {
    outline.iter().rposition(|item| item.range.start <= offset && offset <= item.range.end)
}

/// The index of the element that holds the element at `index`.
pub fn parent(outline: &[OutlineItem], index: usize) -> Option<usize> {
    let depth = outline.get(index)?.depth.checked_sub(1)?;
    outline.get(..index)?.iter().rposition(|item| item.depth == depth)
}

/// Whether `name` is a name an element can have (as far as characters go): not empty, a name-start character and then name characters.
pub fn is_element_name(name: &str) -> bool {
    name.chars().next().is_some_and(is_name_start_char) && name.chars().all(is_name_char)
}

/// Why an element cannot be renamed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameError {
    /// The new name is not a name.
    NotAName,
    /// The new name is the old one.
    Unchanged,
}

/// The edits that rename `item` (an element of `text`): its start tag and, when it has one, its end tag, even when the two names differ.
/// The end tag comes after the start tag, so the edits are in ascending order and cannot overlap. Nothing to do when both names are
/// the new one already.
pub fn rename_edits(text: &str, item: &OutlineItem, new_name: &str) -> Result<Vec<Edit>, RenameError> {
    if !is_element_name(new_name) {
        return Err(RenameError::NotAName);
    }
    let end = item.end_name_range.clone();
    let end_is_new = end.as_ref().is_none_or(|range| text.get(range.clone()) == Some(new_name));
    if item.name == new_name && end_is_new {
        return Err(RenameError::Unchanged);
    }
    let mut edits = vec![Edit::replace(item.name_range.clone(), new_name)];
    if let Some(range) = end {
        edits.push(Edit::replace(range, new_name));
    }
    Ok(edits)
}

/// The edits that remove the tags of `item` and keep what is inside it: its start tag and its end tag. An empty-element tag has only the
/// one, so the element goes altogether.
pub fn unwrap_edits(item: &OutlineItem) -> Vec<Edit> {
    let mut edits = vec![Edit::delete(item.start_tag.clone())];
    if let Some(end) = &item.end_tag {
        edits.push(Edit::delete(end.clone()));
    }
    edits
}

/// The range an element is selected by: from its start tag to its end tag.
pub fn element_range(item: &OutlineItem) -> Range<usize> {
    item.range.clone()
}

/// The path of the element at `index` as XPath writes it: `/library/book[2]/title`. A step has a position only when the element has
/// brothers of the same name, since `/library/book` would then be all of them.
pub fn xpath(outline: &[OutlineItem], index: usize) -> Option<String> {
    let mut steps = Vec::new();
    let mut at = Some(index);
    while let Some(i) = at {
        let item = outline.get(i)?;
        let up = parent(outline, i);
        // The brothers are the elements of the parent's subtree one level down (the whole outline, for the root): one pass over them.
        let (from, to) = match up {
            Some(p) => {
                let depth = outline.get(p)?.depth;
                let after = outline.get(p + 1..)?;
                (p + 1, p + 1 + after.iter().position(|b| b.depth <= depth).unwrap_or(after.len()))
            }
            None => (0, outline.len()),
        };
        let (mut total, mut position) = (0usize, 0usize);
        for (j, b) in outline.get(from..to)?.iter().enumerate() {
            if b.depth == item.depth && b.name == item.name {
                total += 1;
                if from + j <= i {
                    position += 1;
                }
            }
        }
        if total > 1 {
            steps.push(format!("{}[{}]", item.name, position));
        } else {
            steps.push(item.name.clone());
        }
        at = up;
    }
    steps.reverse();
    Some(format!("/{}", steps.join("/")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;

    const DOC: &str = "<library>\n  <book id=\"a\"><title>T</title></book>\n  <book id=\"b\"><title>U</title><note/></book>\n</library>";

    fn outline(text: &str) -> Vec<OutlineItem> {
        analyze(text, 0).outline
    }

    fn at(text: &str, needle: &str) -> usize {
        text.find(needle).expect("in the text")
    }

    #[test]
    fn the_innermost_element_is_the_one_the_caret_is_in_and_its_parent_is_the_one_around() {
        let o = outline(DOC);
        let name_of = |offset| innermost(&o, offset).and_then(|i| o.get(i)).map(|i| i.name.clone());
        assert_eq!(name_of(at(DOC, "T</title>")), Some("title".to_owned()));
        assert_eq!(name_of(at(DOC, "<book id=\"a\">") + 3), Some("book".to_owned()), "in a start tag");
        assert_eq!(name_of(at(DOC, "<note/>") + 2), Some("note".to_owned()), "in an empty-element tag");
        assert_eq!(name_of(0), Some("library".to_owned()));
        assert_eq!(name_of(DOC.len()), Some("library".to_owned()), "right after the end of the root");
        assert_eq!(name_of(DOC.len() + 1), None);
        let title = innermost(&o, at(DOC, "T</title>")).expect("an element");
        let book = parent(&o, title).expect("a parent");
        assert_eq!(o.get(book).map(|i| i.name.as_str()), Some("book"));
        let library = parent(&o, book).expect("a parent");
        assert_eq!(parent(&o, library), None, "the root has none");
        assert_eq!(parent(&o, 99), None);
        assert_eq!(innermost(&[], 0), None);
    }

    #[test]
    fn renaming_an_element_renames_both_its_tags_even_when_they_differ() {
        let text = "<a><b>x</b></a>";
        let o = outline(text);
        let b = o.iter().find(|i| i.name == "b").expect("b");
        let edits = rename_edits(text, b, "item").expect("renames");
        assert_eq!(edits.len(), 2);
        let mut e = xmlcraft_text::Buffer::from_text(text);
        for edit in edits.iter().rev() {
            e.apply(edit).expect("applies");
        }
        assert_eq!(e.text(), "<a><item>x</item></a>");
        // Tags that already differ are both set to the new name.
        let broken = "<a><b>x</c></a>";
        let o = outline(broken);
        let b = o.iter().find(|i| i.name == "b").expect("b");
        let edits = rename_edits(broken, b, "b").expect("the end tag is not b: renaming to b fixes it");
        assert_eq!(edits.len(), 2);
        // An element with no end tag has one edit; an empty-element tag too.
        let lone = outline("<a><b/></a>");
        let b = lone.iter().find(|i| i.name == "b").expect("b");
        assert_eq!(rename_edits("<a><b/></a>", b, "c").expect("renames").len(), 1);
    }

    #[test]
    fn a_name_that_is_not_one_or_is_the_same_renames_nothing() {
        let text = "<a><b>x</b></a>";
        let o = outline(text);
        let b = o.iter().find(|i| i.name == "b").expect("b");
        for bad in ["", "1x", "a b", "a<b", "-x", " ", "x>"] {
            assert_eq!(rename_edits(text, b, bad), Err(RenameError::NotAName), "{bad:?}");
        }
        assert_eq!(rename_edits(text, b, "b"), Err(RenameError::Unchanged));
        assert!(rename_edits(text, b, "ns:b").is_ok(), "a prefix is part of the name");
        assert!(rename_edits(text, b, "_b-1.x").is_ok());
        assert!(is_element_name("é") && !is_element_name(""));
    }

    #[test]
    fn unwrapping_takes_the_tags_away_and_keeps_the_content() {
        let text = "<a><b id=\"1\">x<c/>y</b></a>";
        let o = outline(text);
        let b = o.iter().find(|i| i.name == "b").expect("b");
        let mut buffer = xmlcraft_text::Buffer::from_text(text);
        for edit in unwrap_edits(b).iter().rev() {
            buffer.apply(edit).expect("applies");
        }
        assert_eq!(buffer.text(), "<a>x<c/>y</a>");
        // An empty-element tag has only itself: the element goes.
        let c = o.iter().find(|i| i.name == "c").expect("c");
        assert_eq!(unwrap_edits(c).len(), 1);
    }

    #[test]
    fn the_path_numbers_brothers_of_one_name_and_only_those() {
        let o = outline(DOC);
        let path = |needle: &str| innermost(&o, at(DOC, needle)).and_then(|i| xpath(&o, i));
        assert_eq!(path("T</title>").as_deref(), Some("/library/book[1]/title"));
        assert_eq!(path("U</title>").as_deref(), Some("/library/book[2]/title"));
        assert_eq!(path("<note/>").as_deref(), Some("/library/book[2]/note"), "a lone name has no position");
        assert_eq!(path("<library>").as_deref(), Some("/library"));
        assert_eq!(xpath(&o, 99), None);
        // Another parent with the same names does not count: the position is among the brothers.
        let text = "<r><a><x/></a><a><x/><x/></a></r>";
        let o = outline(text);
        let first = text.find("<x/>").expect("x");
        let last = text.rfind("<x/>").expect("x");
        let p = |offset: usize| innermost(&o, offset + 1).and_then(|i| xpath(&o, i));
        assert_eq!(p(first).as_deref(), Some("/r/a[1]/x"));
        assert_eq!(p(last).as_deref(), Some("/r/a[2]/x[2]"));
    }
}
