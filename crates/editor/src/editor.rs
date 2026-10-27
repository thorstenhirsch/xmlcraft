//! The editor: a buffer, its undo history and the carets, with the commands that change them.
//!
//! Every command works on all carets at once. A command that changes text first plans its edits (one per caret, in order, not
//! overlapping), applies them as one undoable step, and maps the carets through the edits. A command that cannot be carried out leaves
//! the editor as it was; nothing here panics.

use std::ops::Range;

use xmlcraft_syntax::Highlighter;
use xmlcraft_text::eol::{self, Eol};
use xmlcraft_text::{Buffer, Edit, EditKind, History, Selection};

use crate::caret::{Caret, normalize};
use crate::folds::{Fold, FoldMap};
use crate::motion::{
    clamp, display_column, first_non_blank, next_grapheme, offset_at_column, prev_grapheme, smart_home, word_left, word_range_at, word_right,
};

/// How text is indented and how new lines end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Display width of a tab character.
    pub tab_width: usize,
    /// How many columns one level of indentation is.
    pub indent_size: usize,
    /// Indent with tab characters rather than spaces.
    pub use_tabs: bool,
    /// What new line breaks are written as (the document's dominant line ending).
    pub eol: Eol,
    /// Typing a quote after `=` in a tag writes the pair and steps over the closing one ([`Editor::type_input`]).
    pub auto_pair: bool,
    /// Typing `>` that ends a start tag writes the end tag, and `</` is completed ([`Editor::type_input`]).
    pub auto_close: bool,
    /// Enter between a start tag and its end tag puts the end tag on a line of its own, and after a start tag indents one more level
    /// ([`Editor::insert_newline`]).
    pub smart_indent: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { tab_width: 4, indent_size: 2, use_tabs: false, eol: Eol::Lf, auto_pair: true, auto_close: true, smart_indent: true }
    }
}

/// A caret movement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    /// To the first non-blank character, or the line start if already there.
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
    /// Up by this many lines.
    PageUp(usize),
    PageDown(usize),
}

/// Where an offset at the exact place of an insertion goes when it is mapped through the edits.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bias {
    Before,
    After,
}

/// `offset` (in the text before `edits`) in the text after them. `edits` are ascending and do not overlap. An offset inside a replaced
/// range moves to the end of what replaced it.
pub(crate) fn map_offset(edits: &[Edit], offset: usize, bias: Bias) -> usize {
    let mut shift: isize = 0;
    for edit in edits {
        if offset > edit.range.start && offset < edit.range.end {
            return edit.range.start.saturating_add_signed(shift) + edit.text.len();
        }
        if offset < edit.range.end || (offset == edit.range.end && edit.range.is_empty() && bias == Bias::Before) {
            break;
        }
        shift += edit.delta();
    }
    offset.saturating_add_signed(shift)
}

/// How many recent changes the editor remembers the first line of.
const CHANGE_LOG: usize = 64;

pub struct Editor {
    pub(crate) buffer: Buffer,
    /// Where in the markup an offset is (inside a tag, a value, a comment…), for the typing aids. Knows the modes of the lines it has been
    /// asked about, and forgets those after a change.
    pub(crate) lexer: Highlighter,
    /// Stretches of lines that are hidden, in the order of the text (see [`crate::Fold`]).
    pub(crate) folds: Vec<Fold>,
    /// Which lines the folds hide, for the current text and folds.
    pub(crate) fold_map: FoldMap,
    history: History,
    pub(crate) carets: Vec<Caret>,
    pub(crate) primary: usize,
    settings: Settings,
    version: u64,
    time: f64,
    reveal: bool,
    /// For recent versions, the first line whose text changed (lines before it are as they were).
    changes: std::collections::VecDeque<(u64, usize)>,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new("")
    }
}

impl Editor {
    /// An editor on `text` with the default settings, except that new line breaks use the text's dominant line ending.
    pub fn new(text: &str) -> Self {
        let defaults = Settings::default();
        Self::with_settings(text, Settings { eol: line_ending_of(text, defaults.eol), ..defaults })
    }

    pub fn with_settings(text: &str, settings: Settings) -> Self {
        Self {
            buffer: Buffer::from_text(text),
            lexer: Highlighter::new(),
            folds: Vec::new(),
            fold_map: FoldMap::default(),
            history: History::new(),
            carets: vec![Caret::at(0)],
            primary: 0,
            settings,
            version: 0,
            time: 0.0,
            reveal: false,
            changes: std::collections::VecDeque::new(),
        }
    }

    // --- reading -------------------------------------------------------------------------------------------------------------

    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    pub fn text(&self) -> String {
        self.buffer.text()
    }

    /// Counts up with every change of the text (including undo and redo), not with caret moves.
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn set_settings(&mut self, settings: Settings) {
        self.settings = settings;
    }

    pub fn carets(&self) -> &[Caret] {
        &self.carets
    }

    /// The caret the view follows.
    pub fn primary(&self) -> Caret {
        self.carets.get(self.primary).or_else(|| self.carets.last()).copied().unwrap_or(Caret::at(0))
    }

    /// Names the text as it is now: it changes with every change and comes back with undo and redo, so a document can tell that undoing
    /// has returned it to what was saved. (Loading a text gives a state no earlier one has.)
    pub fn state_id(&self) -> u64 {
        self.history.state()
    }

    /// Where the primary caret is in [`Editor::carets`]: always a valid index.
    pub fn primary_index(&self) -> usize {
        self.primary.min(self.carets.len().saturating_sub(1))
    }

    pub fn selections(&self) -> Vec<Selection> {
        self.carets.iter().map(|c| c.sel).collect()
    }

    /// The selections with the primary caret's last, which is how [`Editor::set_selections`] reads them: undo and redo give back the
    /// primary caret that an edit had, not the last one in the text.
    fn selections_for_history(&self) -> Vec<Selection> {
        let primary = self.primary_index();
        let mut out: Vec<Selection> = self.carets.iter().enumerate().filter(|(i, _)| *i != primary).map(|(_, c)| c.sel).collect();
        out.extend(self.carets.get(primary).map(|c| c.sel));
        out
    }

