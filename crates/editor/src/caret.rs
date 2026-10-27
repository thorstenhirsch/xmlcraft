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
/// so no two ever edit the same text. Returns the carets and where the one that held `primary_head` ended up.
pub fn normalize(mut carets: Vec<Caret>, primary_head: usize) -> (Vec<Caret>, usize) {
    carets.sort_by_key(|c| (c.start(), c.end()));
    let mut merged: Vec<Caret> = Vec::with_capacity(carets.len());
    for caret in carets {
        match merged.last_mut() {
            Some(last) if last.end() > caret.start() || (last.end() == caret.start() && (last.is_empty() || caret.is_empty())) => {
                let start = last.start().min(caret.start());
                let end = last.end().max(caret.end());
                // The merged selection keeps the direction of the one that came later.
                let forward = caret.sel.head >= caret.sel.anchor;
                last.sel = if forward { Selection { anchor: start, head: end } } else { Selection { anchor: end, head: start } };
                last.goal = caret.goal;
            }
            _ => merged.push(caret),
        }
    }
    let primary = merged.iter().position(|c| c.start() <= primary_head && primary_head <= c.end()).unwrap_or(merged.len().saturating_sub(1));
    (merged, primary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carets_are_sorted_and_merged() {
        let (carets, primary) = normalize(vec![Caret::at(9), Caret::at(2), Caret::at(2), Caret::new(5, 7)], 9);
        assert_eq!(carets.iter().map(|c| (c.start(), c.end())).collect::<Vec<_>>(), [(2, 2), (5, 7), (9, 9)]);
        assert_eq!(primary, 2);
    }

    #[test]
    fn overlapping_and_touching_selections_become_one() {
        let (carets, _) = normalize(vec![Caret::new(1, 5), Caret::new(4, 8)], 8);
        assert_eq!(carets.len(), 1);
        assert_eq!((carets[0].start(), carets[0].end()), (1, 8));
        let (carets, _) = normalize(vec![Caret::new(1, 3), Caret::at(3)], 3);
        assert_eq!(carets.len(), 1, "a caret on the edge of a selection");
        let (carets, _) = normalize(vec![Caret::new(1, 3), Caret::new(3, 5)], 3);
        assert_eq!(carets.len(), 2, "two selections that only touch stay apart");
        let (carets, _) = normalize(vec![Caret::new(8, 4), Caret::new(6, 2)], 2);
        assert_eq!((carets[0].start(), carets[0].end()), (2, 8));
        assert_eq!(carets[0].sel.head, 2, "backwards: the later caret's direction");
    }

    #[test]
    fn nothing_in_nothing_out() {
        let (carets, primary) = normalize(Vec::new(), 0);
        assert!(carets.is_empty());
        assert_eq!(primary, 0);
    }
}
