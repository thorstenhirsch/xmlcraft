//! Quick fixes: what can be done about a problem, as edits of the text. A fix is never applied by itself; the person chooses it (the Fix
//! button of the Problems panel, Edit ▸ Quick Fix at the caret, or the tool `doc_quick_fix`), and it is one undoable step that changes only
//! what the fix says. A problem that has no obvious fix has none: nothing is guessed.

use std::ops::Range;

use xmlcraft_syntax::{Code, Diagnostic};
use xmlcraft_text::Edit;

/// One way to put a problem right.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickFix {
    /// What it does, in a few words: "Rename the end tag to </a>".
    pub title: String,
    /// The edits, in the coordinates of the text the problem was found in; ascending and not overlapping.
    pub edits: Vec<Edit>,
    /// Where the caret goes afterwards, as an offset in the text as it is after the edits.
    pub caret: Option<usize>,
}

/// The fixes for `diagnostic`, which is one of `all` (the problems of the same text: closing an element also closes the unclosed ones
/// inside it). In the order they are offered; the first is the likely one.
pub fn quick_fixes(text: &str, diagnostic: &Diagnostic, all: &[Diagnostic]) -> Vec<QuickFix> {
    let span = &diagnostic.span;
    let arg = |i: usize| diagnostic.args.get(i).map(String::as_str);
    let mut fixes = Vec::new();
    match diagnostic.code {
        Code::MismatchedEndTag => {
            // `</c` is the span: the name is after the two characters of `</`.
            if let (Some(expected), Some(name)) = (arg(0), span.start.checked_add(2).filter(|s| *s <= span.end)) {
                fixes.push(QuickFix {
                    title: format!("Rename the end tag to </{expected}>"),
                    edits: vec![Edit::replace(name..span.end, expected)],
                    caret: Some(name + expected.len()),
                });
            }
        }
        Code::UnmatchedEndTag => {
            if let Some(range) = end_tag_range(text, span) {
                fixes.push(QuickFix { title: "Remove the end tag".to_owned(), edits: vec![Edit::delete(range.clone())], caret: Some(range.start) });
            }
        }
        Code::UnclosedElement => {
            // This element and the unclosed ones that were opened inside it, innermost first, at the end of the text.
            let mut open: Vec<&Diagnostic> = all.iter().filter(|d| d.code == Code::UnclosedElement && d.span.start >= span.start).collect();
            open.sort_by_key(|d| std::cmp::Reverse(d.span.start));
            let ends: String = open.iter().filter_map(|d| d.args.first()).map(|n| format!("</{n}>")).collect();
            if !ends.is_empty() {
                let at = text.len();
                let title = match arg(0) {
                    Some(name) => format!("Close <{name}> at the end of the document"),
                    None => "Close the element at the end of the document".to_owned(),
                };
                fixes.push(QuickFix { title, edits: vec![Edit::insert(at, ends.clone())], caret: Some(at + ends.len()) });
            }
        }
        Code::UnquotedAttrValue => {
            if let Some(value) = text.get(span.clone()).filter(|v| !v.contains('"')) {
                fixes.push(QuickFix {
                    title: "Put the value in quotes".to_owned(),
                    edits: vec![Edit::replace(span.clone(), format!("\"{value}\""))],
                    caret: Some(span.end + 2),
                });
            }
        }
        Code::AttrWithoutValue => {
            fixes.push(QuickFix {
                title: "Give the attribute an empty value".to_owned(),
                edits: vec![Edit::insert(span.end, "=\"\"")],
                caret: Some(span.end + 2),
            });
        }
        Code::DuplicateAttribute => {
            if let Some(range) = attribute_range(text, span) {
                fixes.push(QuickFix {
                    title: format!("Remove the second {}", arg(0).unwrap_or("attribute")),
                    edits: vec![Edit::delete(range.clone())],
                    caret: Some(range.start),
                });
            }
        }
        Code::BareAmpersand => fixes.push(replacement(span, "&amp;", "Write &amp; for the ampersand")),
        Code::BareLessThan => fixes.push(replacement(span, "&lt;", "Write &lt; for the less-than sign")),
        Code::CdataEndInContent => fixes.push(replacement(span, "]]&gt;", "Write ]]&gt; for the three characters")),
        Code::VcRequiredAttribute | Code::CvcAttributeRequired | Code::XsdMissingAttribute => {
            // (A schema also reports "public or system" for a notation: that is not an attribute name.)
            if let (Some(attribute), Some(at)) = (arg(1).filter(|a| crate::elements::is_element_name(a)), start_tag_insert_point(text, span.end)) {
                let written = format!(" {attribute}=\"\"");
                fixes.push(QuickFix {
                    title: format!("Add the attribute {attribute}"),
                    caret: Some(at + written.len() - 1),
                    edits: vec![Edit::insert(at, written)],
                });
            }
        }
        _ => {}
    }
    fixes
}

