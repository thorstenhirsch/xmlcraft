//! Folding: stretches of lines that are hidden, so that the line before them stands for all of them.
//!
//! A [`Fold`] is a stretch of the text, from the end of the text of the line that stays visible to the end of the text of the last line
//! it hides (the line break after the visible line and every line break up to there are in it). Folds are positions in the text, so
//! they move with it: an edit before a fold shifts it, and one that touches it opens it. The view works in *rows*, the lines that are
//! not hidden; [`FoldMap`] turns lines into rows and back.

use std::ops::Range;

use xmlcraft_text::{Buffer, Edit};

use crate::Editor;
use crate::caret::Caret;

/// A hidden stretch of the text: the positions after `hidden.start` up to and including `hidden.end` are not shown. Both ends are the
/// ends of the text of a line (before its line break).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fold {
    pub hidden: Range<usize>,
    /// How many more positions after `hidden.end` the fold hides: those inside the line break that ends the last hidden line (one for
    /// CRLF: between the CR and the LF), none for LF and for a last line without a break.
    pub tail: usize,
}

/// Which lines are hidden, and what row each of the others is on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FoldMap {
    /// The hidden stretches of lines (`first..end`), ascending and apart.
    lines: Vec<Range<usize>>,
    /// `before[i]` is how many lines are hidden by `lines[..i]`.
    before: Vec<usize>,
    /// How many rows come before `lines[i]`: the rows of the lines above it.
    rows_before: Vec<usize>,
    /// The hidden stretches of text as (start, end]: positions after `start` up to and including `end`, ascending and apart.
    zones: Vec<(usize, usize)>,
    /// How many lines the text has.
    line_count: usize,
}

impl FoldMap {
    /// The map of `folds` in `buffer`. A fold that is not a stretch of whole lines in the text (it was left behind by something that
    /// changed the text without telling the editor) is ignored.
    pub fn new(buffer: &Buffer, folds: &[Fold]) -> Self {
        let line_count = buffer.line_count();
        let mut lines: Vec<Range<usize>> = Vec::new();
        let mut zones: Vec<(usize, usize)> = Vec::new();
        for fold in folds {
            let (start, end) = (fold.hidden.start, fold.hidden.end);
            // Both ends are the ends of the text of a line: one that is not is something else, and hides the wrong lines.
            let ends_a_line = |offset: usize| buffer.line_range(buffer.line_of(offset)).is_some_and(|r| r.end == offset);
            if start >= end || end > buffer.len() || !ends_a_line(start) || !ends_a_line(end) {
                continue;
            }
            let (first, last) = (buffer.line_of(start) + 1, buffer.line_of(end));
            if first > last || last >= line_count {
                continue;
            }
            lines.push(first..last + 1);
            zones.push(zone_of_fold(buffer, fold));
        }
        lines.sort_by_key(|r| (r.start, r.end));
        zones.sort_unstable();
        let lines = merge(lines, |r| r.start, |r| r.end, |a, b| a.start..a.end.max(b.end));
        let zones = merge(zones, |z| z.0, |z| z.1, |a, b| (a.0, a.1.max(b.1)));
        let mut before = Vec::with_capacity(lines.len() + 1);
        let mut rows_before = Vec::with_capacity(lines.len());
        let mut hidden = 0;
        for range in &lines {
            before.push(hidden);
            rows_before.push(range.start - hidden);
            hidden += range.len();
        }
        before.push(hidden);
        Self { lines, before, rows_before, zones, line_count }
    }

    /// Whether nothing is hidden.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// How many rows there are: the lines that are not hidden.
    pub fn rows(&self) -> usize {
        self.line_count.saturating_sub(self.before.last().copied().unwrap_or(0))
    }

    /// Whether `line` is hidden.
    pub fn is_hidden(&self, line: usize) -> bool {
        let at = self.lines.partition_point(|r| r.start <= line);
        at > 0 && self.lines.get(at - 1).is_some_and(|r| line < r.end)
    }

    /// The row `line` is on; a hidden line is on the row of the line that stands for it.
    pub fn row_of(&self, line: usize) -> usize {
        let at = self.lines.partition_point(|r| r.start <= line);
        // A line inside a hidden stretch is on the row of the line above the stretch.
        let (line, at) = match at.checked_sub(1).and_then(|i| self.lines.get(i).map(|r| (i, r))) {
            Some((i, r)) if line < r.end => (r.start.saturating_sub(1), i),
            _ => (line, at),
        };
        line.saturating_sub(self.before.get(at).copied().unwrap_or(0))
    }

