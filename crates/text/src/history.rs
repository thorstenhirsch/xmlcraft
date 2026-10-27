//! Undo and redo: a log of transactions, merged while the user types, bounded in memory.

use crate::buffer::{Buffer, Edit, TextError};

/// A selection: where it started and where the caret is now. Equal when it is just a caret.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn caret(offset: usize) -> Self {
        Self { anchor: offset, head: offset }
    }

    pub fn is_caret(&self) -> bool {
        self.anchor == self.head
    }

    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }
}

/// What kind of change a transaction is, which decides whether it merges into the one before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    /// Characters typed.
    Typing,
    /// Characters deleted one by one (Backspace, Delete).
    Delete,
    /// Anything that should be its own undo step: a paste, a format, a replace, a command.
    Other,
}

impl EditKind {
    fn merges(self) -> bool {
        matches!(self, EditKind::Typing | EditKind::Delete)
    }
}

/// How long a pause between keystrokes still counts as one typing run, in seconds.
const MERGE_WINDOW: f64 = 1.0;
/// The most memory the history keeps (the text of its edits, both directions).
const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
struct Entry {
    /// The edits that make the change, in the order they were applied.
    forward: Vec<Edit>,
    /// The edits that undo it, in the order they are applied (the last change first).
    inverse: Vec<Edit>,
    before: Vec<Selection>,
    after: Vec<Selection>,
    kind: EditKind,
    time: f64,
}

impl Entry {
    fn bytes(&self) -> usize {
        self.forward.iter().chain(&self.inverse).map(|e| e.text.len() + 32).sum::<usize>() + 64
    }
}

