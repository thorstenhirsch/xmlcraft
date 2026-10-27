//! What in a document can be folded, and the commands that fold it.
//!
//! [`fold_ranges`] reads the syntax tree: an element whose content is on lines of its own, a comment, a CDATA section, a processing
//! instruction or a DOCTYPE that goes over several lines. A [`FoldRange`] says which lines hide when it is folded. The commands work
//! on an [`Editor`] with the ranges of an analysis of *the text as it is*: ranges of older text name the wrong lines.

use xmlcraft_editor::Editor;
use xmlcraft_syntax::{SyntaxKind, SyntaxNode};
use xmlcraft_text::Buffer;

/// At most this many ranges are made for one text.
pub const MAX_FOLD_RANGES: usize = 50_000;

/// What a fold covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldKind {
    Element,
    Comment,
    Cdata,
    ProcessingInstruction,
    Doctype,
}

impl FoldKind {
    /// The kind as it is written in the tools' answers.
    pub fn name(self) -> &'static str {
        match self {
            FoldKind::Element => "element",
            FoldKind::Comment => "comment",
            FoldKind::Cdata => "cdata",
            FoldKind::ProcessingInstruction => "processing_instruction",
            FoldKind::Doctype => "doctype",
        }
    }
}

/// A stretch of lines that can be folded. All lines count from 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoldRange {
    /// The line that stays visible and stands for the others (for an element, the line its start tag ends on).
    pub first_line: usize,
    /// The last line that is hidden.
    pub last_line: usize,
    /// The line the thing ends on: `last_line`, or the line after it when an end tag stays visible.
    pub end_line: usize,
    /// How deep it is: 1 for the root element and what is outside it, 2 inside the root element, and so on.
    pub level: usize,
    pub kind: FoldKind,
}

impl FoldRange {
    /// Whether `line` is in it, the line that stands for it and a visible end tag included.
    pub fn contains(&self, line: usize) -> bool {
        self.first_line <= line && line <= self.end_line
    }
}

/// The ranges of the text `root` is the tree of, in the order they start, and whether there were more than [`MAX_FOLD_RANGES`] (the
/// rest are left out). Of ranges that start on the same line only the outer one is there: there is one mark to click on a line.
pub fn fold_ranges(root: &SyntaxNode, text: &str) -> (Vec<FoldRange>, bool) {
    use rowan::WalkEvent;
    let buffer = Buffer::from_text(text);
    let mut ranges: Vec<FoldRange> = Vec::new();
    let mut depth = 0usize;
    let mut truncated = false;
    for event in root.preorder() {
        let node = match &event {
            WalkEvent::Enter(node) | WalkEvent::Leave(node) => node,
        };
        let is_element = node.kind() == SyntaxKind::Element;
        match event {
            WalkEvent::Leave(_) => {
                if is_element {
                    depth = depth.saturating_sub(1);
                }
            }
            WalkEvent::Enter(node) => {
                if let Some(range) = range_of(&node, &buffer, depth + 1)
                    && ranges.last().is_none_or(|last| last.first_line < range.first_line)
                {
                    if ranges.len() >= MAX_FOLD_RANGES {
                        truncated = true;
                        break;
                    }
                    ranges.push(range);
                }
                if is_element {
                    depth += 1;
                }
            }
        }
    }
    (ranges, truncated)
}

/// The line the last character before `end` is on (a tag that is not closed may end in a character of several bytes).
fn line_of_last_char(buffer: &Buffer, end: usize) -> usize {
    buffer.line_of(buffer.prev_char(end))
}

