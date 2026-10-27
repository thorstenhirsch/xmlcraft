//! Column selection: a rectangle of text, one selection on each line it crosses.

use crate::caret::Caret;
use crate::editor::Editor;
use crate::motion::{clamp, display_column, offset_at_column};

/// The most lines one column selection crosses: more would be a caret each for a document of millions of lines.
pub const MAX_COLUMN_LINES: usize = 20_000;

impl Editor {
    /// Select the rectangle between the offsets `anchor` and `head`: on every line from the one to the other, the text from the display
    /// column of one to the display column of the other (a tab fills the columns it spans). A line too short to reach the left edge gets
    /// a caret at its end, as a line that ends inside the rectangle gets a selection to its end. The caret of the head's line is the
    /// primary one, and a rectangle dragged to the left selects backwards. Lines hidden by a fold are left out, and at most
    /// [`MAX_COLUMN_LINES`] lines are taken, counted from the anchor's.
    pub fn select_columns(&mut self, anchor: usize, head: usize) {
        let tab = self.settings().tab_width;
        let (anchor, head) = (clamp(&self.buffer, anchor), clamp(&self.buffer, head));
        let (anchor_line, head_line) = (self.buffer.line_of(anchor), self.buffer.line_of(head));
        let (anchor_column, head_column) = (display_column(&self.buffer, anchor, tab), display_column(&self.buffer, head, tab));
        let (left, right) = (anchor_column.min(head_column), anchor_column.max(head_column));
        let backwards = head_column < anchor_column;
        // Towards the head from the anchor, as far as the cap allows.
        let reach = head_line.abs_diff(anchor_line).min(MAX_COLUMN_LINES - 1);
        let (first, last) = if head_line >= anchor_line { (anchor_line, anchor_line + reach) } else { (anchor_line - reach, anchor_line) };
        let end_line = if head_line >= anchor_line { last } else { first };
        let mut carets = Vec::new();
        let mut primary = 0;
        for line in first..=last {
            if self.fold_map.is_hidden(line) {
                continue;
            }
            let (start, end) = (offset_at_column(&self.buffer, line, left, tab), offset_at_column(&self.buffer, line, right, tab));
            if line == end_line {
                primary = carets.len();
            }
            carets.push(if backwards { Caret::new(end, start) } else { Caret::new(start, end) });
        }
        if carets.is_empty() {
            return;
        }
        self.set_carets_raw(carets, primary);
    }
}

#[cfg(test)]
mod tests {
    use crate::Editor;

    fn selections(e: &Editor) -> Vec<(usize, usize)> {
        e.carets().iter().map(|c| (c.sel.anchor, c.sel.head)).collect()
    }

    // Lines start at 0, 7, 10 and 11.
    const TEXT: &str = "abcdef\nab\n\nabcdefgh\n";

    #[test]
    fn a_rectangle_is_one_selection_per_line_and_short_lines_are_cut_at_their_end() {
        let mut e = Editor::new(TEXT);
        // From column 1 of line 0 to column 3 of line 3.
        e.select_columns(1, 14);
        assert_eq!(selections(&e), [(1, 3), (8, 9), (10, 10), (12, 14)]);
        assert_eq!(e.primary().head(), 14, "the head's line is the primary one");
    }

    #[test]
    fn dragging_to_the_left_selects_backwards_and_upwards_makes_the_first_line_the_primary() {
        let mut e = Editor::new(TEXT);
        // From column 4 of line 3 to column 1 of line 0.
        e.select_columns(15, 1);
        assert_eq!(selections(&e), [(4, 1), (9, 8), (10, 10), (15, 12)]);
        assert_eq!(e.primary_index(), 0, "the head is on the first line");
    }

    #[test]
    fn a_tab_fills_the_columns_it_spans() {
        let mut e = Editor::new("\tx\nabcdefgh\n");
        // Columns 4 to 6: after the tab (it is four wide), on the first line only the `x`.
        e.select_columns(1, 3 + 6);
        assert_eq!(selections(&e), [(1, 2), (7, 9)]);
        // From inside the tab's columns: the tab is not split, the offset is before or after it.
        e.select_columns(0, 3 + 2);
        assert!(selections(&e).iter().all(|&(a, h)| a <= h));
    }

    #[test]
    fn a_single_line_is_the_stream_selection_and_a_point_is_a_caret() {
        let mut e = Editor::new(TEXT);
        e.select_columns(1, 4);
        assert_eq!(selections(&e), [(1, 4)]);
        e.select_columns(2, 2);
        assert_eq!(selections(&e), [(2, 2)]);
    }

    #[test]
    fn typing_replaces_every_line_of_the_rectangle_in_one_undo_step() {
        let mut e = Editor::new("abcd\nabcd\nabcd\n");
        e.select_columns(1, 13);
        e.type_text("X");
        assert_eq!(e.text(), "aXd\naXd\naXd\n");
        assert!(e.undo());
        assert_eq!(e.text(), "abcd\nabcd\nabcd\n");
    }

    #[test]
    fn lines_hidden_by_a_fold_are_left_out_and_the_number_of_lines_is_capped() {
        let mut e = Editor::new("a\nb\nc\nd\n");
        assert!(e.fold_lines(0, 2));
        e.select_columns(0, 6);
        assert_eq!(e.carets().len(), 2, "lines 1 and 2 are hidden");
        let long = "x\n".repeat(super::MAX_COLUMN_LINES + 500);
        let mut e = Editor::new(&long);
        e.select_columns(0, long.len() - 2);
        assert_eq!(e.carets().len(), super::MAX_COLUMN_LINES);
        // Nowhere to go from a point past the end, and no panic for an anchor or head that is not in the text.
        e.select_columns(usize::MAX, usize::MAX);
        e.select_columns(3, usize::MAX);
        assert!(!e.carets().is_empty());
    }
}