    /// The line that is on `row`. A row past the last is the line after the last row's.
    pub fn line_of(&self, row: usize) -> usize {
        let at = self.rows_before.partition_point(|&rows| rows <= row);
        row.saturating_add(self.before.get(at).copied().unwrap_or(0))
    }

    /// How many lines are hidden right after `line`: more than none for the line a fold starts on.
    pub fn hidden_after(&self, line: usize) -> usize {
        let next = line.saturating_add(1);
        let at = self.lines.partition_point(|r| r.start <= next);
        // (the last possible line has no line after it to hide)
        at.checked_sub(1).and_then(|i| self.lines.get(i)).filter(|r| r.start == next && line != usize::MAX).map_or(0, Range::len)
    }

    /// The hidden stretch of text that has `offset` in it, as (start, end]: the positions after `start` up to `end`.
    pub fn zone_of(&self, offset: usize) -> Option<(usize, usize)> {
        let at = self.zones.partition_point(|z| z.0 < offset);
        at.checked_sub(1).and_then(|i| self.zones.get(i)).copied().filter(|z| offset <= z.1)
    }
}

/// The positions a fold hides, as (start, end]: after the end of the line that stands for it up to the start of the line after the
/// last one it hides, less one (so that is every position in the line break of the last hidden line too, also the one in the middle of
/// a CRLF).
fn zone_of_fold(buffer: &Buffer, fold: &Fold) -> (usize, usize) {
    let (start, end) = (fold.hidden.start, fold.hidden.end);
    let after = buffer.line_start(buffer.line_of(end) + 1).map_or(end, |next| next.saturating_sub(1));
    (start, after.max(end))
}

/// Join the ranges that overlap or touch. `items` are sorted by start.
fn merge<T: Clone>(items: Vec<T>, start: impl Fn(&T) -> usize, end: impl Fn(&T) -> usize, join: impl Fn(&T, &T) -> T) -> Vec<T> {
    let mut out: Vec<T> = Vec::with_capacity(items.len());
    for item in items {
        match out.last_mut() {
            Some(last) if start(&item) <= end(last) => *last = join(last, &item),
            _ => out.push(item),
        }
    }
    out
}

impl Editor {
    /// The folds, in the order of the text.
    pub fn folds(&self) -> &[Fold] {
        &self.folds
    }

    /// How many rows the view has: the lines that are not hidden.
    pub fn visible_rows(&self) -> usize {
        if self.fold_map.is_empty() { self.buffer.line_count() } else { self.fold_map.rows() }
    }

    /// The row `line` is on.
    pub fn row_of_line(&self, line: usize) -> usize {
        if self.fold_map.is_empty() { line } else { self.fold_map.row_of(line) }
    }

    /// The line that is on `row`.
    pub fn line_of_row(&self, row: usize) -> usize {
        if self.fold_map.is_empty() { row } else { self.fold_map.line_of(row) }
    }

    pub fn is_line_hidden(&self, line: usize) -> bool {
        !self.fold_map.is_empty() && self.fold_map.is_hidden(line)
    }

    /// How many lines a fold that starts on `line` hides (none when `line` has no fold).
    pub fn hidden_lines_after(&self, line: usize) -> usize {
        if self.fold_map.is_empty() { 0 } else { self.fold_map.hidden_after(line) }
    }

    /// Hide the lines after `first` up to and including `last`, so that `first` stands for them. The carets that were in them go to the
    /// end of `first`. Returns whether anything was folded: not when the lines are not all in the text, there is nothing to hide, or this
    /// is a fold already.
    pub fn fold_lines(&mut self, first: usize, last: usize) -> bool {
        self.fold_many(&[(first, last)])
    }