fn range_of(node: &SyntaxNode, buffer: &Buffer, level: usize) -> Option<FoldRange> {
    let span = usize::from(node.text_range().start())..usize::from(node.text_range().end());
    let kind = match node.kind() {
        SyntaxKind::Element => FoldKind::Element,
        SyntaxKind::Comment => FoldKind::Comment,
        SyntaxKind::CData => FoldKind::Cdata,
        SyntaxKind::ProcessingInstruction => FoldKind::ProcessingInstruction,
        SyntaxKind::Doctype => FoldKind::Doctype,
        _ => return None,
    };
    if kind != FoldKind::Element {
        let (first, last) = (buffer.line_of(span.start), line_of_last_char(buffer, span.end));
        if first >= last {
            return None;
        }
        // Something after it on its last line would be hidden with that line: the fold stops a line short of it.
        let line = buffer.line_range(last)?;
        let rest_is_blank = buffer.slice(span.end.min(line.end)..line.end).ok()?.trim().is_empty();
        let hidden_to = if rest_is_blank { last } else { last - 1 };
        return (hidden_to > first).then_some(FoldRange { first_line: first, last_line: hidden_to, end_line: last, level, kind });
    }
    let start_tag = node.children().find(|n| n.kind() == SyntaxKind::StartTag)?;
    let end_tag = node.children().find(|n| n.kind() == SyntaxKind::EndTag)?;
    let tag_end = usize::from(start_tag.text_range().end());
    let end_start = usize::from(end_tag.text_range().start());
    let end_end = usize::from(end_tag.text_range().end());
    let first = line_of_last_char(buffer, tag_end);
    let end_line = buffer.line_of(end_start);
    if end_line <= first {
        return None;
    }
    // Lines of the end tag's line before it and after it: when it stands alone on its line, that line stays visible.
    let line = buffer.line_range(end_line)?;
    let before_is_blank = buffer.slice(line.start..end_start).ok()?.trim().is_empty();
    let after_is_blank = buffer.slice(end_end.min(line.end)..line.end).ok()?.trim().is_empty();
    let last = if before_is_blank && after_is_blank {
        end_line - 1
    } else if after_is_blank {
        end_line
    } else {
        return None; // something follows the end tag on its line: hiding the line would hide that too
    };
    (last > first).then_some(FoldRange { first_line: first, last_line: last, end_line, level, kind })
}

/// What to do to the folds of a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldAction {
    /// Fold the innermost range at the caret's line that is not folded already.
    Fold,
    /// Unfold the innermost folded range at the caret's line.
    Unfold,
    FoldAll,
    UnfoldAll,
    /// Fold the ranges of this level (1 is the root element) and nothing else.
    Level(usize),
}

/// Do `action` to `editor` with the `ranges` of its text, at the caret's line. Returns whether any fold changed.
pub fn apply(editor: &mut Editor, ranges: &[FoldRange], action: FoldAction) -> bool {
    let line = editor.buffer().line_of(editor.primary().head());
    apply_at(editor, ranges, action, line)
}

