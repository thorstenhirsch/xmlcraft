//! Commands that know the structure of the XML: commenting lines out and in, and growing a selection by what the syntax tree says.

use std::ops::Range;

use rowan::{NodeOrToken, TextRange, TextSize};
use xmlcraft_syntax::{SyntaxKind, SyntaxNode, parse};
use xmlcraft_text::{Edit, EditKind, Selection};

use crate::Editor;
use crate::find::word_at;
use crate::motion::first_non_blank;

/// A text longer than this is not parsed to grow a selection.
const MAX_PARSED: usize = 8 * 1024 * 1024;

/// Why lines were not commented out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentRefused {
    /// The text has `--` in it, which a comment cannot hold.
    DoubleHyphen,
}

impl std::fmt::Display for CommentRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommentRefused::DoubleHyphen => f.write_str("the text has `--` in it, which a comment cannot hold"),
        }
    }
}

impl std::error::Error for CommentRefused {}

/// A selection that was grown, and what it was before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Expansion {
    /// The version of the text it happened in: after an edit the way back is not known.
    version: u64,
    before: Vec<Selection>,
    after: Vec<Selection>,
}

impl Editor {
    /// Comment out the lines the carets touch, or, when they are one comment already, bring them back: `<!--` and `-->` go around the
    /// text from the first character that is not blank on the first line to the end of the last line. Several carets are several
    /// blocks (lines two carets share are one). `Ok(false)` when there was nothing to do; nothing is changed when a block has `--`
    /// in it (it cannot go into a comment).
    pub fn toggle_comment(&mut self) -> Result<bool, CommentRefused> {
        let mut blocks: Vec<Range<usize>> = Vec::new();
        for caret in &self.carets {
            let first = self.buffer.line_of(caret.start());
            let mut last = self.buffer.line_of(caret.end());
            if last > first && self.buffer.line_start(last) == Some(caret.end()) {
                last -= 1;
            }
            let start = first_non_blank(&self.buffer, first);
            let end = self.buffer.line_range(last).map_or(start, |r| r.end);
            if start < end {
                blocks.push(start..end);
            }
        }
        blocks.sort_by_key(|b| (b.start, b.end));
        let mut joined: Vec<Range<usize>> = Vec::new();
        for block in blocks {
            match joined.last_mut() {
                Some(last) if block.start < last.end => last.end = last.end.max(block.end),
                _ => joined.push(block),
            }
        }
        let mut edits: Vec<Edit> = Vec::new();
        for block in &joined {
            let text = self.buffer.slice(block.clone()).unwrap_or_default();
            match comment_markers(&text) {
                Some((open, close)) => {
                    edits.push(Edit::delete(block.start..block.start + open));
                    edits.push(Edit::delete(block.end - close..block.end));
                }
                None if text.contains("--") => return Err(CommentRefused::DoubleHyphen),
                None => {
                    edits.push(Edit::insert(block.start, "<!-- "));
                    edits.push(Edit::insert(block.end, " -->"));
                }
            }
        }
        if edits.is_empty() {
            return Ok(false);
        }
        let carets = self.mapped_carets(&edits);
        Ok(self.commit(edits, carets, EditKind::Other))
    }

    /// Grow each selection to the smallest thing of the syntax tree that holds it and is bigger: the word, then the token, the value
    /// without its quotes and with them, the attribute, the tag, the content of the element, the element, and so on outwards. Returns
    /// whether anything grew (no, for a text too long to parse).
    pub fn expand_selection(&mut self) -> bool {
        let text = self.text();
        if text.len() > MAX_PARSED {
            return false;
        }
        let parsed = parse(&text);
        let root = parsed.syntax();
        if usize::from(root.text_range().end()) != text.len() {
            return false; // the parser gave up: there is no tree to go by
        }
        let before = self.selections();
        let mut grew = false;
        let after: Vec<Selection> = before
            .iter()
            .map(|sel| match larger_range(&root, &text, sel.start()..sel.end()) {
                Some(range) => {
                    grew = true;
                    Selection { anchor: range.start, head: range.end }
                }
                None => *sel,
            })
            .collect();
        if !grew {
            return false;
        }
        self.expanding = true;
        self.set_selections(&after);
        self.expanding = false;
        // `set_selections` may have moved carets (it keeps them apart): what is remembered is what they are now.
        let after = self.selections();
        self.expansions.push(Expansion { version: self.version(), before, after });
        true
    }