    /// [`Editor::fold_lines`] for many (first line, last line) pairs at once: the folds are put in order and the map is made once, so
    /// folding everything in a text with tens of thousands of ranges is not tens of thousands of times the work of folding one.
    pub fn fold_many(&mut self, ranges: &[(usize, usize)]) -> bool {
        let line_count = self.buffer.line_count();
        let mut known: std::collections::HashSet<(usize, usize)> = self.folds.iter().map(|f| (f.hidden.start, f.hidden.end)).collect();
        let mut added: Vec<Fold> = Vec::new();
        for &(first, last) in ranges {
            if first >= last || last >= line_count {
                continue;
            }
            let (Some(start), Some(end)) = (self.buffer.line_range(first).map(|r| r.end), self.buffer.line_range(last).map(|r| r.end)) else {
                continue;
            };
            if !known.insert((start, end)) {
                continue;
            }
            let mut fold = Fold { hidden: start..end, tail: 0 };
            fold.tail = zone_of_fold(&self.buffer, &fold).1 - end;
            added.push(fold);
        }
        if added.is_empty() {
            return false;
        }
        // Only what the new folds hide is moved: a caret in a fold that was there before is where its owner put it.
        let mut new_zones: Vec<(usize, usize)> = added.iter().map(|f| zone_of_fold(&self.buffer, f)).collect();
        new_zones.sort_unstable();
        let new_zones = merge(new_zones, |z| z.0, |z| z.1, |a, b| (a.0, a.1.max(b.1)));
        self.folds.extend(added);
        self.folds.sort_by_key(|f| (f.hidden.start, f.hidden.end));
        self.rebuild_fold_map();
        // What was in the hidden lines goes to the end of the line that stands for them.
        let out = |offset: usize| {
            let at = new_zones.partition_point(|z| z.0 < offset);
            at.checked_sub(1).and_then(|i| new_zones.get(i)).filter(|z| offset <= z.1).map_or(offset, |z| z.0)
        };
        let moved: Vec<Caret> = self.carets.iter().map(|c| Caret::new(out(c.sel.anchor), out(c.sel.head))).collect();
        self.set_carets_raw(moved, self.primary);
        true
    }

    /// Show what the folds that start on `line` hide. Returns whether there was one.
    pub fn unfold_line(&mut self, line: usize) -> bool {
        let Some(end_of_line) = self.buffer.line_range(line).map(|r| r.end) else { return false };
        let before = self.folds.len();
        self.folds.retain(|f| f.hidden.start != end_of_line);
        let changed = self.folds.len() != before;
        if changed {
            self.rebuild_fold_map();
        }
        changed
    }

    /// Show everything. Returns whether anything was hidden.
    pub fn unfold_all(&mut self) -> bool {
        let changed = !self.folds.is_empty();
        self.folds.clear();
        self.rebuild_fold_map();
        changed
    }

    pub(crate) fn rebuild_fold_map(&mut self) {
        self.fold_map = if self.folds.is_empty() { FoldMap::default() } else { FoldMap::new(&self.buffer, &self.folds) };
    }

    /// Carry the folds through `edits`, which were made one after the other (each in the coordinates of the text just before it): a
    /// fold that an edit touches is opened; one after it shifts.
    pub(crate) fn remap_folds(&mut self, edits: &[Edit]) {
        if self.folds.is_empty() {
            return;
        }
        for edit in edits {
            let (s, e, delta) = (edit.range.start, edit.range.end, edit.delta());
            let breaks_line = edit.text.contains(['\n', '\r']);
            self.folds.retain_mut(|fold| {
                let (start, end) = (fold.hidden.start, fold.hidden.end);
                if e < start || (e == start && !breaks_line) {
                    fold.hidden = start.saturating_add_signed(delta)..end.saturating_add_signed(delta);
                    true
                } else {
                    // After the fold (past what it hides, the middle of a CRLF included), and not touching it.
                    s > end.saturating_add(fold.tail)
                }
            });
        }
        self.rebuild_fold_map();
    }

    /// Open the folds that hide a caret (a jump to a match, a message or the outline lands in one).
    pub(crate) fn reveal_carets(&mut self) {
        if self.fold_map.is_empty() {
            return;
        }
        let heads: Vec<usize> = self.carets.iter().map(|c| c.head()).collect();
        let before = self.folds.len();
        let buffer = &self.buffer;
        self.folds.retain(|f| {
            let (start, end) = zone_of_fold(buffer, f);
            !heads.iter().any(|&h| start < h && h <= end)
        });
        if self.folds.len() != before {
            self.rebuild_fold_map();
        }
    }