/// [`apply`] with the lines `Fold` and `Unfold` look at given (0-based), whatever the caret is on: a line a fold hides can be asked
/// about without opening the fold by putting the caret in it.
pub fn apply_at(editor: &mut Editor, ranges: &[FoldRange], action: FoldAction, line: usize) -> bool {
    // The ranges the caret's line is in, the innermost first (they start in the order of the text, so the later the inner).
    let here = || ranges.iter().rev().filter(move |r| r.contains(line));
    match action {
        FoldAction::Fold => match here().find(|r| editor.hidden_lines_after(r.first_line) == 0) {
            Some(range) => editor.fold_lines(range.first_line, range.last_line),
            None => false,
        },
        FoldAction::Unfold => match here().find(|r| editor.hidden_lines_after(r.first_line) > 0) {
            Some(range) => editor.unfold_line(range.first_line),
            None => false,
        },
        FoldAction::FoldAll => {
            let mut changed = false;
            for range in ranges {
                changed |= editor.fold_lines(range.first_line, range.last_line);
            }
            changed
        }
        FoldAction::UnfoldAll => editor.unfold_all(),
        FoldAction::Level(level) => {
            let mut changed = editor.unfold_all();
            for range in ranges.iter().filter(|r| r.level == level) {
                changed |= editor.fold_lines(range.first_line, range.last_line);
            }
            changed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xmlcraft_syntax::parse;

    fn ranges(text: &str) -> Vec<(usize, usize, usize, usize, FoldKind)> {
        fold_ranges(&parse(text).syntax(), text).0.into_iter().map(|r| (r.first_line, r.last_line, r.end_line, r.level, r.kind)).collect()
    }

    use FoldKind::*;

    #[test]
    fn an_element_with_content_on_lines_of_its_own_folds_down_to_its_end_tag() {
        let text = "<lib>\n  <book>\n    <t>x</t>\n    <u>y</u>\n  </book>\n  <br/>\n</lib>\n";
        assert_eq!(ranges(text), [(0, 5, 6, 1, Element), (1, 3, 4, 2, Element)]);
        // Content on one line is nothing to fold, nor an empty element or one that is on one line.
        assert_eq!(ranges("<a>\n</a>"), []);
        assert_eq!(
            ranges("<a><b>\n</b></a>"),
            [(0, 1, 1, 1, Element)],
            "`a` hides the line of `</b></a>`; `b` starts on the same line and is not offered"
        );
        assert_eq!(ranges("<a>x</a>\n<b/>"), []);
        assert_eq!(ranges("<a>\n  <b/>\n</a>"), [(0, 1, 2, 1, Element)]);
    }

    #[test]
    fn the_end_tag_with_something_before_it_is_hidden_with_the_content() {
        // Text before the end tag on its line: the line is part of what is hidden.
        assert_eq!(ranges("<a>\n  text</a>\n"), [(0, 1, 1, 1, Element)]);
        // Something after it on the line: not offered.
        assert_eq!(ranges("<a>\n  text</a><b/>\n"), []);
        assert_eq!(ranges("<r>\n<a>\n  x\n</a> <!--c-->\n</r>"), [(0, 3, 4, 1, Element)], "only the root: `a`'s end tag has a comment after it");
        // A start tag over several lines folds from the line it ends on.
        assert_eq!(ranges("<a\n  b=\"1\"\n  c=\"2\">\n  <x/>\n</a>"), [(2, 3, 4, 1, Element)]);
    }

    #[test]
    fn comments_cdata_instructions_and_doctypes_fold_through_their_last_line() {
        let text =
            "<?xml version=\"1.0\"?>\n<!DOCTYPE a [\n  <!ELEMENT a ANY>\n]>\n<!-- one\n two -->\n<a>\n  <![CDATA[x\n  y]]>\n  <?pi\n  data?>\n</a>\n";
        assert_eq!(
            ranges(text),
            [(1, 3, 3, 1, Doctype), (4, 5, 5, 1, Comment), (6, 10, 11, 1, Element), (7, 8, 8, 2, Cdata), (9, 10, 10, 2, ProcessingInstruction)]
        );
        assert_eq!(ranges("<!-- one line -->"), []);
    }

    #[test]
    fn something_after_a_comment_on_its_last_line_stays_visible() {
        assert_eq!(ranges("<!-- a\n b\n c --> <x/>\n"), [(0, 1, 2, 1, Comment)], "the line with `-->` and `<x/>` is shown");
        assert_eq!(ranges("<!-- a\n b --> <x/>\n"), [], "nothing is left to hide");
        assert_eq!(ranges("<!-- a\n b -->  \n"), [(0, 1, 1, 1, Comment)], "white space after it does not matter");
        assert_eq!(ranges("<?pi\n a\n?><x/>"), [(0, 1, 2, 1, ProcessingInstruction)]);
    }

    #[test]
    fn of_two_ranges_on_the_same_line_the_outer_one_is_there() {
        // `<a><b>` and the two are on line 0; `b` is inside `a` and ends before it.
        assert_eq!(ranges("<a><b>\n  x\n  y\n</b>\n</a>"), [(0, 3, 4, 1, Element)]);
    }

    #[test]
    fn a_node_that_ends_in_a_multi_byte_character_is_not_cut_in_the_middle() {
        // Found by the outline property test: a comment that is not closed ends in the middle of nothing but an `é`.
        assert_eq!(ranges("<!--\né"), [(0, 1, 1, 1, Comment)]);
        assert_eq!(ranges("<a é</a>\n<b>\n x\n</b>").len(), 1);
        assert_eq!(ranges("<?pi\n日"), [(0, 1, 1, 1, ProcessingInstruction)]);
    }

    #[test]
    fn a_broken_document_and_a_huge_one_are_handled() {
        // Unclosed elements have no end tag to fold to.
        assert_eq!(ranges("<a>\n  <b>\n    x\n"), []);
        assert_eq!(ranges("<!-- never closed\n\n"), [(0, 1, 1, 1, Comment)], "a comment that is not closed goes to the end of the text");
        let wide = format!("<r>\n{}</r>", "<i>\n x\n</i>\n".repeat(MAX_FOLD_RANGES + 10));
        let (found, truncated) = fold_ranges(&parse(&wide).syntax(), &wide);
        assert_eq!((found.len(), truncated), (MAX_FOLD_RANGES, true), "capped, and it says so");
        assert!(!fold_ranges(&parse("<a>\n x\n</a>").syntax(), "<a>\n x\n</a>").1);
        assert_eq!(FoldKind::ProcessingInstruction.name(), "processing_instruction");
    }

    // --- the commands ---------------------------------------------------------------------------------------------------------

    const DOC: &str = "<lib>\n  <book>\n    <t>x</t>\n    <u>y</u>\n  </book>\n  <book>\n    <t>z</t>\n  </book>\n</lib>\n";

    fn editor_with_caret(line: usize) -> (Editor, Vec<FoldRange>) {
        let mut e = Editor::new(DOC);
        let at = e.buffer().line_start(line).unwrap_or(0) + 1;
        e.set_selection(at, at);
        (e, fold_ranges(&parse(DOC).syntax(), DOC).0)
    }

    fn hidden(e: &Editor) -> Vec<usize> {
        (0..e.buffer().line_count()).filter(|l| e.is_line_hidden(*l)).collect()
    }

    #[test]
    fn fold_and_unfold_work_on_the_innermost_range_at_the_caret() {
        let (mut e, ranges) = editor_with_caret(2); // inside the first book
        assert!(apply(&mut e, &ranges, FoldAction::Fold));
        assert_eq!(hidden(&e), [2, 3], "the first book's content (its end tag line stays)");
        assert_eq!(e.buffer().line_of(e.primary().head()), 1, "the caret is on the line that stands for it now");
        // Folded already: Fold again folds the range around it.
        assert!(apply(&mut e, &ranges, FoldAction::Fold));
        assert_eq!(hidden(&e), [1, 2, 3, 4, 5, 6, 7], "everything in lib");
        // Unfold opens the fold the caret's line stands for, and then (the caret on the lib line) opens that one.
        assert!(apply(&mut e, &ranges, FoldAction::Unfold));
        assert_eq!(hidden(&e), [2, 3], "the outer one is open, the inner one stays folded");
        e.set_selection(e.buffer().line_start(1).unwrap_or(0), e.buffer().line_start(1).unwrap_or(0));
        assert!(apply(&mut e, &ranges, FoldAction::Unfold));
        assert!(hidden(&e).is_empty());
        assert!(!apply(&mut e, &ranges, FoldAction::Unfold), "nothing folded at the caret");
        // The caret on an end tag that stays visible: Unfold opens its element.
        let (mut e, ranges) = editor_with_caret(2);
        apply(&mut e, &ranges, FoldAction::Fold);
        let end_tag_line = e.buffer().line_start(4).unwrap_or(0);
        e.place_caret(end_tag_line + 3, false);
        assert!(apply(&mut e, &ranges, FoldAction::Unfold));
        assert!(hidden(&e).is_empty());
    }

    #[test]
    fn fold_all_unfold_all_and_levels() {
        let (mut e, ranges) = editor_with_caret(0);
        assert!(apply(&mut e, &ranges, FoldAction::FoldAll));
        assert_eq!(hidden(&e), [1, 2, 3, 4, 5, 6, 7], "lib folded, with the books inside it");
        assert!(apply(&mut e, &ranges, FoldAction::UnfoldAll));
        assert!(!apply(&mut e, &ranges, FoldAction::UnfoldAll));
        // Level 2: the books fold, lib stays open.
        assert!(apply(&mut e, &ranges, FoldAction::Level(2)));
        assert_eq!(hidden(&e), [2, 3, 6]);
        // Level 1: lib folds, nothing else (the books are not folded: they are hidden by it).
        assert!(apply(&mut e, &ranges, FoldAction::Level(1)));
        assert_eq!(e.folds().len(), 1);
        assert!(!apply(&mut e, &ranges, FoldAction::Level(7)) || e.folds().is_empty(), "a level there is nothing at opens everything");
        assert!(hidden(&e).is_empty());
    }
}
