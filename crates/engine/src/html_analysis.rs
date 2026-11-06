//! The analysis of an HTML document: its problems, its outline and what can be folded, from one read of the text (`xmlcraft-html`).

use xmlcraft_html::{Close, Document, Element, parse};

use crate::analysis::{Analysis, MAX_OUTLINE_ITEMS, OutlineItem};
use crate::folding::{FoldKind, FoldRange, Lines, MAX_FOLD_RANGES};
use crate::language::Language;

/// The longest hint shown beside an element's name in the outline.
const MAX_LABEL: usize = 40;

pub(crate) fn analyze_html(text: &str, version: u64) -> Analysis {
    let doc = parse(text);
    let lines = Lines::new(text);
    let (outline, outline_truncated) = outline(&doc);
    let (folds, folds_truncated) = folds(&doc, &lines, text);
    Analysis {
        version,
        language: Language::Html,
        doctype: doc.doctype.clone(),
        diagnostics: doc.diagnostics,
        outline,
        outline_truncated,
        folds,
        folds_truncated,
        doctype_ids: None,
        schema_hints: Vec::new(),
        root_namespace: None,
        skipped: None,
    }
}

/// A short hint for an element: its `id`, else its `name`.
fn label(e: &Element) -> Option<String> {
    let value = ["id", "name"].iter().find_map(|n| e.attribute(n)?.value.as_deref().filter(|v| !v.is_empty()))?;
    let one_line: String = value.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let mut shown: String = one_line.chars().take(MAX_LABEL).collect();
    if one_line.chars().count() > MAX_LABEL {
        shown.push('…');
    }
    Some(shown)
}

fn outline(doc: &Document) -> (Vec<OutlineItem>, bool) {
    let mut items = Vec::new();
    for e in &doc.elements {
        if items.len() >= MAX_OUTLINE_ITEMS {
            return (items, true);
        }
        items.push(OutlineItem {
            depth: e.depth,
            name: e.name.clone(),
            label: label(e),
            range: e.span.clone(),
            name_range: e.name_span.clone(),
            start_tag: e.start_tag.clone(),
            end_tag: e.end_tag.clone(),
            end_name_range: e.end_name_span.clone(),
        });
    }
    let cut = doc.truncated && items.len() >= MAX_OUTLINE_ITEMS;
    (items, cut)
}

/// The folds of the elements and comments that go over lines. An end tag that has a line of its own stays visible.
fn folds(doc: &Document, lines: &Lines<'_>, text: &str) -> (Vec<FoldRange>, bool) {
    let mut folds: Vec<FoldRange> = Vec::new();
    let mut truncated = false;
    let mut push = |fold: FoldRange, folds: &mut Vec<FoldRange>| {
        if folds.len() >= MAX_FOLD_RANGES {
            truncated = true;
        } else {
            folds.push(fold);
        }
    };
    for e in &doc.elements {
        if e.close == Close::Void {
            continue;
        }
        let first = lines.line_of(e.start_tag.end.saturating_sub(1));
        let last = lines.line_of(e.span.end.saturating_sub(1).max(e.start_tag.end.saturating_sub(1)));
        if last <= first {
            continue;
        }
        // An end tag with a line of its own stays visible.
        let own_line = e.end_tag.as_ref().is_some_and(|t| {
            let line = lines.line_of(t.start);
            lines.text_of(line).is_some_and(|l| text.get(l.start..t.start).is_some_and(|before| before.trim().is_empty()))
        });
        let (last_line, end_line) = if own_line { (last.saturating_sub(1), last) } else { (last, last) };
        if last_line <= first {
            continue;
        }
        push(FoldRange { first_line: first, last_line, end_line, level: e.depth + 1, kind: FoldKind::Element }, &mut folds);
    }
    for c in &doc.comments {
        let (first, last) = (lines.line_of(c.start), lines.line_of(c.end.saturating_sub(1)));
        if last > first {
            push(FoldRange { first_line: first, last_line: last, end_line: last, level: 1, kind: FoldKind::Comment }, &mut folds);
        }
    }
    folds.sort_by_key(|f| (f.first_line, std::cmp::Reverse(f.last_line)));
    (folds, truncated)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    const PAGE: &str =
        "<!DOCTYPE html>\n<html>\n<head>\n<title>T</title>\n</head>\n<body>\n<div id=main>\n<p>x\n<p>y\n</div>\n<!-- a\nnote -->\n</body>\n</html>\n";

    #[test]
    fn the_outline_lists_the_elements_with_their_depth_and_a_hint_from_their_id() {
        let a = analyze_html(PAGE, 3);
        assert_eq!((a.version, a.language, a.error_count()), (3, Language::Html, 0));
        let names: Vec<(usize, &str, Option<&str>)> = a.outline.iter().map(|i| (i.depth, i.name.as_str(), i.label.as_deref())).collect();
        assert_eq!(
            names,
            [(0, "html", None), (1, "head", None), (2, "title", None), (1, "body", None), (2, "div", Some("main")), (3, "p", None), (3, "p", None)]
        );
        assert!(a.doctype.is_some());
        let div = a.outline.iter().find(|i| i.name == "div").expect("div");
        assert_eq!(&PAGE[div.range.clone()], "<div id=main>\n<p>x\n<p>y\n</div>");
    }

    #[test]
    fn folds_hide_the_lines_inside_an_element_and_leave_its_end_tag_visible() {
        let a = analyze_html(PAGE, 1);
        let folds: Vec<_> = a.folds.iter().map(|f| (f.first_line, f.last_line, f.end_line, f.kind)).collect();
        // The html element (line 1 to 13), head (2 to 4), body (5 to 12), div (6 to 9) and the comment (10 to 11).
        assert!(folds.contains(&(1, 12, 13, FoldKind::Element)), "{folds:?}");
        assert!(folds.contains(&(2, 3, 4, FoldKind::Element)), "{folds:?}");
        assert!(folds.contains(&(6, 8, 9, FoldKind::Element)), "{folds:?}");
        assert!(folds.contains(&(10, 11, 11, FoldKind::Comment)), "{folds:?}");
        assert!(a.folds.windows(2).all(|w| w[0].first_line <= w[1].first_line), "in the order they start");
    }

    #[test]
    fn problems_come_with_their_places_and_a_broken_text_is_still_analysed() {
        let a = analyze_html("<div><span>x</div><p id=a><p id=a>", 1);
        let keys: Vec<&str> = a.diagnostics.iter().map(|d| d.code.key()).collect();
        assert_eq!(keys, ["html.unclosed-before-end-tag", "html.duplicate-id"]);
        assert_eq!(a.warning_count(), 2);
        let empty = analyze_html("", 1);
        assert!(empty.outline.is_empty() && empty.diagnostics.is_empty());
    }
}