    pub fn has_selection(&self) -> bool {
        self.carets.iter().any(|c| !c.is_empty())
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// The time (in seconds) edits are stamped with, which decides whether typing merges into one undo step.
    pub fn set_time(&mut self, seconds: f64) {
        self.time = seconds;
    }

    /// The first line whose text changed since `version` (0-based), or `None` if nothing did. Whatever is derived from the lines above it
    /// (colouring, folds) is still good. After more changes than the editor remembers, or an undo, the answer is the first line.
    pub fn first_changed_line_since(&self, version: u64) -> Option<usize> {
        if version >= self.version {
            return None;
        }
        let oldest = self.changes.front().map_or(self.version, |(v, _)| *v);
        if oldest > version + 1 {
            return Some(0);
        }
        self.changes.iter().filter(|(v, _)| *v > version).map(|(_, line)| *line).min().or(Some(0))
    }

    fn log_change(&mut self, first_line: usize) {
        self.lexer.invalidate_from(first_line);
        self.changes.push_back((self.version, first_line));
        while self.changes.len() > CHANGE_LOG {
            self.changes.pop_front();
        }
    }

    /// Whether a change asked for the primary caret to be scrolled into view; reading it clears it.
    pub fn take_reveal(&mut self) -> bool {
        std::mem::take(&mut self.reveal)
    }

    /// The next edit starts a new undo step, however soon it comes (after the caret was moved by the mouse, or the file was saved).
    pub fn break_undo_group(&mut self) {
        self.history.break_group();
    }

    /// Make sure [`Editor::version`] is above `floor`, so that a text replacing this editor's (a reload) never has a version an earlier
    /// text had: results that are about versions (analyses) cannot then be taken for the new text's.
    pub fn raise_version_above(&mut self, floor: u64) {
        self.version = self.version.max(floor.saturating_add(1));
    }

    /// Replace the whole text (loading a file): history is cleared and the caret goes to the start. New line breaks use the text's
    /// dominant line ending (the current one when the text has no line break).
    pub fn load(&mut self, text: &str) {
        self.settings.eol = line_ending_of(text, self.settings.eol);
        self.buffer = Buffer::from_text(text);
        self.history.clear();
        self.folds.clear();
        self.fold_map = FoldMap::default();
        self.carets = vec![Caret::at(0)];
        self.primary = 0;
        self.version += 1;
        self.log_change(0);
        self.reveal = true;
    }

    // --- carets and selections -----------------------------------------------------------------------------------------------

    fn sanitized(&self, sel: Selection) -> Selection {
        Selection { anchor: clamp(&self.buffer, sel.anchor), head: clamp(&self.buffer, sel.head) }
    }

    /// Make `carets` the carets; `primary` is the index in the list of the primary one.
    pub(crate) fn set_carets_raw(&mut self, carets: Vec<Caret>, primary: usize) {
        let carets: Vec<Caret> = carets.into_iter().map(|c| Caret { sel: self.sanitized(c.sel), goal: c.goal }).collect();
        let (carets, primary) = normalize(carets, primary);
        if carets.is_empty() {
            self.carets = vec![Caret::at(0)];
            self.primary = 0;
        } else {
            self.carets = carets;
            self.primary = primary;
        }
        self.reveal = true;
        // A caret that lands in a fold (a jump to a match, a message, the outline) opens it.
        self.reveal_carets();
    }

    /// One selection, nothing else.
    pub fn set_selection(&mut self, anchor: usize, head: usize) {
        self.set_carets_raw(vec![Caret::new(anchor, head)], 0);
    }

    pub fn set_selections(&mut self, selections: &[Selection]) {
        self.set_carets_raw(selections.iter().map(|s| Caret { sel: *s, goal: None }).collect(), selections.len().saturating_sub(1));
    }

    pub fn select_all(&mut self) {
        self.set_selection(0, self.buffer.len());
    }

    /// A click: the caret goes to `offset`. With `add` it is a further caret, otherwise the only one.
    pub fn place_caret(&mut self, offset: usize, add: bool) {
        let at = clamp(&self.buffer, offset);
        let mut carets = if add { self.carets.clone() } else { Vec::new() };
        carets.push(Caret::at(at));
        let primary = carets.len().saturating_sub(1);
        self.set_carets_raw(carets, primary);
    }

    /// A drag or a shift-click: the primary caret's head goes to `offset`, its anchor stays.
    pub fn extend_selection_to(&mut self, offset: usize) {
        let at = clamp(&self.buffer, offset);
        let mut carets = self.carets.clone();
        if let Some(caret) = carets.get_mut(self.primary) {
            *caret = Caret::new(caret.sel.anchor, at);
        }
        self.set_carets_raw(carets, self.primary);
    }

    /// A double click: the word at `offset`.
    pub fn select_word_at(&mut self, offset: usize, add: bool) {
        let range = word_range_at(&self.buffer, offset);
        self.select_range(range, add);
    }

    /// A triple click: the line at `offset`, with its line break.
    pub fn select_line_at(&mut self, offset: usize, add: bool) {
        let line = self.buffer.line_of(clamp(&self.buffer, offset));
        let range = self.buffer.line_range_with_break(line).unwrap_or(offset..offset);
        self.select_range(range, add);
    }

    fn select_range(&mut self, range: Range<usize>, add: bool) {
        let mut carets = if add { self.carets.clone() } else { Vec::new() };
        carets.push(Caret::new(range.start, range.end));
        let primary = carets.len().saturating_sub(1);
        self.set_carets_raw(carets, primary);
    }

    /// Escape: back to the primary caret alone, keeping its selection.
    pub fn keep_primary_only(&mut self) {
        let primary = self.primary();
        self.set_carets_raw(vec![primary], 0);
    }

    pub fn move_carets(&mut self, motion: Motion, extend: bool) {
        let carets: Vec<Caret> = self
            .carets
            .iter()
            .map(|c| {
                let mut moved = self.moved(*c, motion, extend);
                // A movement that ends in a fold goes past it, or back to the line that stands for it: it does not open it.
                moved.sel.head = self.out_of_folds(moved.sel.head, moved.sel.head >= c.head());
                if !extend {
                    moved.sel.anchor = moved.sel.head;
                }
                moved
            })
            .collect();
        self.set_carets_raw(carets, self.primary);
    }

    fn moved(&self, caret: Caret, motion: Motion, extend: bool) -> Caret {
        let buf = &self.buffer;
        let tab = self.settings.tab_width;
        let head = caret.head();
        let mut goal = None;
        let mut vertical = |lines: isize| -> usize {
            let row = self.row_of_line(buf.line_of(head));
            let column = caret.goal.unwrap_or_else(|| display_column(buf, head, tab));
            goal = Some(column);
            let target = row as isize + lines;
            if target < 0 {
                0
            } else if target as usize >= self.visible_rows() {
                buf.len()
            } else {
                offset_at_column(buf, self.line_of_row(target as usize), column, tab)
            }
        };
        let to = match motion {
            Motion::Left if !extend && !caret.is_empty() => caret.start(),
            Motion::Right if !extend && !caret.is_empty() => caret.end(),
            Motion::Left => prev_grapheme(buf, head),
            Motion::Right => next_grapheme(buf, head),
            Motion::WordLeft => word_left(buf, head),
            Motion::WordRight => word_right(buf, head),
            Motion::LineStart => smart_home(buf, head),
            Motion::LineEnd => buf.line_range(buf.line_of(head)).map_or(head, |r| r.end),
            Motion::DocStart => 0,
            Motion::DocEnd => buf.len(),
            Motion::Up => vertical(-1),
            Motion::Down => vertical(1),
            Motion::PageUp(n) => vertical(-(n.min(isize::MAX as usize) as isize)),
            Motion::PageDown(n) => vertical(n.min(isize::MAX as usize) as isize),
        };
        let sel = if extend { Selection { anchor: caret.sel.anchor, head: to } } else { Selection::caret(to) };
        Caret { sel, goal }
    }

    // --- changing the text ---------------------------------------------------------------------------------------------------

    /// Apply `edits` (ascending, not overlapping) as one undoable step and set the carets; `carets` are in the text after the edits.
    /// Returns whether the text changed.
    pub(crate) fn commit(&mut self, edits: Vec<Edit>, carets: Vec<Caret>, kind: EditKind) -> bool {
        let primary = self.primary.min(carets.len().saturating_sub(1));
        if edits.is_empty() || edits.iter().all(|e| e.range.is_empty() && e.text.is_empty()) {
            self.set_carets_raw(carets, primary);
            return false;
        }
        let before = self.selections_for_history();
        let first_changed = edits.iter().map(|e| e.range.start).min().unwrap_or(0);
        // Last first: the offsets of the edits that come earlier in the text stay valid while the later ones are applied.
        let ordered: Vec<Edit> = edits.into_iter().rev().collect();
        let Ok(inverse) = self.buffer.apply_all(&ordered) else { return false };
        self.remap_folds(&ordered);
        self.set_carets_raw(carets, primary);
        let after = self.selections_for_history();
        self.history.record(ordered, inverse, before, after, kind, self.time);
        self.version += 1;
        // The text before the first edit is unchanged, so that offset is on the same line as before.
        self.log_change(self.buffer.line_of(first_changed.min(self.buffer.len())));
        true
    }

    /// One edit per caret from `plan` (or none, for a caret there is nothing to do for), applied together. Each caret ends after the new
    /// text, or where the deleted text began.
    fn replace_at_carets(&mut self, kind: EditKind, plan: impl Fn(&Self, &Caret) -> Option<Edit>) -> bool {
        let planned: Vec<Option<Edit>> = self.carets.iter().map(|c| plan(self, c)).collect();
        let edits = coalesce(planned.iter().flatten().cloned().collect());
        let carets: Vec<Caret> = self
            .carets
            .iter()
            .zip(&planned)
            .map(|(caret, edit)| match edit {
                Some(e) => Caret::at(map_offset(&edits, e.range.start, Bias::Before) + e.text.len()),
                None => Caret::at(map_offset(&edits, caret.end(), Bias::After)),
            })
            .collect();
        self.commit(edits, carets, kind)
    }

    /// Typing: each caret's selection is replaced by `text`. Consecutive typing merges into one undo step.
    pub fn type_text(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        self.replace_at_carets(EditKind::Typing, |_, c| Some(Edit::replace(c.start()..c.end(), text)))
    }

    /// Insert pasted `text`. Line breaks become the document's. When there are as many lines as carets, each caret gets one line.
    pub fn paste(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        let normalized = eol::convert(text, self.settings.eol);
        let lines: Vec<&str> = normalized.split(self.settings.eol.as_str()).collect();
        let distribute = self.carets.len() > 1 && lines.len() == self.carets.len();
        let pieces: Vec<String> =
            if distribute { lines.iter().map(|l| (*l).to_owned()).collect() } else { vec![normalized.clone(); self.carets.len()] };
        let planned: Vec<Edit> = self.carets.iter().zip(pieces).map(|(c, piece)| Edit::replace(c.start()..c.end(), piece)).collect();
        let carets = planned.iter().map(|e| Caret::at(map_offset(&planned, e.range.start, Bias::Before) + e.text.len())).collect();
        self.commit(planned, carets, EditKind::Other)
    }

    /// Enter: a line break (the document's) followed by the indentation of the line the caret is on, up to the caret. With
    /// [`Settings::smart_indent`]: after a start tag the new line is indented one level more, and between a start tag and its end tag
    /// (`<a>|</a>`) there is a line for the content and the end tag goes to a line of its own.
    pub fn insert_newline(&mut self) -> bool {
        let eol = self.settings.eol.as_str().to_owned();
        let unit = self.indent_unit();
        let smart = self.settings.smart_indent;
        let carets: Vec<Caret> = self.carets.clone();
        // What to write for each caret, and where the caret is in it.
        let writes: Vec<(String, usize)> = carets
            .iter()
            .map(|c| {
                let line = self.buffer.line_of(c.start());
                let line_start = self.buffer.line_start(line).unwrap_or(0);
                let indent_end = first_non_blank(&self.buffer, line).min(c.start());
                let indent = self.buffer.slice(line_start..indent_end).unwrap_or_default();
                // After the end of a start tag, the new line is one level in from the line the tag begins on (a tag over several lines
                // has its attributes on lines of their own).
                let start_tag = if smart { self.after_start_tag(c.start()) } else { None };
                let before_end_tag = start_tag.is_some()
                    && c.is_empty()
                    && self.buffer.slice(c.end()..self.buffer.next_char(self.buffer.next_char(c.end()))).is_ok_and(|s| s == "</");
                let tag_indent = start_tag.map(|at| {
                    let tag_line = self.buffer.line_of(at);
                    let from = self.buffer.line_start(tag_line).unwrap_or(0);
                    self.buffer.slice(from..first_non_blank(&self.buffer, tag_line).min(at)).unwrap_or_default()
                });
                if let (true, Some(base)) = (before_end_tag, &tag_indent) {
                    let caret = eol.len() + base.len() + unit.len();
                    (format!("{eol}{base}{unit}{eol}{base}"), caret)
                } else if let Some(base) = &tag_indent {
                    let text = format!("{eol}{base}{unit}");
                    let caret = text.len();
                    (text, caret)
                } else {
                    let text = format!("{eol}{indent}");
                    let caret = text.len();
                    (text, caret)
                }
            })
            .collect();
        let edits = coalesce(carets.iter().zip(&writes).map(|(c, (text, _))| Edit::replace(c.start()..c.end(), text.clone())).collect());
        let after: Vec<Caret> =
            carets.iter().zip(&writes).map(|(c, (_, caret))| Caret::at(map_offset(&edits, c.start(), Bias::Before) + caret)).collect();
        self.commit(edits, after, EditKind::Other)
    }

    /// Backspace: the selection, or the character cluster before the caret; between the quotes of an empty attribute value, both quotes.
    pub fn backspace(&mut self) -> bool {
        let pairs = self.carets_in_empty_quotes();
        self.replace_at_carets(EditKind::Delete, move |this, c| {
            if !c.is_empty() {
                return Some(Edit::delete(c.start()..c.end()));
            }
            if pairs.binary_search(&c.head()).is_ok() {
                return Some(Edit::delete(c.head().saturating_sub(1)..c.head().saturating_add(1)));
            }
            let from = prev_grapheme(&this.buffer, c.head());
            (from < c.head()).then(|| Edit::delete(from..c.head()))
        })
    }

    /// Delete: the selection, or the character cluster after the caret.
    pub fn delete_forward(&mut self) -> bool {
        self.replace_at_carets(EditKind::Delete, |this, c| {
            if !c.is_empty() {
                return Some(Edit::delete(c.start()..c.end()));
            }
            let to = next_grapheme(&this.buffer, c.head());
            (to > c.head()).then(|| Edit::delete(c.head()..to))
        })
    }

    pub fn delete_word_backward(&mut self) -> bool {
        self.replace_at_carets(EditKind::Other, |this, c| {
            let from = if c.is_empty() { word_left(&this.buffer, c.head()) } else { c.start() };
            let to = c.end();
            (from < to).then(|| Edit::delete(from..to))
        })
    }

    pub fn delete_word_forward(&mut self) -> bool {
        self.replace_at_carets(EditKind::Other, |this, c| {
            let to = if c.is_empty() { word_right(&this.buffer, c.head()) } else { c.end() };
            let from = c.start();
            (from < to).then(|| Edit::delete(from..to))
        })
    }

    /// Delete from the caret back to the start of its line (or the selection).
    pub fn delete_to_line_start(&mut self) -> bool {
        self.replace_at_carets(EditKind::Other, |this, c| {
            let from = if c.is_empty() { this.buffer.line_start(this.buffer.line_of(c.head())).unwrap_or(0) } else { c.start() };
            (from < c.end()).then(|| Edit::delete(from..c.end()))
        })
    }

    /// Delete from the caret to the end of its line (or the selection); at the end of a line, the line break.
    pub fn delete_to_line_end(&mut self) -> bool {
        self.replace_at_carets(EditKind::Other, |this, c| {
            let buf = &this.buffer;
            let to = if c.is_empty() {
                let line = buf.line_of(c.head());
                let end = buf.line_range(line).map_or(c.head(), |r| r.end);
                if end > c.head() { end } else { buf.line_range_with_break(line).map_or(end, |r| r.end) }
            } else {
                c.end()
            };
            (c.start() < to).then(|| Edit::delete(c.start()..to))
        })
    }

    /// Delete `before` characters before and `after` characters after each caret (or selection): an input method asks for this when it
    /// replaces the text around the caret.
    pub fn delete_surrounding(&mut self, before: usize, after: usize) -> bool {
        self.replace_at_carets(EditKind::Other, |this, c| {
            let buf = &this.buffer;
            let mut from = c.start();
            for _ in 0..before.min(buf.len()) {
                if from == 0 {
                    break;
                }
                from = buf.prev_char(from);
            }
            let mut to = c.end();
            for _ in 0..after.min(buf.len()) {
                if to >= buf.len() {
                    break;
                }
                to = buf.next_char(to);
            }
            (from < to).then(|| Edit::delete(from..to))
        })
    }

    // --- indentation ---------------------------------------------------------------------------------------------------------

    fn indent_unit(&self) -> String {
        if self.settings.use_tabs { "\t".to_owned() } else { " ".repeat(self.settings.indent_size.max(1)) }
    }

    /// The lines the carets touch (a selection that ends at the very start of a line does not touch that line), each once.
    fn touched_lines(&self) -> Vec<usize> {
        let mut lines: Vec<usize> = Vec::new();
        for caret in &self.carets {
            let first = self.buffer.line_of(caret.start());
            let mut last = self.buffer.line_of(caret.end());
            if last > first && self.buffer.line_start(last) == Some(caret.end()) {
                last -= 1;
            }
            lines.extend(first..=last);
        }
        lines.sort_unstable();
        lines.dedup();
        lines
    }

    fn spans_lines(&self) -> bool {
        self.carets.iter().any(|c| self.buffer.line_of(c.start()) != self.buffer.line_of(c.end()))
    }

    /// Tab: with a selection over several lines, indent those lines; otherwise insert up to the next indent stop (or a tab).
    pub fn indent(&mut self) -> bool {
        let unit = self.indent_unit();
        if !self.spans_lines() {
            let size = self.settings.indent_size.max(1);
            let tab = self.settings.tab_width;
            let use_tabs = self.settings.use_tabs;
            return self.replace_at_carets(EditKind::Typing, |this, c| {
                let text = if use_tabs { unit.clone() } else { " ".repeat(size - display_column(&this.buffer, c.start(), tab) % size) };
                Some(Edit::replace(c.start()..c.end(), text))
            });
        }
        let edits: Vec<Edit> = self
            .touched_lines()
            .into_iter()
            .filter(|line| self.buffer.line_range(*line).is_some_and(|r| !r.is_empty()))
            .filter_map(|line| self.buffer.line_start(line).map(|at| Edit::insert(at, unit.clone())))
            .collect();
        let carets = self.mapped_carets(&edits);
        self.commit(edits, carets, EditKind::Other)
    }

    /// Shift-Tab: take one level of indentation off each touched line.
    pub fn outdent(&mut self) -> bool {
        let size = self.settings.indent_size.max(1);
        let edits: Vec<Edit> = self
            .touched_lines()
            .into_iter()
            .filter_map(|line| {
                let start = self.buffer.line_start(line)?;
                let removable = if self.buffer.char_at(start) == Some('\t') {
                    1
                } else {
                    (0..size).take_while(|i| self.buffer.char_at(start + i) == Some(' ')).count()
                };
                (removable > 0).then(|| Edit::delete(start..start + removable))
            })
            .collect();
        let carets = self.mapped_carets(&edits);
        self.commit(edits, carets, EditKind::Other)
    }

    /// The carets as they are after `edits`: each selection keeps its extent (its start stays put when an edit begins exactly there).
    fn mapped_carets(&self, edits: &[Edit]) -> Vec<Caret> {
        self.carets
            .iter()
            .map(|c| {
                let (start, end) = if c.is_empty() {
                    let at = map_offset(edits, c.start(), Bias::After);
                    (at, at)
                } else {
                    (map_offset(edits, c.start(), Bias::Before), map_offset(edits, c.end(), Bias::After))
                };
                let forward = c.sel.head >= c.sel.anchor;
                Caret { sel: if forward { Selection { anchor: start, head: end } } else { Selection { anchor: end, head: start } }, goal: None }
            })
            .collect()
    }

    // --- clipboard -----------------------------------------------------------------------------------------------------------

    /// What to copy: the selections, one after another with line breaks between; with no selection anywhere, the whole lines the carets
    /// are on.
    pub fn copy_text(&self) -> String {
        let eol = self.settings.eol.as_str();
        if self.has_selection() {
            let parts: Vec<String> =
                self.carets.iter().filter(|c| !c.is_empty()).filter_map(|c| self.buffer.slice(c.start()..c.end()).ok()).collect();
            return parts.join(eol);
        }
        let mut lines: Vec<usize> = self.carets.iter().map(|c| self.buffer.line_of(c.head())).collect();
        lines.dedup();
        let mut out = String::new();
        for line in lines {
            if let Some(range) = self.buffer.line_range_with_break(line) {
                out.push_str(&self.buffer.slice(range).unwrap_or_default());
                if !out.ends_with('\n') {
                    out.push_str(eol);
                }
            }
        }
        out
    }

    /// Cut: [`Self::copy_text`], then remove what was copied.
    pub fn cut(&mut self) -> String {
        let text = self.copy_text();
        if self.has_selection() {
            self.replace_at_carets(EditKind::Other, |_, c| (!c.is_empty()).then(|| Edit::delete(c.start()..c.end())));
        } else {
            let mut lines: Vec<usize> = self.carets.iter().map(|c| self.buffer.line_of(c.head())).collect();
            lines.dedup();
            let edits = coalesce(lines.into_iter().filter_map(|l| self.buffer.line_range_with_break(l)).map(Edit::delete).collect());
            let carets = self
                .carets
                .iter()
                .map(|c| Caret::at(map_offset(&edits, self.buffer.line_start(self.buffer.line_of(c.head())).unwrap_or(0), Bias::Before)))
                .collect();
            self.commit(edits, carets, EditKind::Other);
        }
        text
    }

    // --- history and outside edits -------------------------------------------------------------------------------------------

    pub fn undo(&mut self) -> bool {
        let result = self.history.undo_applied(&mut self.buffer);
        self.after_history(result)
    }

    pub fn redo(&mut self) -> bool {
        let result = self.history.redo_applied(&mut self.buffer);
        self.after_history(result)
    }

    fn after_history(&mut self, result: Result<Option<xmlcraft_text::Applied>, xmlcraft_text::TextError>) -> bool {
        match result {
            Ok(Some(applied)) => {
                self.version += 1;
                // Which lines an undo touched is not known here: everything counts as changed.
                self.log_change(0);
                self.remap_folds(&applied.edits);
                self.set_selections(&applied.selections);
                true
            }
            _ => false,
        }
    }

    /// Apply edits made by something else (the formatter, a quick fix, Replace All) as one undoable step. They are in the coordinates of
    /// the text as it is now and must not overlap; otherwise nothing changes and `false` is returned. The carets are carried along.
    pub fn apply_edits(&mut self, edits: &[Edit]) -> bool {
        let mut sorted = edits.to_vec();
        sorted.sort_by_key(|e| (e.range.start, e.range.end));
        let valid = sorted.iter().all(|e| self.buffer.check_range(&e.range).is_ok()) && sorted.windows(2).all(|w| w[0].range.end <= w[1].range.start);
        if !valid || sorted.is_empty() {
            return false;
        }
        let carets = self.mapped_carets(&sorted);
        self.commit(sorted, carets, EditKind::Other)
    }
}

/// The dominant line ending of `text`, or `fallback` when it has no line break.
fn line_ending_of(text: &str, fallback: Eol) -> Eol {
    let counts = eol::LineEndings::count(text);
    if counts.total() == 0 { fallback } else { counts.dominant() }
}

/// Merge edits that overlap (only deletions can: a word deletion by one caret may reach into the range another deletes). The input
/// need not be sorted; the output is ascending and disjoint.
pub(crate) fn coalesce(mut edits: Vec<Edit>) -> Vec<Edit> {
    edits.sort_by_key(|e| (e.range.start, e.range.end));
    let mut out: Vec<Edit> = Vec::with_capacity(edits.len());
    for edit in edits {
        match out.last_mut() {
            Some(last) if edit.range.start < last.range.end => {
                last.range.end = last.range.end.max(edit.range.end);
                last.text.push_str(&edit.text);
            }
            _ => out.push(edit),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(text: &str, caret: usize) -> Editor {
        let mut e = Editor::new(text);
        e.set_selection(caret, caret);
        e
    }

    fn at(e: &Editor) -> (String, Vec<(usize, usize)>) {
        (e.text(), e.carets().iter().map(|c| (c.sel.anchor, c.sel.head)).collect())
    }

    #[test]
    fn undo_and_redo_give_back_the_primary_caret() {
        let mut e = Editor::new("one two three");
        e.set_selection(0, 0);
        e.place_caret(8, true);
        e.place_caret(4, true); // the middle one is the primary
        assert_eq!((e.carets().len(), e.primary_index(), e.primary().head()), (3, 1, 4));
        e.type_text("-");
        assert_eq!(e.text(), "-one -two -three");
        e.undo();
        assert_eq!(e.text(), "one two three");
        assert_eq!((e.carets().len(), e.primary_index(), e.primary().head()), (3, 1, 4), "the middle caret again");
        e.redo();
        assert_eq!((e.text().as_str(), e.primary_index(), e.primary().head()), ("-one -two -three", 1, 6), "and after redo");
    }

    #[test]
    fn new_lines_use_the_line_ending_of_the_text() {
        let mut e = Editor::new("a\r\nb\r\nc");
        e.set_selection(1, 1);
        e.insert_newline();
        assert_eq!(e.text(), "a\r\n\r\nb\r\nc", "a CRLF document gets CRLF");
        let mut e = Editor::new("a\nb");
        e.set_selection(1, 1);
        e.insert_newline();
        assert_eq!(e.text(), "a\n\nb");
        // Pasted text is converted to it, and loading another document switches to its ending.
        let mut e = Editor::new("x\r\ny");
        e.set_selection(0, 0);
        e.paste("1\n2");
        assert_eq!(e.text(), "1\r\n2x\r\ny");
        e.load("p\nq");
        e.set_selection(1, 1);
        e.insert_newline();
        assert_eq!(e.text(), "p\n\nq");
        e.load("no break");
        e.set_selection(2, 2);
        e.insert_newline();
        assert_eq!(e.text(), "no\n break", "no line break keeps the current ending");
    }

    #[test]
    fn typing_replaces_the_selection_and_moves_the_caret() {
        let mut e = ed("hello world", 5);
        e.type_text(",");
        assert_eq!(at(&e), ("hello, world".to_owned(), vec![(6, 6)]));
        e.set_selection(7, 12);
        e.type_text("there");
        assert_eq!(at(&e), ("hello, there".to_owned(), vec![(12, 12)]));
        assert!(!e.type_text(""));
    }

    #[test]
    fn backspace_and_delete_take_whole_clusters() {
        let mut e = ed("ae\u{301}b", 4);
        e.backspace();
        assert_eq!(e.text(), "ab", "the accent goes with its letter");
        let mut e = ed("a\u{1F468}\u{200D}\u{1F469}b", 1);
        e.delete_forward();
        assert_eq!(e.text(), "ab");
        let mut e = ed("a\r\nb", 3);
        e.backspace();
        assert_eq!(e.text(), "ab", "a CRLF is one character");
        let mut e = ed("ab", 0);
        assert!(!e.backspace(), "nothing before the start");
        let mut e = ed("ab", 2);
        assert!(!e.delete_forward(), "nothing after the end");
        e.set_selection(0, 2);
        e.delete_forward();
        assert_eq!(at(&e), (String::new(), vec![(0, 0)]));
    }

    #[test]
    fn typing_merges_into_one_undo_step_and_undo_restores_the_caret() {
        let mut e = ed("", 0);
        for (i, c) in "abc".chars().enumerate() {
            e.set_time(i as f64 * 0.1);
            e.type_text(&c.to_string());
        }
        assert_eq!(e.text(), "abc");
        assert!(e.undo());
        assert_eq!(at(&e), (String::new(), vec![(0, 0)]));
        assert!(e.redo());
        assert_eq!(at(&e), ("abc".to_owned(), vec![(3, 3)]));
        assert!(!e.redo());
    }

    #[test]
    fn newline_keeps_the_indentation_up_to_the_caret() {
        let mut e = Editor::new("  abc\n");
        e.set_selection(5, 5);
        e.insert_newline();
        assert_eq!(at(&e), ("  abc\n  \n".to_owned(), vec![(8, 8)]));
        let mut e = ed("\t\tx", 1);
        e.insert_newline();
        assert_eq!(at(&e), ("\t\n\t\tx".to_owned(), vec![(3, 3)]), "only the indentation before the caret is copied");
        let mut e = Editor::with_settings("a", Settings { eol: Eol::CrLf, ..Settings::default() });
        e.set_selection(1, 1);
        e.insert_newline();
        assert_eq!(e.text(), "a\r\n", "the document's line ending");
    }

    #[test]
    fn several_carets_edit_together_and_undo_together() {
        let mut e = Editor::new("one\ntwo\nthree");
        e.set_selections(&[Selection::caret(3), Selection::caret(7), Selection::caret(13)]);
        e.type_text("!");
        assert_eq!(at(&e), ("one!\ntwo!\nthree!".to_owned(), vec![(4, 4), (9, 9), (16, 16)]));
        e.backspace();
        assert_eq!(e.text(), "one\ntwo\nthree");
        e.undo();
        assert_eq!(e.text(), "one!\ntwo!\nthree!");
        e.undo();
        assert_eq!(at(&e), ("one\ntwo\nthree".to_owned(), vec![(3, 3), (7, 7), (13, 13)]));
    }

    #[test]
    fn adjacent_carets_do_not_collide() {
        let mut e = Editor::new("abcdef");
        e.set_selections(&[Selection::caret(2), Selection::caret(3), Selection::caret(4)]);
        e.backspace();
        assert_eq!(at(&e), ("aef".to_owned(), vec![(1, 1)]), "the three deletions join; their carets merge");
        let mut e = Editor::new("abcdef");
        e.set_selections(&[Selection::caret(2), Selection::caret(3)]);
        e.type_text("XY");
        assert_eq!(at(&e), ("abXYcXYdef".to_owned(), vec![(4, 4), (7, 7)]));
    }

    #[test]
    fn word_deletions_that_reach_into_each_other_merge() {
        // Backward: the caret at 5 deletes `bb` (3..5) and the one at 8 deletes `cc`: separate.
        let mut e = Editor::new("aa bb cc");
        e.set_selections(&[Selection::caret(5), Selection::caret(8)]);
        e.delete_word_backward();
        assert_eq!(e.text(), "aa  ");
        // Two carets in one word delete overlapping ranges (3..4 and 3..5), which become one.
        let mut e = Editor::new("aa bb cc");
        e.set_selections(&[Selection::caret(4), Selection::caret(5)]);
        e.delete_word_backward();
        assert_eq!(at(&e), ("aa  cc".to_owned(), vec![(3, 3)]), "one deletion, one caret");
        // Forward, likewise (0..2 and 1..2).
        let mut e = Editor::new("aa bb");
        e.set_selections(&[Selection::caret(0), Selection::caret(1)]);
        e.delete_word_forward();
        assert_eq!(at(&e), (" bb".to_owned(), vec![(0, 0)]));
    }

    #[test]
    fn deleting_around_the_caret_for_an_input_method() {
        let mut e = ed("abcdef", 3);
        e.delete_surrounding(2, 1);
        assert_eq!(at(&e), ("aef".to_owned(), vec![(1, 1)]));
        let mut e = ed("日本語", 3);
        e.delete_surrounding(99, 99);
        assert_eq!(e.text(), "", "past the ends: stops at them");
        let mut e = ed("ab", 1);
        assert!(!e.delete_surrounding(0, 0));
    }

    #[test]
    fn deleting_to_the_line_edges() {
        let mut e = ed("abc def\nxyz", 4);
        e.delete_to_line_start();
        assert_eq!(at(&e), ("def\nxyz".to_owned(), vec![(0, 0)]));
        let mut e = ed("abc def\nxyz", 4);
        e.delete_to_line_end();
        assert_eq!(e.text(), "abc \nxyz");
        e.delete_to_line_end();
        assert_eq!(e.text(), "abc xyz", "at the end of a line it takes the line break");
    }

    #[test]
    fn movement_collapses_extends_and_keeps_the_goal_column() {
        let mut e = ed("abcdef\nxy\nabcdef", 5);
        e.move_carets(Motion::Down, false);
        assert_eq!(e.primary().head(), 9, "the short line's end");
        e.move_carets(Motion::Down, false);
        assert_eq!(e.primary().head(), 15, "the goal column 5 is back");
        e.move_carets(Motion::Up, true);
        assert_eq!((e.primary().sel.anchor, e.primary().head()), (15, 9));
        e.move_carets(Motion::Left, false);
        assert_eq!(at(&e).1, vec![(9, 9)], "Left collapses to the selection's start");
        e.move_carets(Motion::LineStart, false);
        assert_eq!(e.primary().head(), 7);
        e.move_carets(Motion::DocEnd, true);
        assert_eq!((e.primary().sel.anchor, e.primary().head()), (7, 16));
        e.move_carets(Motion::Down, false);
        assert_eq!(e.primary().head(), 16, "Down on the last line goes to the end");
        e.move_carets(Motion::PageUp(99), false);
        assert_eq!(e.primary().head(), 0);
    }

    #[test]
    fn selecting_by_mouse_gestures() {
        let mut e = Editor::new("one two\nthree");
        e.select_word_at(5, false);
        assert_eq!(at(&e).1, vec![(4, 7)]);
        e.select_line_at(5, false);
        assert_eq!(at(&e).1, vec![(0, 8)]);
        e.place_caret(10, false);
        e.place_caret(2, true);
        assert_eq!(at(&e).1, vec![(2, 2), (10, 10)]);
        e.extend_selection_to(0);
        assert_eq!(e.primary().sel, Selection { anchor: 2, head: 0 }, "the primary is the one just added");
        e.keep_primary_only();
        assert_eq!(e.carets().len(), 1);
        e.select_all();
        assert_eq!(at(&e).1, vec![(0, 13)]);
        e.place_caret(999, false);
        assert_eq!(at(&e).1, vec![(13, 13)], "clamped to the end");
    }

    #[test]
    fn tab_indents_to_the_next_stop_or_the_selected_lines() {
        let mut e = ed("ab", 2);
        e.indent();
        assert_eq!(e.text(), "ab  ");
        let mut e = ed("abc", 3);
        e.indent();
        assert_eq!(e.text(), "abc ", "to the next stop of two");
        let mut e = Editor::with_settings("x", Settings { use_tabs: true, ..Settings::default() });
        e.indent();
        assert_eq!(e.text(), "\tx");
        let mut e = Editor::new("a\n\nb\nc");
        e.set_selection(0, 5);
        e.indent();
        assert_eq!(at(&e), ("  a\n\n  b\nc".to_owned(), vec![(0, 9)]), "blank lines stay blank; a selection ending at a line start leaves that line");
        e.outdent();
        assert_eq!(e.text(), "a\n\nb\nc");
        e.outdent();
        assert_eq!(e.text(), "a\n\nb\nc", "nothing to remove");
        let mut e = Editor::new("a\n\nb\nc");
        e.set_selection(0, 6);
        e.indent();
        assert_eq!(e.text(), "  a\n\n  b\n  c", "ending inside the last line touches it");
        let mut e = Editor::new("\t\tx\n   y");
        e.set_selection(0, 9);
        e.outdent();
        assert_eq!(e.text(), "\tx\n y", "a tab, or up to one level of spaces");
    }

    #[test]
    fn copy_cut_and_paste() {
        let mut e = Editor::new("one two three");
        e.set_selections(&[Selection { anchor: 0, head: 3 }, Selection { anchor: 8, head: 13 }]);
        assert_eq!(e.copy_text(), "one\nthree");
        let cut = e.cut();
        assert_eq!(cut, "one\nthree");
        assert_eq!(e.text(), " two ");
        e.undo();
        assert_eq!(e.text(), "one two three");
        let mut e = ed("a\nb\nc", 2);
        assert_eq!(e.copy_text(), "b\n", "no selection: the whole line");
        assert_eq!(e.cut(), "b\n");
        assert_eq!(e.text(), "a\nc");
        let e = ed("ab", 1);
        assert_eq!(e.copy_text(), "ab\n", "the last line gets a line break");
    }

    #[test]
    fn paste_converts_line_endings_and_distributes_lines() {
        let mut e = Editor::with_settings("", Settings { eol: Eol::CrLf, ..Settings::default() });
        e.paste("a\nb\rc");
        assert_eq!(e.text(), "a\r\nb\r\nc");
        let mut e = Editor::new("x\nx\nx");
        e.set_selections(&[Selection::caret(1), Selection::caret(3), Selection::caret(5)]);
        e.paste("1\n2\n3");
        assert_eq!(e.text(), "x1\nx2\nx3", "as many lines as carets: one each");
        let mut e = Editor::new("x\nx");
        e.set_selections(&[Selection::caret(1), Selection::caret(3)]);
        e.paste("1\n2\n3");
        assert_eq!(e.text(), "x1\n2\n3\nx1\n2\n3", "otherwise the whole text at every caret");
        assert!(!e.paste(""));
    }

    #[test]
    fn outside_edits_carry_the_carets_and_are_one_undo_step() {
        let mut e = Editor::new("<a><b/></a>");
        e.set_selections(&[Selection::caret(3), Selection::caret(7)]);
        assert!(e.apply_edits(&[Edit::insert(0, "\n"), Edit::replace(3..3, "  "), Edit::delete(4..5)]));
        assert_eq!(e.text(), "\n<a>  </></a>", "a line break at 0, two spaces at 3, and the `b` (4..5) gone");
        assert_eq!(at(&e).1, vec![(6, 6), (9, 9)], "the carets are carried along");
        e.undo();
        assert_eq!(e.text(), "<a><b/></a>");
        assert_eq!(at(&e).1, vec![(3, 3), (7, 7)]);
        assert!(!e.apply_edits(&[Edit::delete(0..5), Edit::delete(3..6)]), "overlapping edits are refused");
        assert!(!e.apply_edits(&[Edit::delete(0..99)]), "so is a range outside the text");
        assert_eq!(e.text(), "<a><b/></a>");
    }

    #[test]
    fn the_first_changed_line_is_remembered_per_version() {
        let mut e = Editor::new("a\nb\nc\nd");
        let v0 = e.version();
        assert_eq!(e.first_changed_line_since(v0), None);
        e.set_selection(5, 5);
        e.type_text("x");
        let v1 = e.version();
        assert_eq!(e.first_changed_line_since(v0), Some(2));
        e.set_selection(2, 2);
        e.type_text("y");
        assert_eq!(e.first_changed_line_since(v1), Some(1), "only the changes since v1");
        assert_eq!(e.first_changed_line_since(v0), Some(1), "the earliest of both");
        e.set_selection(0, 0);
        e.delete_forward();
        assert_eq!(e.first_changed_line_since(e.version() - 1), Some(0));
        let v = e.version();
        e.undo();
        assert_eq!(e.first_changed_line_since(v), Some(0), "undo: everything counts");
        // Far more changes than are remembered: the answer is the whole document.
        let start = e.version();
        for _ in 0..200 {
            e.set_selection(7, 7);
            e.type_text("z");
        }
        assert_eq!(e.first_changed_line_since(start), Some(0));
        assert_eq!(e.first_changed_line_since(e.version()), None);
    }

    #[test]
    fn loading_resets_everything() {
        let mut e = ed("abc", 3);
        e.type_text("d");
        let version = e.version();
        e.load("new text");
        assert_eq!(at(&e), ("new text".to_owned(), vec![(0, 0)]));
        assert!(!e.can_undo() && e.version() > version);
    }

    #[test]
    fn a_stale_selection_cannot_break_anything() {
        let mut e = Editor::new("é");
        e.set_selection(1, 99);
        assert_eq!(at(&e).1, vec![(0, 2)], "clamped onto character boundaries");
        e.set_selections(&[]);
        assert_eq!(e.carets().len(), 1, "there is always a caret");
    }

    #[test]
    fn offsets_map_through_edits() {
        let edits = [Edit::insert(2, "XX"), Edit::delete(5..8), Edit::replace(10..12, "abc")];
        assert_eq!(map_offset(&edits, 0, Bias::After), 0);
        assert_eq!(map_offset(&edits, 2, Bias::After), 4);
        assert_eq!(map_offset(&edits, 2, Bias::Before), 2);
        assert_eq!(map_offset(&edits, 5, Bias::After), 7);
        assert_eq!(map_offset(&edits, 6, Bias::After), 7, "inside a deletion: its start");
        assert_eq!(map_offset(&edits, 8, Bias::After), 7);
        assert_eq!(map_offset(&edits, 11, Bias::After), 12, "inside a replacement: the end of the new text (10 - 1 + 3)");
        assert_eq!(map_offset(&edits, 12, Bias::After), 12, "after everything: shifted by +2 - 3 + 1");
        assert_eq!(map_offset(&edits, 20, Bias::After), 20);
    }
}