/// The undo and redo stacks of one document.
#[derive(Clone, Debug)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    bytes: usize,
    max_bytes: usize,
    /// No merging into the last entry (the caret moved, or the document was saved).
    closed: bool,
    /// How many of the oldest steps were dropped to stay within the memory limit.
    dropped: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    pub fn new() -> Self {
        Self::with_limit(DEFAULT_MAX_BYTES)
    }

    pub fn with_limit(max_bytes: usize) -> Self {
        Self { undo: Vec::new(), redo: Vec::new(), bytes: 0, max_bytes, closed: true, dropped: 0 }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// How many undo steps there are.
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// How many of the oldest steps have been dropped because the history grew past its memory limit.
    pub fn dropped_steps(&self) -> usize {
        self.dropped
    }

    /// The next edit starts a new undo step, however soon it comes: call it when the caret moves by itself or the file is saved.
    pub fn break_group(&mut self) {
        self.closed = true;
    }

    /// Forget everything (after a reload from disk).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.bytes = 0;
        self.closed = true;
    }

    /// Record a change that has just been applied: `forward` are its edits in order, `inverse` the edits that undo it (as returned
    /// by [`Buffer::apply_all`]), `before` and `after` the selections around it, `time` in seconds. A new change ends the redo
    /// stack.
    pub fn record(&mut self, forward: Vec<Edit>, inverse: Vec<Edit>, before: Vec<Selection>, after: Vec<Selection>, kind: EditKind, time: f64) {
        if forward.is_empty() {
            return;
        }
        self.redo.clear();
        let merge = kind.merges()
            && !self.closed
            && self.undo.last().is_some_and(|last| last.kind == kind && last.after == before && (time - last.time).abs() <= MERGE_WINDOW);
        if merge && let Some(last) = self.undo.last_mut() {
            self.bytes = self.bytes.saturating_sub(last.bytes());
            last.forward.extend(forward);
            // The newest change is undone first.
            let mut inverse = inverse;
            inverse.append(&mut last.inverse);
            last.inverse = inverse;
            last.after = after;
            last.time = time;
            self.bytes = self.bytes.saturating_add(last.bytes());
        } else {
            let entry = Entry { forward, inverse, before, after, kind, time };
            self.bytes = self.bytes.saturating_add(entry.bytes());
            self.undo.push(entry);
        }
        self.closed = !kind.merges();
        self.trim();
    }

    /// Drop the oldest steps while over the memory limit (always keeping the newest).
    fn trim(&mut self) {
        let mut dropped = 0;
        while self.bytes > self.max_bytes && self.undo.len() > 1 {
            let entry = self.undo.remove(0);
            self.bytes = self.bytes.saturating_sub(entry.bytes());
            dropped += 1;
        }
        self.dropped = self.dropped.saturating_add(dropped);
    }

    /// Undo the last change; returns the selections from before it.
    pub fn undo(&mut self, buffer: &mut Buffer) -> Result<Option<Vec<Selection>>, TextError> {
        let Some(entry) = self.undo.pop() else { return Ok(None) };
        self.bytes = self.bytes.saturating_sub(entry.bytes());
        match buffer.apply_all(&entry.inverse) {
            Ok(_) => {}
            Err(e) => {
                // The text no longer matches this history (it was changed behind its back): the history is useless from here.
                self.clear();
                return Err(e);
            }
        }
        let before = entry.before.clone();
        self.bytes = self.bytes.saturating_add(entry.bytes());
        self.redo.push(entry);
        self.closed = true;
        Ok(Some(before))
    }

    /// Redo the last undone change; returns the selections from after it.
    pub fn redo(&mut self, buffer: &mut Buffer) -> Result<Option<Vec<Selection>>, TextError> {
        let Some(entry) = self.redo.pop() else { return Ok(None) };
        self.bytes = self.bytes.saturating_sub(entry.bytes());
        match buffer.apply_all(&entry.forward) {
            Ok(_) => {}
            Err(e) => {
                self.clear();
                return Err(e);
            }
        }
        let after = entry.after.clone();
        self.bytes = self.bytes.saturating_add(entry.bytes());
        self.undo.push(entry);
        self.closed = true;
        Ok(Some(after))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Apply `edits` to `buf` and record them.
    fn do_edit(buf: &mut Buffer, h: &mut History, edits: Vec<Edit>, before: usize, after: usize, kind: EditKind, time: f64) {
        let inverse = buf.apply_all(&edits).expect("valid edits");
        h.record(edits, inverse, vec![Selection::caret(before)], vec![Selection::caret(after)], kind, time);
    }

    #[test]
    fn undo_and_redo_restore_the_text_and_the_selections() {
        let mut buf = Buffer::from_text("abc");
        let mut h = History::new();
        do_edit(&mut buf, &mut h, vec![Edit::insert(3, "d")], 3, 4, EditKind::Other, 0.0);
        do_edit(&mut buf, &mut h, vec![Edit::delete(0..1)], 0, 0, EditKind::Other, 5.0);
        assert!(buf.eq_str("bcd"));
        assert_eq!(h.undo(&mut buf).expect("ok"), Some(vec![Selection::caret(0)]));
        assert!(buf.eq_str("abcd"));
        assert_eq!(h.undo(&mut buf).expect("ok"), Some(vec![Selection::caret(3)]));
        assert!(buf.eq_str("abc"));
        assert_eq!(h.undo(&mut buf).expect("ok"), None, "nothing more to undo");
        assert_eq!(h.redo(&mut buf).expect("ok"), Some(vec![Selection::caret(4)]));
        assert_eq!(h.redo(&mut buf).expect("ok"), Some(vec![Selection::caret(0)]));
        assert!(buf.eq_str("bcd"));
        assert_eq!(h.redo(&mut buf).expect("ok"), None);
    }

    #[test]
    fn a_new_change_ends_the_redo_stack() {
        let mut buf = Buffer::from_text("");
        let mut h = History::new();
        do_edit(&mut buf, &mut h, vec![Edit::insert(0, "a")], 0, 1, EditKind::Other, 0.0);
        h.undo(&mut buf).expect("ok");
        assert!(h.can_redo());
        do_edit(&mut buf, &mut h, vec![Edit::insert(0, "b")], 0, 1, EditKind::Other, 1.0);
        assert!(!h.can_redo());
    }

    #[test]
    fn typing_in_a_run_is_one_undo_step() {
        let mut buf = Buffer::from_text("");
        let mut h = History::new();
        for (i, ch) in "hello".chars().enumerate() {
            do_edit(&mut buf, &mut h, vec![Edit::insert(i, ch.to_string())], i, i + 1, EditKind::Typing, i as f64 * 0.1);
        }
        assert_eq!(h.undo_len(), 1);
        assert!(buf.eq_str("hello"));
        h.undo(&mut buf).expect("ok");
        assert!(buf.eq_str(""));
        h.redo(&mut buf).expect("ok");
        assert!(buf.eq_str("hello"));
    }

    #[test]
    fn a_pause_a_jump_a_different_kind_or_a_break_start_a_new_step() {
        let mut buf = Buffer::from_text("");
        let mut h = History::new();
        do_edit(&mut buf, &mut h, vec![Edit::insert(0, "a")], 0, 1, EditKind::Typing, 0.0);
        do_edit(&mut buf, &mut h, vec![Edit::insert(1, "b")], 1, 2, EditKind::Typing, 5.0);
        assert_eq!(h.undo_len(), 2, "a long pause");
        do_edit(&mut buf, &mut h, vec![Edit::insert(0, "c")], 0, 1, EditKind::Typing, 5.1);
        assert_eq!(h.undo_len(), 3, "the caret was elsewhere");
        do_edit(&mut buf, &mut h, vec![Edit::delete(0..1)], 1, 0, EditKind::Delete, 5.2);
        assert_eq!(h.undo_len(), 4, "deleting is not typing");
        h.break_group();
        do_edit(&mut buf, &mut h, vec![Edit::delete(0..1)], 0, 0, EditKind::Delete, 5.3);
        assert_eq!(h.undo_len(), 5, "after an explicit break");
        do_edit(&mut buf, &mut h, vec![Edit::insert(0, "p")], 0, 1, EditKind::Other, 5.4);
        do_edit(&mut buf, &mut h, vec![Edit::insert(1, "q")], 1, 2, EditKind::Other, 5.5);
        assert_eq!(h.undo_len(), 7, "pastes and commands never merge");
    }

    #[test]
    fn a_merged_step_with_many_edits_undoes_in_the_right_order() {
        let mut buf = Buffer::from_text("");
        let mut h = History::new();
        do_edit(&mut buf, &mut h, vec![Edit::insert(0, "abc")], 0, 3, EditKind::Typing, 0.0);
        do_edit(&mut buf, &mut h, vec![Edit::insert(0, "X"), Edit::insert(4, "Y")], 3, 5, EditKind::Typing, 0.2);
        assert!(buf.eq_str("XabcY"));
        assert_eq!(h.undo_len(), 1);
        h.undo(&mut buf).expect("ok");
        assert!(buf.eq_str(""));
        h.redo(&mut buf).expect("ok");
        assert!(buf.eq_str("XabcY"));
    }

    #[test]
    fn the_history_stays_within_its_memory_limit_and_keeps_the_newest_step() {
        let mut buf = Buffer::from_text("");
        let mut h = History::with_limit(2_000);
        for i in 0..50 {
            let end = buf.len();
            do_edit(&mut buf, &mut h, vec![Edit::insert(end, "x".repeat(100))], 0, 0, EditKind::Other, f64::from(i));
        }
        assert!(h.undo_len() < 50 && h.undo_len() >= 1, "{}", h.undo_len());
        assert_eq!(h.dropped_steps() + h.undo_len(), 50);
        assert!(h.bytes <= 2_000 + 400, "{}", h.bytes);
        // One huge edit is kept, even though it alone is over the limit.
        let big = Edit::insert(buf.len(), "y".repeat(10_000));
        do_edit(&mut buf, &mut h, vec![big], 0, 0, EditKind::Other, 99.0);
        assert_eq!(h.undo_len(), 1);
        h.undo(&mut buf).expect("ok");
    }

    #[test]
    fn a_text_changed_behind_the_histories_back_is_an_error_not_a_panic() {
        let mut buf = Buffer::from_text("abc");
        let mut h = History::new();
        do_edit(&mut buf, &mut h, vec![Edit::insert(3, "d")], 3, 4, EditKind::Other, 0.0);
        buf.apply(&Edit::delete(0..4)).expect("valid");
        assert!(h.undo(&mut buf).is_err());
        assert!(!h.can_undo() && !h.can_redo(), "the history is dropped");
        assert_eq!(h.undo(&mut buf).expect("nothing to do"), None);
    }

    #[test]
    fn selections_know_their_ends() {
        let s = Selection { anchor: 9, head: 3 };
        assert_eq!((s.start(), s.end(), s.is_caret()), (3, 9, false));
        assert!(Selection::caret(4).is_caret());
    }

    #[test]
    fn recording_nothing_records_nothing() {
        let mut h = History::new();
        h.record(Vec::new(), Vec::new(), vec![], vec![], EditKind::Typing, 0.0);
        assert!(!h.can_undo());
        h.clear();
    }
}