    /// Go back one step of [`Editor::expand_selection`]. Returns whether there was one to go back (not after the text changed, or the
    /// selection was changed in some other way).
    pub fn shrink_selection(&mut self) -> bool {
        let now = self.selections();
        let Some(top) = self.expansions.last() else { return false };
        if top.version != self.version() || top.after != now {
            self.expansions.clear();
            return false;
        }
        let before = top.before.clone();
        self.expansions.pop();
        self.expanding = true;
        self.set_selections(&before);
        self.expanding = false;
        true
    }
}

/// How much of `text` is the `<!--` and the `-->` of a comment that it is all of: the opening (with one space after it) and the closing
/// (with one space before it). `None` if it is not exactly one comment.
fn comment_markers(text: &str) -> Option<(usize, usize)> {
    let inner = text.strip_prefix("<!--")?.strip_suffix("-->")?;
    if inner.contains("--") || inner.ends_with('-') {
        return None; // two comments, or one that is not well-formed (`--->`): not something to take the markers off
    }
    let open = 4 + usize::from(inner.starts_with(' '));
    let rest = inner.get(open - 4..)?;
    let close = 3 + usize::from(rest.ends_with(' '));
    Some((open, close))
}

/// The smallest range of the tree that holds `sel` and is bigger than it.
fn larger_range(root: &SyntaxNode, text: &str, sel: Range<usize>) -> Option<Range<usize>> {
    let mut candidates: Vec<Range<usize>> = Vec::new();
    // The word the selection is in (a name, or a word of text) is the first step from a caret.
    if let Some(word) = word_at(text, sel.start) {
        candidates.push(word);
    }
    let (start, end) = (TextSize::try_from(sel.start).ok()?, TextSize::try_from(sel.end).ok()?);
    let mut node = match root.covering_element(TextRange::new(start, end)) {
        NodeOrToken::Node(node) => Some(node),
        NodeOrToken::Token(token) => {
            candidates.push(range_of(token.text_range()));
            token.parent()
        }
    };
    while let Some(current) = node {
        candidates.push(range_of(current.text_range()));
        match current.kind() {
            // The value without its quotes.
            SyntaxKind::AttrValue => {
                let r = range_of(current.text_range());
                if r.len() >= 2 {
                    candidates.push(r.start + 1..r.end - 1);
                }
            }
            // The content of an element: what is between its tags.
            SyntaxKind::Element => {
                let start_tag = current.children().find(|n| n.kind() == SyntaxKind::StartTag);
                let end_tag = current.children().find(|n| n.kind() == SyntaxKind::EndTag);
                if let (Some(s), Some(e)) = (start_tag, end_tag) {
                    let inner = range_of(s.text_range()).end..range_of(e.text_range()).start;
                    if inner.start < inner.end {
                        candidates.push(inner);
                    }
                }
            }
            _ => {}
        }
        node = current.parent();
    }
    candidates.into_iter().filter(|r| r.start <= sel.start && sel.end <= r.end && (r.start, r.end) != (sel.start, sel.end)).min_by_key(Range::len)
}