    /// Where a caret that a movement left in a hidden stretch goes instead: past the stretch when it moved forward, to the end of the
    /// line that stands for it when it moved back (or there is nothing after).
    pub(crate) fn out_of_folds(&self, offset: usize, forward: bool) -> usize {
        let Some((start, end)) = self.fold_map.zone_of(offset) else { return offset };
        if forward {
            let after = self.buffer.line_start(self.buffer.line_of(end) + 1);
            if let Some(after) = after {
                return after;
            }
        }
        start
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// A text of `n` lines, `l0`, `l1`, … (each 2 bytes of name, a digit and a line break; the last without a line break).
    fn lines(n: usize) -> String {
        (0..n).map(|i| format!("l{}", i % 10)).collect::<Vec<_>>().join("\n")
    }

    /// The fold that hides the lines after `first` up to and including `last`.
    fn fold(buffer: &Buffer, first: usize, last: usize) -> Fold {
        let (start, end) = (buffer.line_range(first).map(|r| r.end), buffer.line_range(last).map(|r| r.end));
        Fold { hidden: start.unwrap_or(0)..end.unwrap_or(0), tail: 0 }
    }

    #[test]
    fn rows_and_lines_with_folds() {
        let buffer = Buffer::from_text(&lines(10));
        // Lines 3 to 5 hidden (a fold on line 2), and lines 8 and 9 (a fold on line 7).
        let map = FoldMap::new(&buffer, &[fold(&buffer, 2, 5), fold(&buffer, 7, 9)]);
        assert_eq!(map.rows(), 5, "lines 0, 1, 2, 6 and 7");
        let visible: Vec<usize> = (0..map.rows()).map(|row| map.line_of(row)).collect();
        assert_eq!(visible, [0, 1, 2, 6, 7]);
        let rows: Vec<usize> = (0..10).map(|line| map.row_of(line)).collect();
        assert_eq!(rows, [0, 1, 2, 2, 2, 2, 3, 4, 4, 4], "a hidden line is on the row of the line that stands for it");
        assert_eq!((0..10).filter(|l| map.is_hidden(*l)).collect::<Vec<_>>(), [3, 4, 5, 8, 9]);
        assert_eq!((0..10).map(|l| map.hidden_after(l)).collect::<Vec<_>>(), [0, 0, 3, 0, 0, 0, 0, 2, 0, 0]);
        // Positions: after the end of line 2's text up to the end of line 5's text are hidden.
        let (start, end) = (buffer.line_range(2).map_or(0, |r| r.end), buffer.line_range(5).map_or(0, |r| r.end));
        assert_eq!(map.zone_of(start), None, "the end of the visible line is visible");
        assert_eq!(map.zone_of(start + 1), Some((start, end)));
        assert_eq!(map.zone_of(end), Some((start, end)));
        assert_eq!(map.zone_of(end + 1), None, "the start of the next line");
    }

    #[test]
    fn nested_and_overlapping_folds_join_and_odd_ones_are_ignored() {
        let buffer = Buffer::from_text(&lines(10));
        // An inner fold inside an outer one, and one that touches another.
        let map = FoldMap::new(&buffer, &[fold(&buffer, 1, 8), fold(&buffer, 3, 5), fold(&buffer, 8, 9)]);
        assert_eq!((0..10).filter(|l| map.is_hidden(*l)).collect::<Vec<_>>(), [2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(map.rows(), 2);
        // Nothing to hide, past the end, backwards, and ends that are not the ends of lines (bytes 1 and 7 are in the middle of lines).
        #[allow(clippy::reversed_empty_ranges)] // a fold that was left backwards is what is being tried
        let odd = [
            Fold { hidden: 4..4, tail: 0 },
            Fold { hidden: 0..9999, tail: 0 },
            Fold { hidden: 9..3, tail: 0 },
            Fold { hidden: 1..8, tail: 0 },
            Fold { hidden: 2..7, tail: 0 },
        ];
        assert!(FoldMap::new(&buffer, &odd).is_empty());
        assert_eq!(FoldMap::new(&buffer, &[fold(&buffer, 0, 1)]).rows(), 9, "a good one is not");
        // A past-the-end query is answered, not overflowed.
        let map = FoldMap::new(&buffer, &[fold(&buffer, 0, 1)]);
        assert_eq!((map.hidden_after(usize::MAX), map.line_of(usize::MAX) > 0, map.row_of(usize::MAX) > 0), (0, true, true));
        assert!(FoldMap::default().is_empty());
        assert_eq!(FoldMap::default().rows(), 0);
    }

    proptest! {
        /// The map agrees with listing the lines that are not hidden.
        #[test]
        fn the_map_agrees_with_a_list_of_the_visible_lines(
            count in 1usize..40,
            folds in proptest::collection::vec((0usize..40, 1usize..10), 0..8),
        ) {
            let buffer = Buffer::from_text(&lines(count));
            let folds: Vec<Fold> = folds.into_iter().filter(|(first, len)| first + len < count).map(|(first, len)| fold(&buffer, first, first + len)).collect();
            let map = FoldMap::new(&buffer, &folds);
            let mut hidden = vec![false; count];
            for f in &folds {
                for line in buffer.line_of(f.hidden.start) + 1..=buffer.line_of(f.hidden.end) {
                    hidden[line] = true;
                }
            }
            let visible: Vec<usize> = (0..count).filter(|l| !hidden[*l]).collect();
            if folds.is_empty() {
                prop_assert!(map.is_empty());
            } else {
                prop_assert_eq!(map.rows(), visible.len());
                for (row, line) in visible.iter().enumerate() {
                    prop_assert_eq!(map.line_of(row), *line);
                    prop_assert_eq!(map.row_of(*line), row);
                }
                for (line, is_hidden) in hidden.iter().enumerate() {
                    prop_assert_eq!(map.is_hidden(line), *is_hidden);
                    // Every line is on the row of the last visible line at or above it.
                    let above = visible.iter().rposition(|v| *v <= line).unwrap_or(0);
                    prop_assert_eq!(map.row_of(line), above);
                }
            }
        }
    }

    // --- the editor with folds ------------------------------------------------------------------------------------------------

    use crate::Motion;

    /// An editor on eight lines `a0` … `a7`: line `i` is bytes `3 * i` to `3 * i + 2`, followed by a line break.
    fn eight() -> Editor {
        Editor::new(&(0..8).map(|i| format!("a{i}")).collect::<Vec<_>>().join("\n"))
    }

    fn head(e: &Editor) -> usize {
        e.primary().head()
    }

    #[test]
    fn folding_hides_lines_and_moves_the_carets_that_were_in_them() {
        let mut e = eight();
        e.set_selection(7, 7); // in line 2
        assert!(e.fold_lines(1, 3), "lines 2 and 3 are hidden, line 1 stands for them");
        assert_eq!((e.visible_rows(), e.is_line_hidden(2), e.is_line_hidden(3), e.is_line_hidden(1)), (6, true, true, false));
        assert_eq!(head(&e), 5, "the end of line 1, which stands for them");
        assert_eq!((e.row_of_line(4), e.line_of_row(2), e.hidden_lines_after(1)), (2, 4, 2));
        // Nothing to hide, a fold that is there already, and lines past the text.
        assert!(!e.fold_lines(1, 3) && !e.fold_lines(4, 4) && !e.fold_lines(5, 4) && !e.fold_lines(6, 99));
        assert_eq!(e.folds().len(), 1);
        assert!(e.unfold_line(1));
        assert_eq!((e.visible_rows(), e.folds().len(), e.hidden_lines_after(1)), (8, 0, 0));
        assert!(!e.unfold_line(1));
        e.fold_lines(0, 2);
        e.fold_lines(4, 6);
        assert!(e.unfold_all());
        assert_eq!(e.visible_rows(), 8);
        assert!(!e.unfold_all());
    }

    #[test]
    fn the_caret_moves_over_a_fold_not_into_it() {
        let mut e = eight();
        e.fold_lines(1, 3); // rows: lines 0, 1, 4, 5, 6, 7
        // Down from the line that stands for the fold goes to the line after it, and up comes back.
        e.set_selection(4, 4); // line 1, column 1
        e.move_carets(Motion::Down, false);
        assert_eq!(head(&e), 13, "line 4 starts at 12; the column is kept");
        e.move_carets(Motion::Up, false);
        assert_eq!(head(&e), 4);
        // Right at the end of that line goes to the start of the next line shown, and Left comes back.
        e.set_selection(5, 5);
        e.move_carets(Motion::Right, false);
        assert_eq!(head(&e), 12);
        e.move_carets(Motion::Left, false);
        assert_eq!(head(&e), 5);
        // A selection extended over it keeps its anchor and ends outside.
        e.set_selection(0, 0);
        e.move_carets(Motion::Down, true);
        e.move_carets(Motion::Down, true);
        assert_eq!((e.primary().sel.anchor, head(&e)), (0, 12));
        // Page movements count rows, and the end of the text is the end of the last row.
        e.set_selection(0, 0);
        e.move_carets(Motion::PageDown(2), false);
        assert_eq!(e.buffer().line_of(head(&e)), 4);
        e.move_carets(Motion::PageDown(99), false);
        assert_eq!(head(&e), e.buffer().len());
        // A fold at the end of the text: the end of the text is hidden, so the caret stays on the line that stands for it.
        let mut e = eight();
        e.fold_lines(5, 7);
        e.set_selection(0, 0);
        e.move_carets(Motion::DocEnd, false);
        assert_eq!(head(&e), 17, "the end of line 5");
        e.set_selection(17, 17);
        e.move_carets(Motion::Right, false);
        assert_eq!(head(&e), 17, "nothing after the fold to go to");
    }

    #[test]
    fn typing_at_the_end_of_a_fold_line_keeps_the_fold_unless_it_breaks_the_line() {
        let mut e = eight();
        e.fold_lines(1, 3);
        e.set_selection(5, 5);
        e.type_text("X");
        assert_eq!((e.folds().len(), e.folds()[0].hidden.clone(), head(&e)), (1, 6..12, 6), "the fold moved with the text after it");
        assert_eq!(e.visible_rows(), 6);
        e.insert_newline();
        assert_eq!((e.folds().len(), e.visible_rows()), (0, 9), "Enter there opens the fold, then adds a line");
    }

    #[test]
    fn an_edit_before_a_fold_moves_it_one_inside_or_touching_opens_it_and_one_after_leaves_it() {
        let make = || {
            let mut e = eight();
            e.fold_lines(1, 3); // hidden (5, 11]
            e
        };
        let mut e = make();
        e.apply_edits(&[Edit::insert(0, "zz\n")]);
        assert_eq!((e.folds()[0].hidden.clone(), e.hidden_lines_after(2)), (8..14, 2), "a line above: the fold is on line 2 now");
        let mut e = make();
        e.apply_edits(&[Edit::insert(13, "Q")]);
        assert_eq!(e.folds()[0].hidden, 5..11, "after the fold");
        for edit in [Edit::replace(6..8, "Q"), Edit::delete(4..7), Edit::insert(11, "Q"), Edit::delete(0..16), Edit::replace(5..6, "")] {
            let mut e = make();
            e.apply_edits(std::slice::from_ref(&edit));
            assert!(e.folds().is_empty(), "{edit:?} touches the fold");
            assert_eq!(e.visible_rows(), e.buffer().line_count());
        }
        // Deleting the end of the line that stands for the fold only moves it.
        let mut e = make();
        e.apply_edits(&[Edit::delete(4..5)]);
        assert_eq!(e.folds()[0].hidden, 4..10);
    }

    #[test]
    fn undo_and_redo_carry_the_folds_along() {
        let mut e = eight();
        e.fold_lines(1, 3);
        e.set_selection(2, 2);
        e.type_text("Z");
        assert_eq!(e.folds()[0].hidden, 6..12);
        e.undo();
        assert_eq!((e.folds()[0].hidden.clone(), e.text().len()), (5..11, 23));
        e.redo();
        assert_eq!(e.folds()[0].hidden, 6..12);
        // Undoing an edit that was made inside what is hidden now opens the fold, and the caret is where it was.
        let mut e = eight();
        e.set_selection(7, 7);
        e.type_text("Q");
        e.fold_lines(1, 3);
        assert_eq!(head(&e), 5);
        e.undo();
        assert_eq!((e.folds().len(), e.text().len(), head(&e)), (0, 23, 7));
    }

    #[test]
    fn a_caret_in_the_line_break_after_the_last_hidden_line_is_in_the_fold() {
        // Found by the model property test: between the CR and the LF that end the last hidden line is a hidden position too.
        let mut e = Editor::new("a\r\nb\r\nc\r\nd");
        e.fold_lines(0, 1); // line 1 (`b`) is hidden: bytes 1..=4, its break is 4..6
        assert_eq!(e.folds()[0].hidden, 1..4);
        e.set_selection(5, 5);
        assert!(e.folds().is_empty(), "position 5 is between the CR and the LF of the break after `b`");
        let mut e = Editor::new("a\r\nb\r\nc");
        e.fold_lines(0, 1);
        e.set_selection(6, 6);
        assert_eq!(e.folds().len(), 1, "the start of the next line is visible");
        e.move_carets(Motion::Left, false);
        assert_eq!(head(&e), 1, "back from there is the end of the line that stands for the fold");
    }

    #[test]
    fn the_middle_of_the_last_line_break_belongs_to_the_fold_when_folding_and_when_editing() {
        // Found in review: a caret between the CR and the LF after the last hidden line was not moved out when folding (the fold opened
        // again at once), and an edit there was taken for one after the fold.
        let mut e = Editor::new("a\r\nb\r\nc\r\nd");
        e.set_selection(5, 5); // between the CR and the LF of the break after `b`
        assert!(e.fold_lines(0, 1));
        assert_eq!((e.folds().len(), e.folds()[0].tail, head(&e)), (1, 1, 1), "the fold stays, the caret went to the end of line 0");
        e.apply_edits(&[Edit::insert(5, "X")]);
        assert!(e.folds().is_empty(), "an edit inside the hidden break opens the fold");
        // With LF there is no middle.
        let mut e = Editor::new("a\nb\nc");
        e.fold_lines(0, 1);
        assert_eq!(e.folds()[0].tail, 0);
        e.apply_edits(&[Edit::insert(4, "X")]);
        assert_eq!(e.folds().len(), 1, "the start of line 2 is after the fold");
    }

    #[test]
    fn folding_moves_only_what_the_new_fold_hides() {
        // A selection that goes from inside an old fold to outside it keeps its anchor when an unrelated fold is added.
        let mut e = Editor::new("a0\na1\na2\na3\na4\na5\na6\na7");
        e.fold_lines(0, 2); // hides lines 1 and 2: the positions 3 to 8
        e.set_selection(4, 14); // the anchor is inside the fold, the head (line 4) is not
        assert_eq!(e.folds().len(), 1, "the head is out of the fold: it stays");
        assert!(e.fold_lines(4, 5));
        assert_eq!(e.folds().len(), 2);
        assert_eq!((e.primary().sel.anchor, e.primary().sel.head), (4, 14), "the unrelated fold moved nothing");
        // What the new fold hides is moved, as always.
        assert!(e.fold_lines(3, 6));
        assert_eq!(e.primary().sel.head, 11, "the head was in the lines it hides");
    }

    #[test]
    fn many_folds_are_made_in_one_go() {
        // Review: one rebuild of the map per fold made folding everything in a big text take quadratic time.
        let text = (0..60_000).map(|i| format!("l{i}")).collect::<Vec<_>>().join("\n");
        let mut e = Editor::new(&text);
        let ranges: Vec<(usize, usize)> = (0..20_000).map(|i| (3 * i, 3 * i + 2)).collect();
        let started = std::time::Instant::now();
        assert!(e.fold_many(&ranges));
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "took {:?}", started.elapsed());
        assert_eq!((e.folds().len(), e.visible_rows()), (20_000, 60_000 - 40_000));
        assert!(!e.fold_many(&ranges), "folded already: nothing changed");
        // Some that are not there, backwards, and past the text are skipped; the good ones are made.
        let mut e = Editor::new("a\nb\nc\nd\ne");
        assert!(e.fold_many(&[(3, 3), (9, 12), (0, 1), (2, 4)]));
        assert_eq!(e.folds().len(), 2);
        assert_eq!((e.is_line_hidden(1), e.is_line_hidden(3), e.is_line_hidden(4)), (true, true, true));
    }

    #[test]
    fn a_jump_into_a_fold_opens_it() {
        let mut e = eight();
        e.fold_lines(1, 3);
        e.set_selection(7, 8);
        assert!(e.folds().is_empty());
        let mut e = eight();
        e.fold_lines(1, 3);
        e.place_caret(10, false);
        assert!(e.folds().is_empty(), "a click that lands in one (the view does not allow it, a command might)");
        // A selection that only has its far end in a fold: the head decides.
        let mut e = eight();
        e.fold_lines(1, 3);
        e.set_selection(0, 8);
        assert!(e.folds().is_empty());
        // Inside two folds, both open.
        let mut e = eight();
        e.fold_lines(0, 5);
        e.fold_lines(2, 4);
        e.set_selection(10, 10);
        assert!(e.folds().is_empty());
        // Loading another text forgets the folds.
        let mut e = eight();
        e.fold_lines(1, 3);
        e.load("x\ny");
        assert_eq!((e.folds().len(), e.visible_rows()), (0, 2));
    }
}