fn replacement(span: &Range<usize>, with: &str, title: &str) -> QuickFix {
    QuickFix { title: title.to_owned(), edits: vec![Edit::replace(span.clone(), with)], caret: Some(span.start + with.len()) }
}

/// The whole end tag whose `</name` starts at `span`: through its `>` (and the white space before it).
fn end_tag_range(text: &str, span: &Range<usize>) -> Option<Range<usize>> {
    let rest = text.get(span.end..)?;
    let close = rest.find('>')?;
    // Only white space may be between the name and the `>`.
    rest.get(..close).filter(|between| between.chars().all(char::is_whitespace))?;
    Some(span.start..span.end + close + 1)
}

/// The attribute whose name is at `name`, with the white space before it: `x="2"` in `<a x="1" x="2"/>`, or just ` x` in `<a x="1" x/>`.
/// Never reaches past the next `<` (a value cannot hold one), so an unterminated value does not take other markup with it.
fn attribute_range(text: &str, name: &Range<usize>) -> Option<Range<usize>> {
    let before = text.get(..name.start)?;
    let start = before.trim_end_matches(char::is_whitespace).len();
    let rest = text.get(name.end..)?;
    let mut chars = rest.char_indices().peekable();
    let skip_space = |chars: &mut std::iter::Peekable<std::str::CharIndices<'_>>| {
        while chars.next_if(|(_, c)| c.is_whitespace()).is_some() {}
    };
    skip_space(&mut chars);
    if chars.peek().map(|(_, c)| *c) != Some('=') {
        // A duplicate with no value: just the name.
        return Some(start..name.end);
    }
    chars.next();
    skip_space(&mut chars);
    let (_, quote) = chars.next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    for (i, c) in chars {
        if c == quote {
            return Some(start..name.end + i + 1);
        }
        if c == '<' {
            return None;
        }
    }
    None
}

