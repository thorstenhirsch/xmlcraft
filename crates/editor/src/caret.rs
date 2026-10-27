//! Carets: a selection plus the column that vertical movement tries to keep.

use xmlcraft_text::Selection;

/// A caret (an empty selection) or a selection, with the display column up/down keeps while it moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caret {
    pub sel: Selection,
    /// The column Up and Down aim for; `None` until a vertical move sets it, and reset by any other move.
    pub goal: Option<usize>,
}

impl Caret {
    pub fn at(offset: usize) -> Self {
        Self { sel: Selection::caret(offset), goal: None }
    }

    pub fn new(anchor: usize, head: usize) -> Self {
        Self { sel: Selection { anchor, head }, goal: None }
    }

    pub fn start(&self) -> usize {
        self.sel.start()
    }

    pub fn end(&self) -> usize {
        self.sel.end()
    }

    pub fn head(&self) -> usize {
        self.sel.head
    }

    pub fn is_empty(&self) -> bool {
        self.sel.is_caret()
    }
}

/// Sort carets by position and merge those that overlap or touch (two empty carets at one place, or a caret on the edge of a selection),
/// so no two ever edit the same text. `primary` is the index in `carets` of the primary caret; returns the carets and the index where
/// that one ended up (the last one if `primary` is out of range).
///
/// A merged selection keeps the direction of the selection among the pair, not of an empty caret, and of the later one when both are
/// selections.
pub fn normalize(carets: Vec<Caret>, primary: usize) -> (Vec<Caret>, usize) {
    let mut order: Vec<(usize, Caret)> = carets.into_iter().enumerate().collect();
    order.sort_by_key(|(_, c)| (c.start(), c.end()));
    let mut merged: Vec<Caret> = Vec::with_capacity(order.len());
    let mut primary_at = None;
    for (index, caret) in order {
        let slot = match merged.last_mut() {
            Some(last) if last.end() > caret.start() || (last.end() == caret.start() && (last.is_empty() || caret.is_empty())) => {
                let start = last.start().min(caret.start());
                let end = last.end().max(caret.end());
                let source = if caret.is_empty() && !last.is_empty() { *last } else { caret };
                let forward = source.sel.head >= source.sel.anchor;
                last.sel = if forward { Selection { anchor: start, head: end } } else { Selection { anchor: end, head: start } };
                last.goal = source.goal;
                merged.len() - 1
            }
            _ => {
                merged.push(caret);
                merged.len() - 1
            }
        };
        if index == primary {
            primary_at = Some(slot);
        }
    }
    let primary = primary_at.unwrap_or(merged.len().saturating_sub(1));
    (merged, primary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carets_are_sorted_and_merged() {
        // The primary caret is the one at 9, the first in the list.
        let (carets, primary) = normalize(vec![Caret::at(9), Caret::at(2), Caret::at(2), Caret::new(5, 7)], 0);
        assert_eq!(carets.iter().map(|c| (c.start(), c.end())).collect::<Vec<_>>(), [(2, 2), (5, 7), (9, 9)]);
        assert_eq!(primary, 2);
        let (_, primary) = normalize(vec![Caret::at(9), Caret::at(2), Caret::at(2), Caret::new(5, 7)], 2);
        assert_eq!(primary, 0, "the second of two carets that merge");
    }

    #[test]
    fn overlapping_and_touching_selections_become_one() {
        let (carets, _) = normalize(vec![Caret::new(1, 5), Caret::new(4, 8)], 1);
        assert_eq!(carets.len(), 1);
        assert_eq!((carets[0].start(), carets[0].end()), (1, 8));
        let (carets, _) = normalize(vec![Caret::new(1, 3), Caret::at(3)], 1);
        assert_eq!(carets.len(), 1, "a caret on the edge of a selection");
        let (carets, _) = normalize(vec![Caret::new(1, 3), Caret::new(3, 5)], 1);
        assert_eq!(carets.len(), 2, "two selections that only touch stay apart");
        let (carets, _) = normalize(vec![Caret::new(8, 4), Caret::new(6, 2)], 1);
        assert_eq!((carets[0].start(), carets[0].end()), (2, 8));
        assert_eq!(carets[0].sel.head, 2, "backwards: the later caret's direction");
    }

    #[test]
    fn the_primary_caret_stays_the_primary_caret() {
        // A backward selection whose head touches the end of the one before it: they stay apart, and the primary is the second.
        let (carets, primary) = normalize(vec![Caret::new(1, 3), Caret::new(5, 3)], 1);
        assert_eq!(carets.iter().map(|c| (c.start(), c.end())).collect::<Vec<_>>(), [(1, 3), (3, 5)]);
        assert_eq!(primary, 1, "head 3 is inside both, the caret that held it is the second");
        // The same, listed the other way round.
        let (_, primary) = normalize(vec![Caret::new(5, 3), Caret::new(1, 3)], 0);
        assert_eq!(primary, 1);
        // An index past the end means the last caret.
        assert_eq!(normalize(vec![Caret::at(1), Caret::at(4)], 9).1, 1);
    }

    #[test]
    fn a_selection_keeps_its_direction_when_an_empty_caret_joins_it() {
        // The empty caret sorts after the backward selection and touches it.
        let (carets, _) = normalize(vec![Caret::new(5, 1), Caret::at(5)], 0);
        assert_eq!((carets[0].start(), carets[0].end(), carets[0].sel.head), (1, 5, 1), "backward stays backward");
        // And before a forward one.
        let (carets, _) = normalize(vec![Caret::at(1), Caret::new(1, 5)], 1);
        assert_eq!((carets[0].start(), carets[0].end(), carets[0].sel.head), (1, 5, 5), "forward stays forward");
        // Inside a backward selection.
        let (carets, _) = normalize(vec![Caret::new(8, 2), Caret::at(5)], 0);
        assert_eq!((carets[0].start(), carets[0].end(), carets[0].sel.head), (2, 8, 2));
    }

    #[test]
    fn nothing_in_nothing_out() {
        let (carets, primary) = normalize(Vec::new(), 0);
        assert!(carets.is_empty());
        assert_eq!(primary, 0);
    }
}