fn range_of(range: TextRange) -> Range<usize> {
    usize::from(range.start())..usize::from(range.end())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An editor on `marked` without its `|` (a caret) and `[` `]` (a selection from one to the other); see [`shown`].
    fn at(marked: &str) -> Editor {
        let mut text = String::new();
        let mut selections: Vec<Selection> = Vec::new();
        let mut open: Option<usize> = None;
        for c in marked.chars() {
            match c {
                '|' => selections.push(Selection::caret(text.len())),
                '[' => open = Some(text.len()),
                ']' => selections.push(Selection { anchor: open.take().unwrap_or(text.len()), head: text.len() }),
                c => text.push(c),
            }
        }
        let mut e = Editor::new(&text);
        e.set_selections(&selections);
        e
    }

    /// The text with `[` and `]` around each selection and `|` at each caret.
    fn shown(e: &Editor) -> String {
        let mut text = e.text();
        for c in e.carets().iter().rev() {
            if c.is_empty() {
                text.insert(c.end(), '|');
            } else {
                text.insert(c.end(), ']');
                text.insert(c.start(), '[');
            }
        }
        text
    }

    fn toggled(marked: &str) -> String {
        let mut e = at(marked);
        e.toggle_comment().expect("can be commented");
        shown(&e)
    }

    #[test]
    fn lines_are_commented_out_and_back_in() {
        assert_eq!(
            toggled("  <a>te|xt</a>"),
            "  <!-- <a>te|xt</a> -->",
            "from the first character that is not blank to the end of the line; the caret stays in its text"
        );
        assert_eq!(toggled("<a/>|"), "<!-- <a/> -->|");
        let mut e = at("  <a>text</a>|");
        e.toggle_comment().expect("ok");
        e.toggle_comment().expect("ok");
        assert_eq!(e.text(), "  <a>text</a>", "twice is nothing");
        assert_eq!(toggled("<!-- <a/> -->|"), "<a/>|");
        assert_eq!(toggled("  <!--<a/>-->|"), "  <a/>|", "without the spaces too");
        assert_eq!(toggled("<!---->|"), "|");
        assert_eq!(toggled("<!-- -->|"), "|");
    }

    #[test]
    fn a_selection_over_lines_is_one_comment() {
        assert_eq!(toggled("<a>\n  [<b/>\n  <c/>]\n</a>"), "<a>\n  [<!-- <b/>\n  <c/> -->]\n</a>");
        assert_eq!(toggled("<a>\n  <b/>\n  <c|/>\n</a>"), "<a>\n  <b/>\n  <!-- <c|/> -->\n</a>");
        // A selection that ends at the start of a line does not touch that line.
        assert_eq!(toggled("<a>\n[<b/>\n]<c/>\n</a>"), "<a>\n[<!-- <b/> -->\n]<c/>\n</a>");
        // The whole block is one comment already: the markers go.
        assert_eq!(toggled("[<!-- <b/>\n  <c/> -->]"), "[<b/>\n  <c/>]");
        // A caret on one line of a comment that goes over lines does not hold all of it: that line cannot be commented.
        let mut e = at("<!-- <b/>\n  <c/> -->|");
        assert_eq!(e.toggle_comment(), Err(CommentRefused::DoubleHyphen));
        // Several carets are several blocks, and a line two of them are on is one.
        assert_eq!(toggled("<a>|\n<b/>|\n<c/>|"), "<!-- <a> -->|\n<!-- <b/> -->|\n<!-- <c/> -->|");
        assert_eq!(toggled("<a>|x|\n<b/>"), "<!-- <a>|x -->|\n<b/>", "two carets on the same line: one block, each caret where its text is");
    }

    #[test]
    fn what_cannot_be_in_a_comment_is_refused_and_nothing_changes() {
        let mut e = at("<a>x</a>\n<!-- c --> <b/>|");
        assert_eq!(e.toggle_comment(), Err(CommentRefused::DoubleHyphen));
        assert_eq!(e.text(), "<a>x</a>\n<!-- c --> <b/>");
        let mut e = at("<!-- a --> <!-- b -->|");
        assert_eq!(e.toggle_comment(), Err(CommentRefused::DoubleHyphen), "two comments are not one");
        let mut e = at("<a x=\"--\"/>|");
        assert!(e.toggle_comment().is_err());
        // White space only, and an empty text: nothing to do.
        let mut e = at("  \n|  \n");
        assert_eq!(e.toggle_comment(), Ok(false));
        assert_eq!(Editor::new("").toggle_comment(), Ok(false));
        assert!(!CommentRefused::DoubleHyphen.to_string().is_empty());
    }

    #[test]
    fn a_comment_that_is_not_well_formed_is_not_taken_apart() {
        let mut e = at("<!--x--->|");
        assert_eq!(e.toggle_comment(), Err(CommentRefused::DoubleHyphen), "a body that ends in a hyphen cannot be closed");
        assert_eq!(e.text(), "<!--x--->");
        assert_eq!(toggled("<!--x- -->|"), "x-|", "a space before the markers is fine");
    }

    #[test]
    fn commenting_is_one_undo_step() {
        let mut e = at("<a>\n<b/>\n</a>");
        e.set_selection(0, e.text().len());
        e.toggle_comment().expect("ok");
        assert_eq!(e.text(), "<!-- <a>\n<b/>\n</a> -->");
        assert!(e.undo());
        assert_eq!(e.text(), "<a>\n<b/>\n</a>");
    }

    /// The selections after each expansion, starting from `marked`.
    fn expansions(marked: &str) -> Vec<String> {
        let mut e = at(marked);
        let mut out = vec![shown(&e)];
        while e.expand_selection() {
            out.push(shown(&e));
            assert!(out.len() < 20, "it ends");
        }
        out
    }

    #[test]
    fn a_selection_grows_by_what_the_tree_says() {
        assert_eq!(
            expansions("<r><a b=\"val|ue\">x</a></r>"),
            [
                "<r><a b=\"val|ue\">x</a></r>",
                "<r><a b=\"[value]\">x</a></r>",
                "<r><a b=[\"value\"]>x</a></r>",
                "<r><a [b=\"value\"]>x</a></r>",
                "<r>[<a b=\"value\">]x</a></r>",
                "<r>[<a b=\"value\">x</a>]</r>",
                "[<r><a b=\"value\">x</a></r>]",
            ]
        );
        assert_eq!(
            expansions("<a>hel|lo wor</a>"),
            ["<a>hel|lo wor</a>", "<a>[hello] wor</a>", "<a>[hello wor]</a>", "[<a>hello wor</a>]"],
            "the word, then the text of the element, then the element"
        );
        assert_eq!(expansions("<ab|c/>"), ["<ab|c/>", "<[abc]/>", "[<abc/>]"]);
        assert_eq!(
            expansions("<a><b>x</b><c|/></a>"),
            ["<a><b>x</b><c|/></a>", "<a><b>x</b><[c]/></a>", "<a><b>x</b>[<c/>]</a>", "<a>[<b>x</b><c/>]</a>", "[<a><b>x</b><c/></a>]"]
        );
        // From a name in an end tag, from a comment, from the middle of white space.
        assert_eq!(expansions("<a>x</a|>"), ["<a>x</a|>", "<a>x</[a]>", "<a>x[</a>]", "[<a>x</a>]"]);
        assert_eq!(
            expansions("<r><!-- no|te --></r>"),
            ["<r><!-- no|te --></r>", "<r><!-- [note] --></r>", "<r><!--[ note ]--></r>", "<r>[<!-- note -->]</r>", "[<r><!-- note --></r>]"]
        );
    }

    #[test]
    fn the_selections_of_several_carets_grow_each_by_itself() {
        let mut e = at("<a><b|>x</b><c|/></a>");
        assert!(e.expand_selection());
        assert_eq!(shown(&e), "<a><[b]>x</b><[c]/></a>");
        assert!(e.expand_selection());
        assert_eq!(shown(&e), "<a>[<b>]x</b>[<c/>]</a>");
    }

    #[test]
    fn shrinking_goes_back_the_way_it_grew_until_something_else_happens() {
        let mut e = at("<a>hel|lo</a>");
        assert!(!e.shrink_selection(), "nothing grew yet");
        e.expand_selection();
        assert_eq!(shown(&e), "<a>[hello]</a>", "the word");
        e.expand_selection();
        assert_eq!(shown(&e), "[<a>hello</a>]", "the element (its content is the word)");
        assert!(e.shrink_selection());
        assert_eq!(shown(&e), "<a>[hello]</a>");
        assert!(e.shrink_selection());
        assert_eq!(shown(&e), "<a>hel|lo</a>", "back at the caret");
        assert!(!e.shrink_selection());
        // After an edit, or a selection that was made another way, the way back is not known.
        e.expand_selection();
        e.set_selection(0, 0);
        assert!(!e.shrink_selection());
        // Going somewhere else and coming back to the very selection the expansion made does not make the way back its own again.
        let mut e = at("<a>hel|lo</a>");
        e.expand_selection();
        let grown = e.selections();
        e.set_selection(0, 0);
        e.set_selections(&grown);
        assert!(!e.shrink_selection());
        let mut e = at("<a>hel|lo</a>");
        e.expand_selection();
        e.type_text("X");
        assert!(!e.shrink_selection());
    }

    #[test]
    fn nothing_grows_past_the_whole_text_or_in_a_text_too_long_to_parse() {
        let mut e = at("[<a/>]");
        assert!(!e.expand_selection());
        assert_eq!(shown(&e), "[<a/>]");
        let mut e = Editor::new(&format!("<a>{}</a>", "x".repeat(MAX_PARSED)));
        e.set_selection(5, 5);
        assert!(!e.expand_selection(), "too long to parse");
        // A text that is not well-formed has a tree too, and the selection grows in it.
        let mut e = at("<a><b|>");
        assert!(e.expand_selection());
        assert_eq!(shown(&e), "<a><[b]>");
    }
}