/// Where a new attribute goes in the start tag whose name ends at `name_end`: before the `>` or the `/>`, after the attributes (a `>` in
/// a quoted value is not the end of the tag).
fn start_tag_insert_point(text: &str, name_end: usize) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut previous_slash_at: Option<usize> = None;
    for (i, c) in text.get(name_end..)?.char_indices() {
        let at = name_end + i;
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '/') => previous_slash_at = Some(at),
            (None, '>') => {
                return Some(match previous_slash_at {
                    Some(slash) if slash + 1 == at => slash,
                    _ => at,
                });
            }
            (None, '<') => return None,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;

    /// The text with the first fix of the first problem that has one applied.
    fn fixed(text: &str) -> Option<String> {
        let analysis = analyze(text, 0);
        analysis.diagnostics.iter().find_map(|d| {
            let fix = quick_fixes(text, d, &analysis.diagnostics).into_iter().next()?;
            let mut out = text.to_owned();
            for edit in fix.edits.iter().rev() {
                out.replace_range(edit.range.clone(), &edit.text);
            }
            Some(out)
        })
    }

    fn fixes_for_code(text: &str, code: Code) -> Vec<QuickFix> {
        let analysis = analyze(text, 0);
        analysis.diagnostics.iter().filter(|d| d.code == code).flat_map(|d| quick_fixes(text, d, &analysis.diagnostics)).collect()
    }

    #[test]
    fn a_mismatched_end_tag_is_renamed_and_a_stray_one_removed() {
        assert_eq!(fixed("<a><b></c></a>").as_deref(), Some("<a><b></b></a>"));
        assert_eq!(fixed("<a/></b>").as_deref(), Some("<a/>"));
        assert_eq!(fixed("<a/></b >\n").as_deref(), Some("<a/>\n"));
    }

    #[test]
    fn unclosed_elements_are_closed_innermost_first_at_the_end() {
        assert_eq!(fixed("<a><b>").as_deref(), Some("<a><b></b></a>"), "the outer fix closes what is inside it too");
        let both = fixes_for_code("<a><b>", Code::UnclosedElement);
        assert_eq!(both.len(), 2);
        assert!(both[0].title.contains("<a>") && both[1].title.contains("<b>"));
        assert_eq!(both[1].edits[0].text, "</b>", "the inner one closes only itself");
        let mut text = String::from("<a><b>x");
        let inner = &fixes_for_code(&text, Code::UnclosedElement)[1];
        text.insert_str(text.len(), &inner.edits[0].text);
        assert_eq!(text, "<a><b>x</b>", "and the outer is still to be closed");
    }

    #[test]
    fn attributes_are_quoted_given_a_value_or_deduplicated() {
        assert_eq!(fixed("<a x=1/>").as_deref(), Some("<a x=\"1\"/>"));
        assert_eq!(fixed("<a x/>").as_deref(), Some("<a x=\"\"/>"));
        assert_eq!(fixed("<a x=\"1\" x=\"2\"/>").as_deref(), Some("<a x=\"1\"/>"));
        assert_eq!(fixed("<a x='1'  y='2' y = '3'/>").as_deref(), Some("<a x='1'  y='2'/>"));
        // A duplicate with no value: the fix for the duplicate (the problem of having no value has its own) takes just the name.
        let text = "<a x=\"1\" x/>";
        let remove = &fixes_for_code(text, Code::DuplicateAttribute)[0];
        assert_eq!((remove.edits[0].range.clone(), remove.edits[0].text.as_str()), (8..10, ""), "the space and the name");
        // An unterminated value does not take the next tag with it.
        let text = "<a x=\"1\" x=\"2><b c=\"3\"/>";
        let analysis = analyze(text, 0);
        let dup = analysis.diagnostics.iter().find(|d| d.code == Code::DuplicateAttribute).expect("a duplicate");
        assert!(quick_fixes(text, dup, &analysis.diagnostics).is_empty(), "it would cross into <b c=...>");
    }

    #[test]
    fn characters_that_cannot_be_text_are_written_as_references() {
        assert_eq!(fixed("<a>x & y</a>").as_deref(), Some("<a>x &amp; y</a>"));
        assert_eq!(fixed("<a>x < y</a>").as_deref(), Some("<a>x &lt; y</a>"));
        assert_eq!(fixed("<a>]]></a>").as_deref(), Some("<a>]]&gt;</a>"));
    }

    #[test]
    fn a_problem_nobody_can_decide_has_no_fix_and_nothing_panics_on_odd_text() {
        for text in ["<a", "<a x=\"1/>", "", "</", "<a>y", "<a/>y", "<>", "<a x=>", "<é></è>", "<a>&</a>é&"] {
            let analysis = analyze(text, 0);
            for d in &analysis.diagnostics {
                for fix in quick_fixes(text, d, &analysis.diagnostics) {
                    let mut out = text.to_owned();
                    for edit in fix.edits.iter().rev() {
                        assert!(out.get(edit.range.clone()).is_some(), "{text:?}: {edit:?}");
                        out.replace_range(edit.range.clone(), &edit.text);
                    }
                    assert!(fix.caret.is_none_or(|c| out.is_char_boundary(c)), "{text:?}: caret {:?}", fix.caret);
                }
            }
        }
        assert!(fixes_for_code("<a\n", Code::UnterminatedTag).is_empty());
    }

    #[test]
    fn an_attribute_name_that_is_a_placeholder_is_not_added() {
        let diag = Diagnostic::new(Code::XsdMissingAttribute, 1..2, vec!["notation".to_owned(), "public or system".to_owned()]);
        assert!(quick_fixes("<a/>", &diag, std::slice::from_ref(&diag)).is_empty());
    }

    #[test]
    fn a_required_attribute_is_added_before_the_end_of_the_start_tag() {
        let d = |args: &[&str], span: Range<usize>| Diagnostic::new(Code::VcRequiredAttribute, span, args.iter().map(|s| (*s).to_owned()).collect());
        for (text, expected) in [
            ("<a/>", "<a id=\"\"/>"),
            ("<a>x</a>", "<a id=\"\">x</a>"),
            ("<a b=\"1>2\"/>", "<a b=\"1>2\" id=\"\"/>"),
            ("<a b='x' >", "<a b='x'  id=\"\">"),
        ] {
            let diag = d(&["a", "id"], 1..2);
            let fix = quick_fixes(text, &diag, std::slice::from_ref(&diag)).into_iter().next().expect("a fix");
            let mut out = text.to_owned();
            for edit in fix.edits.iter().rev() {
                out.replace_range(edit.range.clone(), &edit.text);
            }
            assert_eq!(out, expected, "{text}");
            assert_eq!(&out[fix.caret.expect("a caret") - 1..fix.caret.expect("a caret") + 1], "\"\"", "the caret is between the quotes");
        }
        // Nothing to add to when the tag does not end (the text stops, or another tag starts).
        let diag = d(&["a", "id"], 1..2);
        assert!(quick_fixes("<a b=\"1\"", &diag, std::slice::from_ref(&diag)).is_empty());
        assert!(quick_fixes("<a <b/>", &diag, std::slice::from_ref(&diag)).is_empty());
    }
}
