//! The Outline panel's body: the element tree (or the members of a data document) of the document being shown, compact and
//! collapsible. A slim toolbar holds the filter and Collapse All / Expand All; each row has its disclosure chevron, a glyph for what it
//! is, its name, a hint from its `id`, `name` or `key`, and a look at its text (or, collapsed, how many items are under it); a dot at
//! the right edge says a problem lies inside it. The outline follows the caret, opening what hides it. The Workbench
//! ([`crate::workbench`]) hosts it with the other panels.
//!
//! What is collapsed is kept per document ([`OutlineState`], in the document's view, dropped when it closes), by the *path* of an
//! item: its parent's path, its place among its parent's items, and its name. A path survives typing elsewhere (offsets move; paths do
//! not), so nothing has to be remapped when a new analysis arrives.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};

use egui::{Color32, Rect, Sense, pos2, vec2};
use xmlcraft_engine::analysis::{Analysis, OutlineItem, OutlineKind, Skipped};
use xmlcraft_engine::elements;
use xmlcraft_engine::language::Language;
use xmlcraft_syntax::Severity;

use crate::i18n::tl;
use crate::kit::{self, Tone};
use crate::theme::{self, Tokens};
use crate::{XmlCraftApp, icons, widgets};

/// Height of a row.
pub(crate) const ROW: f32 = 20.0;
/// Height of the toolbar above the rows, and of the controls in it.
const TOOLBAR: f32 = 24.0;
const CONTROL: f32 = 20.0;
/// The panel's margin at the left and the right.
pub(crate) const MARGIN: f32 = 8.0;
/// Size of the text of a row.
pub(crate) const TEXT: f32 = 11.5;
/// Indent of depth 0, and of each level below it.
const INDENT: f32 = 6.0;
const STEP: f32 = 12.0;
/// The deepest level that is indented further.
const MAX_INDENT_DEPTH: usize = 20;
/// Diameter of a problem dot.
const DOT: f32 = 5.0;
/// Documents with at most this many items open fully; larger ones open to depth 2.
const OPEN_FULLY: usize = 400;
/// The depth a large document is opened to.
const OPEN_DEPTH: usize = 2;

/// The scroll area of the rows.
const ROWS_ID: &str = "xmlcraft-outline-rows";
/// Where the filter field is (Entry Helpers' has its own id).
const FILTER_ID: &str = "xmlcraft-outline-filter";

/// One row of the list: an index into the analysis's outline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub index: usize,
    /// Shown only because something under it matches the filter (drawn muted).
    pub context: bool,
}

/// The worst problem inside an item (and, while it is collapsed, inside what it hides).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Marker {
    Warning,
    Error,
}

/// The rows worked out for one filter, document, analysis, set of problems and collapse state; kept between frames so that 50,000
/// items are not walked again every frame.
struct Rows {
    key: RowsKey,
    rows: Vec<Row>,
    /// Aligned with `rows`.
    markers: Vec<Option<Marker>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RowsKey {
    filter: String,
    version: u64,
    generation: u64,
    /// The validation the markers come from (its version and how many problems it has), if any.
    validation: Option<(u64, usize)>,
}

/// What the Outline keeps for one document.
#[derive(Default)]
pub(crate) struct OutlineState {
    /// The paths ([`keys`]) of the collapsed items.
    collapsed: HashSet<u64>,
    /// The default (open fully, or to depth 2) has been applied.
    initialised: bool,
    /// Counts the changes of `collapsed`: what the cached rows are keyed by.
    generation: u64,
    /// The paths of the items of the analysis of this version.
    keys: Vec<u64>,
    keys_version: Option<u64>,
    rows: Option<Rows>,
    /// The item the caret was last seen in (its path): a change is the caret moving to another element.
    caret: Option<u64>,
    /// The item that has (or is to get) the keyboard, and whether it is to be given the keyboard and scrolled to at the next frame.
    cursor: Option<u64>,
    focus_pending: bool,
    /// Scroll the row of this path into view at the next frame (when it is outside the visible part).
    reveal: Option<u64>,
}

impl OutlineState {
    fn set_collapsed(&mut self, key: u64, collapsed: bool) {
        let changed = if collapsed { self.collapsed.insert(key) } else { self.collapsed.remove(&key) };
        if changed {
            self.generation = self.generation.wrapping_add(1);
        }
    }

    fn is_collapsed(&self, key: u64) -> bool {
        self.collapsed.contains(&key)
    }

    /// The paths of `analysis`'s items, worked out once per version.
    fn refresh_keys(&mut self, analysis: &Analysis) {
        if self.keys_version != Some(analysis.version) || self.keys.len() != analysis.outline.len() {
            self.keys = keys(&analysis.outline);
            self.keys_version = Some(analysis.version);
        }
    }

    fn key(&self, index: usize) -> Option<u64> {
        self.keys.get(index).copied()
    }
}

/// The path of each item: a hash of its parent's path, its place among its parent's items and its name. One pass.
pub(crate) fn keys(outline: &[OutlineItem]) -> Vec<u64> {
    // (depth, path, how many items are under it so far)
    let mut open: Vec<(usize, u64, u32)> = Vec::new();
    let mut roots = 0u32;
    let mut out = Vec::with_capacity(outline.len());
    for item in outline {
        while open.last().is_some_and(|(depth, ..)| *depth >= item.depth) {
            open.pop();
        }
        let (parent, place) = match open.last_mut() {
            Some((_, path, count)) => {
                let place = *count;
                *count = count.saturating_add(1);
                (*path, place)
            }
            None => {
                let place = roots;
                roots = roots.saturating_add(1);
                (0, place)
            }
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (parent, place, item.name.as_str()).hash(&mut hasher);
        let key = hasher.finish();
        out.push(key);
        open.push((item.depth, key, 0));
    }
    out
}

/// What is collapsed when a document is first outlined: nothing in a small one, everything below depth 2 in a large one.
pub(crate) fn default_collapsed(outline: &[OutlineItem], keys: &[u64]) -> HashSet<u64> {
    if outline.len() <= OPEN_FULLY {
        return HashSet::new();
    }
    outline.iter().zip(keys).filter(|(item, _)| item.depth == OPEN_DEPTH && item.children > 0).map(|(_, key)| *key).collect()
}

/// Whether `item` is what `filter` (lower case, not empty) looks for: in its name, its hint or its preview.
fn matches(item: &OutlineItem, filter: &str) -> bool {
    item.name.to_lowercase().contains(filter)
        || item.label.as_ref().is_some_and(|l| l.to_lowercase().contains(filter))
        || item.preview.to_lowercase().contains(filter)
}

/// The rows to show. Without a filter: every item that no collapsed item hides. With one (lower case): the items that match, each
/// with the items around it (as context), whatever is collapsed. One pass.
pub(crate) fn visible_rows(outline: &[OutlineItem], keys: &[u64], collapsed: &HashSet<u64>, filter: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    if filter.is_empty() {
        let mut hidden_below: Option<usize> = None;
        for (index, item) in outline.iter().enumerate() {
            if let Some(depth) = hidden_below {
                if item.depth > depth {
                    continue;
                }
                hidden_below = None;
            }
            rows.push(Row { index, context: false });
            if item.children > 0 && keys.get(index).is_some_and(|k| collapsed.contains(k)) {
                hidden_below = Some(item.depth);
            }
        }
        return rows;
    }
    // The items around the one being looked at, and whether each has been shown.
    let mut open: Vec<(usize, bool)> = Vec::new();
    for (index, item) in outline.iter().enumerate() {
        while open.last().and_then(|(i, _)| outline.get(*i)).is_some_and(|o| o.depth >= item.depth) {
            open.pop();
        }
        let hit = matches(item, filter);
        if hit {
            for (i, shown) in open.iter_mut().filter(|(_, shown)| !*shown) {
                rows.push(Row { index: *i, context: true });
                *shown = true;
            }
            rows.push(Row { index, context: false });
        }
        open.push((index, hit));
    }
    rows
}

/// The rows' problem dots. A problem (where it starts) belongs to the innermost item it is in; it is shown on that item's row, or, when
/// a collapsed item hides that row, on the collapsed item's; an open item does not repeat what its rows show. With a filter, only the
/// rows that are shown carry their own. `problems` are sorted by offset; one pass over them and the items.
pub(crate) fn markers(outline: &[OutlineItem], rows: &[Row], problems: &[(usize, Marker)], filtering: bool) -> Vec<Option<Marker>> {
    // Which row stands for each item: its own, or that of the collapsed item that hides it.
    let mut row_of: Vec<Option<usize>> = vec![None; outline.len()];
    for (r, row) in rows.iter().enumerate() {
        if let Some(slot) = row_of.get_mut(row.index) {
            *slot = Some(r);
        }
    }
    if !filtering {
        let mut open: Vec<(usize, Option<usize>)> = Vec::new();
        for (index, item) in outline.iter().enumerate() {
            while open.last().is_some_and(|(depth, _)| *depth >= item.depth) {
                open.pop();
            }
            let own = row_of.get(index).copied().flatten();
            let row = own.or_else(|| open.last().and_then(|(_, r)| *r));
            if let Some(slot) = row_of.get_mut(index) {
                *slot = row;
            }
            open.push((item.depth, row));
        }
    }
    let mut out = vec![None; rows.len()];
    // The items open at the problem's offset (ranges nest), innermost last.
    let mut open: Vec<usize> = Vec::new();
    let mut next = 0usize;
    for &(offset, marker) in problems {
        while let Some(item) = outline.get(next).filter(|item| item.range.start <= offset) {
            while open.last().and_then(|&i| outline.get(i)).is_some_and(|o| o.range.end < item.range.start) {
                open.pop();
            }
            open.push(next);
            next += 1;
        }
        while open.last().and_then(|&i| outline.get(i)).is_some_and(|o| o.range.end < offset) {
            open.pop();
        }
        let Some(&innermost) = open.last() else { continue };
        if let Some(slot) = row_of.get(innermost).copied().flatten().and_then(|r| out.get_mut(r)) {
            *slot = (*slot).max(Some(marker));
        }
    }
    out
}

/// Open the items around the item at `index`; whether anything changed. One walk back from it.
pub(crate) fn reveal(outline: &[OutlineItem], keys: &[u64], collapsed: &mut HashSet<u64>, index: usize) -> bool {
    let Some(mut depth) = outline.get(index).map(|i| i.depth) else { return false };
    let mut changed = false;
    for j in (0..index).rev() {
        if depth == 0 {
            break;
        }
        let Some(item) = outline.get(j) else { continue };
        if item.depth < depth {
            depth = item.depth;
            if let Some(key) = keys.get(j) {
                changed |= collapsed.remove(key);
            }
        }
    }
    changed
}

/// The indexes of the item at `index` and of every item under it.
fn subtree(outline: &[OutlineItem], index: usize) -> std::ops::Range<usize> {
    let Some(depth) = outline.get(index).map(|i| i.depth) else { return index..index };
    let end = outline.get(index + 1..).and_then(|rest| rest.iter().position(|i| i.depth <= depth)).map_or(outline.len(), |n| index + 1 + n);
    index..end
}

/// The problems of the document being shown that are about the text the analysis is about, by where they start: what the text has
/// as XML (or its own format) and, when a validation of the same text is in, what the grammar says.
fn problems_of(app: &XmlCraftApp, analysis: &Analysis) -> (Vec<Problem>, Option<(u64, usize)>) {
    let marker = |s: Severity| match s {
        Severity::Error => Some(Marker::Error),
        Severity::Warning => Some(Marker::Warning),
        Severity::Info => None,
    };
    let mut out: Vec<(usize, Marker)> = analysis.diagnostics.iter().filter_map(|d| Some((d.span.start, marker(d.severity())?))).collect();
    let validation = app.active_validation().filter(|v| v.version == analysis.version);
    if let Some(v) = validation {
        out.extend(v.problems.iter().filter(|p| p.file.is_none()).filter_map(|p| Some((p.diagnostic.span.start, marker(p.diagnostic.severity())?))));
    }
    out.sort_unstable();
    (out, validation.map(|v| (v.version, v.problems.len())))
}

/// Where a problem starts, and how bad it is.
pub(crate) type Problem = (usize, Marker);

/// What a row's context menu asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MenuAction {
    Command(&'static str),
    /// Select the element, then run the command (Wrap in Tag wraps the selection).
    CommandOnSelection(&'static str),
    CollapseChildren,
    ExpandChildren,
}

/// What happened in the list this frame.
#[derive(Default)]
struct Asked {
    jump: Option<usize>,
    select: Option<usize>,
    toggle: Option<(usize, bool)>,
    menu: Option<(usize, MenuAction)>,
    cursor: Option<usize>,
    /// Asked with the keyboard: the row keeps it.
    keyboard: bool,
}

/// One row of the outline as a test sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineRowInfo {
    /// The accessible label: the name and the hint.
    pub label: String,
    pub depth: usize,
    pub collapsed: bool,
    /// "error" or "warning" when a dot is shown.
    pub marker: Option<&'static str>,
}

impl XmlCraftApp {
    /// The rows the Outline showed at the last frame (for tests).
    pub fn outline_rows(&self) -> Vec<OutlineRowInfo> {
        let (Some(doc), Some(analysis)) = (self.session.active_document(), self.active_analysis()) else { return Vec::new() };
        let Some(state) = self.views.get(&doc.id()).map(|v| &v.outline) else { return Vec::new() };
        let Some(rows) = &state.rows else { return Vec::new() };
        rows.rows
            .iter()
            .zip(&rows.markers)
            .filter_map(|(row, marker)| {
                let item = analysis.outline.get(row.index)?;
                Some(OutlineRowInfo {
                    label: row_label(item),
                    depth: item.depth,
                    collapsed: item.children > 0 && state.key(row.index).is_some_and(|k| state.is_collapsed(k)),
                    marker: marker.map(|m| if m == Marker::Error { "error" } else { "warning" }),
                })
            })
            .collect()
    }
}

/// The accessible name of a row: the name and the hint (what tests and screen readers find it by).
fn row_label(item: &OutlineItem) -> String {
    match &item.label {
        Some(label) => format!("{} {label}", item.name),
        None => item.name.clone(),
    }
}

pub(crate) fn body(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    let Some(doc) = app.session.active_document() else {
        widgets::empty_state(ui, 16.0, tl!("No document open"), tl!("The element tree of the open file appears here."));
        return;
    };
    let (caret, document, language) = (doc.editor().primary().head(), doc.id(), doc.language());
    let toolbar = toolbar(ui, &mut app.outline_filter);
    let filter = app.outline_filter.trim().to_lowercase();
    let Some(analysis) = app.active_analysis() else {
        widgets::empty_state(ui, 8.0, tl!("Reading the document…"), "");
        return;
    };
    match analysis.skipped {
        Some(Skipped::TooLarge) => {
            return widgets::empty_state(ui, 8.0, tl!("Not checked"), tl!("The document is too large for an outline while typing."));
        }
        Some(Skipped::Failed) => return widgets::empty_state(ui, 8.0, tl!("The check failed"), tl!("This is a bug. The document is unchanged.")),
        None => {}
    }
    if analysis.outline.is_empty() {
        widgets::empty_state(ui, 8.0, tl!("No elements"), tl!("The element tree appears once the document has a root element."));
        return;
    }
    let (problems, validation) = problems_of(app, analysis);
    // The analysis is borrowed from the app; the state is in the document's view, next to it.
    let Some(view) = app.views.get_mut(&document) else { return };
    let Some(analysis) = view.analysis.as_ref() else { return };
    let state = &mut view.outline;
    let outline = &analysis.outline;
    state.refresh_keys(analysis);
    if !state.initialised {
        state.collapsed = default_collapsed(outline, &state.keys);
        state.initialised = true;
        state.generation = state.generation.wrapping_add(1);
    }
    match toolbar {
        Some(true) => {
            for (item, key) in outline.iter().zip(&state.keys) {
                if item.children > 0 {
                    state.collapsed.insert(*key);
                }
            }
            state.generation = state.generation.wrapping_add(1);
        }
        Some(false) => {
            state.collapsed.clear();
            state.generation = state.generation.wrapping_add(1);
        }
        None => {}
    }
    // Follow the caret: when it is in another element than before (not because of a click here), open what hides it and show its row.
    let current = elements::innermost(outline, caret);
    let current_key = current.and_then(|i| state.key(i));
    if current_key != state.caret {
        state.caret = current_key;
        if let Some(index) = current {
            if filter.is_empty() && reveal(outline, &state.keys, &mut state.collapsed, index) {
                state.generation = state.generation.wrapping_add(1);
            }
            state.reveal = current_key;
        }
    }
    let key = RowsKey { filter: filter.clone(), version: analysis.version, generation: state.generation, validation };
    let rows = match state.rows.take().filter(|r| r.key == key) {
        Some(rows) => rows,
        None => {
            let rows = visible_rows(outline, &state.keys, &state.collapsed, &filter);
            let markers = markers(outline, &rows, &problems, !filter.is_empty());
            Rows { key, rows, markers }
        }
    };
    if analysis.outline_truncated {
        let t = Tokens::get(ui.ctx());
        egui::Frame::NONE.inner_margin(egui::Margin::symmetric(MARGIN as i8, 2)).show(ui, |ui| {
            ui.label(egui::RichText::new(tl!("Only the first elements are listed.")).font(theme::regular(TEXT)).color(t.text_faint));
        });
    }
    if rows.rows.is_empty() {
        widgets::empty_state(ui, 8.0, tl!("No match"), tl!("No element contains that."));
        state.rows = Some(rows);
        return;
    }
    let asked = list(ui, state, outline, &rows, current, language);
    state.rows = Some(rows);
    apply(app, document, asked);
}

/// The toolbar: the filter field, then Collapse All and Expand All. `Some(true)` when Collapse All was clicked, `Some(false)` for
/// Expand All.
fn toolbar(ui: &mut egui::Ui, filter: &mut String) -> Option<bool> {
    let mut asked = None;
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, TOOLBAR), Sense::hover());
    let inner = Rect::from_min_max(pos2(rect.left() + MARGIN, rect.top() + 2.0), pos2(rect.right() - MARGIN, rect.bottom() - 2.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::right_to_left(egui::Align::Center)), |ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if icons::button(ui, "list-tree", CONTROL, false, tl!("Expand All")).clicked() {
            asked = Some(false);
        }
        if icons::button(ui, "list", CONTROL, false, tl!("Collapse All")).clicked() {
            asked = Some(true);
        }
        ui.add_space(4.0);
        let field = (ui.available_width()).max(40.0);
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            compact_filter(ui, filter, egui::Id::unique(FILTER_ID), tl!("Filter elements"), field);
        });
    });
    asked
}

/// A compact filter field: 20 points high, a `filter` icon inside at the left, "Filter" as its placeholder and an × that clears it
/// when it holds something. `name` is what assistive technology (and a test) finds it by. Returns the text field's response.
pub(crate) fn compact_filter(ui: &mut egui::Ui, text: &mut String, id: egui::Id, name: &str, width: f32) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(width, CONTROL), Sense::hover());
    let focused = ui.memory(|m| m.has_focus(id));
    let edge = if focused { t.accent } else { t.border };
    ui.painter().rect(rect, egui::CornerRadius::same(5), t.field, egui::Stroke::new(1.0, edge), egui::StrokeKind::Inside);
    icons::paint(ui, Rect::from_center_size(pos2(rect.left() + 11.0, rect.center().y), vec2(12.0, 12.0)), "filter", 12.0, t.text_faint);
    let clear = !text.is_empty();
    let right = if clear { rect.right() - CONTROL } else { rect.right() - 4.0 };
    let field_rect = Rect::from_min_max(pos2(rect.left() + 20.0, rect.top() + 1.0), pos2(right.max(rect.left() + 20.0), rect.bottom() - 1.0));
    let response = ui.put(
        field_rect,
        egui::TextEdit::singleline(text)
            .id(id)
            .frame(egui::Frame::NONE)
            .margin(egui::Margin::symmetric(2, 2))
            .font(theme::regular(TEXT))
            .hint_text(tl!("Filter"))
            .desired_width(field_rect.width()),
    );
    widgets::name_for_assistive_technology(ui, &response, name);
    if clear {
        let x = Rect::from_min_size(pos2(rect.right() - CONTROL, rect.top()), vec2(CONTROL, CONTROL));
        let button = ui.put(x, |ui: &mut egui::Ui| icons::button(ui, "x", CONTROL - 2.0, false, tl!("Clear Filter")));
        if button.clicked() {
            text.clear();
        }
    }
    response
}

/// Draw the rows (only those in view) and collect what was asked.
fn list(ui: &mut egui::Ui, state: &mut OutlineState, outline: &[OutlineItem], rows: &Rows, current: Option<usize>, language: Language) -> Asked {
    let t = Tokens::get(ui.ctx());
    let mut asked = Asked::default();
    let filtering = !rows.key.filter.is_empty();
    let pointer_over = ui.rect_contains_pointer(ui.available_rect_before_wrap());
    // The row to bring into view: the caret's (not while the pointer is over the list), or the one given the keyboard.
    let target = if state.focus_pending { state.cursor } else { state.reveal.take().filter(|_| !pointer_over) };
    let target_row = target.and_then(|key| rows.rows.iter().position(|r| state.key(r.index) == Some(key)));
    // The row the keyboard was to go to is not shown (a filter hides it): it goes nowhere.
    if state.focus_pending && target_row.is_none() {
        state.focus_pending = false;
    }
    let mut area = egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt(ROWS_ID);
    // (Rows touch: no spacing between them, so row `r` starts at `r × ROW`.)
    ui.spacing_mut().item_spacing.y = 0.0;
    if let Some(r) = target_row {
        let offset =
            egui::scroll_area::State::load(ui.ctx(), ui.make_persistent_id(egui::IdSalt::new(ROWS_ID))).map_or(0.0, |s| s.clamped_offset().y);
        let (y, view) = (r as f32 * ROW, ui.available_height());
        if y < offset || y + ROW > offset + view {
            area = area.vertical_scroll_offset((y - view / 2.0).max(0.0));
        }
    }
    area.show_rows(ui, ROW, rows.rows.len(), |ui, range| {
        for r in range {
            let (Some(row), Some(item)) = (rows.rows.get(r), rows.rows.get(r).and_then(|row| outline.get(row.index))) else { continue };
            let key = state.key(row.index).unwrap_or_default();
            let collapsed = !filtering && item.children > 0 && state.is_collapsed(key);
            let marker = rows.markers.get(r).copied().flatten();
            let drawn = draw_row(ui, &t, item, *row, key, collapsed, filtering, Some(row.index) == current, marker);
            if state.focus_pending && state.cursor == Some(key) {
                drawn.row.request_focus();
                state.focus_pending = false;
            }
            if drawn.row.has_focus() {
                // (Not while the keyboard is on its way to another row.)
                if !state.focus_pending {
                    state.cursor = Some(key);
                }
                row_keys(ui, &drawn.row, row.index, r, rows, outline, collapsed, &mut asked);
            }
            if let Some(chevron) = &drawn.chevron
                && chevron.clicked()
            {
                let subtree = ui.input(|i| i.modifiers.alt);
                asked.toggle = Some((row.index, subtree));
            } else if drawn.row.double_clicked() {
                asked.select = Some(row.index);
            } else if drawn.row.clicked() {
                asked.jump = Some(row.index);
            }
            drawn.row.context_menu(|ui| {
                if let Some(action) = menu(ui, item, language) {
                    asked.menu = Some((row.index, action));
                    ui.close();
                }
            });
        }
    });
    asked
}

/// The keys of the row that has the keyboard: Up and Down move, Left closes or goes to the parent, Right opens or goes to the first
/// item under it, Enter jumps to it and gives the editor the keyboard.
#[allow(clippy::too_many_arguments)]
fn row_keys(ui: &egui::Ui, row: &egui::Response, index: usize, at: usize, rows: &Rows, outline: &[OutlineItem], collapsed: bool, asked: &mut Asked) {
    // The arrows are the row's, not egui's focus moves.
    ui.memory_mut(|m| m.set_focus_lock_filter(row.id, egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, ..Default::default() }));
    let pressed = |key| ui.input(|i| i.key_pressed(key));
    asked.keyboard |= [egui::Key::ArrowDown, egui::Key::ArrowUp, egui::Key::ArrowLeft, egui::Key::ArrowRight].into_iter().any(pressed);
    let row_at = |r: usize| rows.rows.get(r).map(|row| row.index);
    let Some(item) = outline.get(index) else { return };
    if pressed(egui::Key::ArrowDown) {
        asked.cursor = row_at(at + 1);
    } else if pressed(egui::Key::ArrowUp) {
        asked.cursor = at.checked_sub(1).and_then(row_at);
    } else if pressed(egui::Key::ArrowLeft) {
        if item.children > 0 && !collapsed {
            asked.toggle = Some((index, false));
        } else {
            asked.cursor = elements::parent(outline, index);
        }
    } else if pressed(egui::Key::ArrowRight) {
        if collapsed {
            asked.toggle = Some((index, false));
        } else if item.children > 0 {
            asked.cursor = row_at(at + 1);
        }
    } else if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
        // (Consumed: the editor, which gets the keyboard and is drawn later in this frame, is not to type it.)
        asked.jump = Some(index);
    }
}

/// A row's context menu; what was chosen.
fn menu(ui: &mut egui::Ui, item: &OutlineItem, language: Language) -> Option<MenuAction> {
    let mut chosen = None;
    let mut entry = |ui: &mut egui::Ui, label: &str, action: MenuAction| {
        if ui.button(label).clicked() {
            chosen = Some(action);
        }
    };
    entry(ui, tl!("Select Element"), MenuAction::Command("xml.select_element"));
    if matches!(language, Language::Xml | Language::Html) {
        entry(ui, tl!("Copy XPath"), MenuAction::Command("xml.copy_xpath"));
    }
    if language == Language::Xml {
        ui.separator();
        entry(ui, tl!("Rename Element…"), MenuAction::Command("xml.rename_element"));
        entry(ui, tl!("Wrap in Tag"), MenuAction::CommandOnSelection("xml.wrap_in_tag"));
        entry(ui, tl!("Unwrap Element"), MenuAction::Command("xml.unwrap_element"));
    }
    if item.children > 0 {
        ui.separator();
        entry(ui, tl!("Collapse Children"), MenuAction::CollapseChildren);
        entry(ui, tl!("Expand Children"), MenuAction::ExpandChildren);
    }
    chosen
}

/// The responses of one row: the row, and its chevron when it has one.
struct DrawnRow {
    row: egui::Response,
    chevron: Option<egui::Response>,
}

#[allow(clippy::too_many_arguments)]
fn draw_row(
    ui: &mut egui::Ui,
    t: &Tokens,
    item: &OutlineItem,
    row: Row,
    key: u64,
    collapsed: bool,
    filtering: bool,
    current: bool,
    marker: Option<Marker>,
) -> DrawnRow {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
    let id = egui::Id::unique("xmlcraft-outline-row").with(key);
    let response = ui.interact(rect, id, Sense::click());
    let label = row_label(item);
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, current, label.clone()));
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    if current {
        painter.rect_filled(rect, 0.0, t.selected);
    } else if response.contains_pointer() {
        painter.rect_filled(rect, 0.0, t.hover);
    }
    if response.has_focus() {
        painter.rect_stroke(rect.shrink(0.5), 2.0, egui::Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
    }
    let depth = item.depth.min(MAX_INDENT_DEPTH);
    // The guide lines of the levels around it, through the middle of their chevrons.
    for level in 0..depth {
        let x = (rect.left() + INDENT + STEP * level as f32 + 6.0).round() + 0.5;
        painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], egui::Stroke::new(1.0, t.divider));
    }
    let x0 = rect.left() + INDENT + STEP * depth as f32;
    let cy = rect.center().y;
    let chevron_rect = Rect::from_center_size(pos2(x0 + 6.0, cy), vec2(12.0, ROW));
    let chevron = (item.children > 0 && !filtering).then(|| {
        let name = crate::i18n::fmt(if collapsed { tl!("Expand {name}") } else { tl!("Collapse {name}") }, &[("name", &label)]);
        let r = ui.interact(chevron_rect, id.with("chevron"), Sense::click());
        r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone()));
        r
    });
    if item.children > 0 {
        let icon = if collapsed { "chevron-right" } else { "chevron-down" };
        let tint = if chevron.as_ref().is_some_and(|c| c.hovered()) { t.text } else { t.icon };
        icons::paint(ui, chevron_rect, icon, 12.0, tint);
    }
    kind_glyph(&painter, t, item.kind, Rect::from_center_size(pos2(x0 + 20.0, cy), vec2(12.0, 12.0)), ui);
    let name_color = if row.context { t.text_muted } else { t.text };
    let name = painter.text(pos2(x0 + 30.0, cy), egui::Align2::LEFT_CENTER, &item.name, theme::medium(TEXT), name_color);
    let mut end = name.right();
    // A data document's hint is its value, which the preview shows.
    if let Some(hint) = item.label.as_deref().filter(|l| *l != item.preview) {
        let hint = painter.text(pos2(end + 6.0, cy), egui::Align2::LEFT_CENTER, hint, theme::regular(TEXT), t.text_faint);
        end = hint.right();
    }
    let right = rect.right() - MARGIN - if marker.is_some() { DOT + 6.0 } else { 0.0 };
    let (tail, color) = if collapsed && item.children > 0 { (item.children.to_string(), t.text_faint) } else { (item.preview.clone(), t.text_muted) };
    let room = right - (end + 12.0);
    if !tail.is_empty() && room > 12.0 {
        let mut job = egui::text::LayoutJob::simple_singleline(tail, theme::regular(TEXT), color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(room);
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        painter.galley(pos2(right - galley.size().x, cy - galley.size().y / 2.0), galley, color);
    }
    if let Some(marker) = marker {
        let tone = if marker == Marker::Error { Tone::Bad } else { Tone::Warn };
        let (fill, _) = kit::tone_colors(t, tone);
        painter.circle_filled(pos2(rect.right() - MARGIN - DOT / 2.0, cy), DOT / 2.0, fill);
    }
    DrawnRow { row: theme::hand(response), chevron }
}

/// The small glyph of what an item is, in the icon colour: `<>` for an element that holds elements, a dot for one that holds text, a
/// ring for an empty one; `{}` for an object, `[]` for a list, a dot for a scalar.
fn kind_glyph(painter: &egui::Painter, t: &Tokens, kind: OutlineKind, rect: Rect, ui: &egui::Ui) {
    let color: Color32 = t.icon;
    let c = rect.center();
    match kind {
        OutlineKind::Element => icons::paint(ui, rect, "code-xml", 12.0, color),
        OutlineKind::Object => icons::paint(ui, rect, "braces", 12.0, color),
        OutlineKind::Array => {
            let stroke = egui::Stroke::new(1.25, color);
            // (Short arms, wide apart: two brackets, not a box.)
            for (side, x) in [(1.0f32, rect.left() + 1.5), (-1.0, rect.right() - 1.5)] {
                let (top, bottom) = (rect.top() + 1.5, rect.bottom() - 1.5);
                painter.line(vec![pos2(x + 1.75 * side, top), pos2(x, top), pos2(x, bottom), pos2(x + 1.75 * side, bottom)], stroke);
            }
        }
        OutlineKind::Empty => {
            painter.circle_stroke(c, 2.5, egui::Stroke::new(1.0, color));
        }
        OutlineKind::Leaf | OutlineKind::String | OutlineKind::Number | OutlineKind::Boolean | OutlineKind::Null => {
            painter.circle_filled(c, 2.0, color);
        }
    }
}

/// Carry out what the list asked for.
fn apply(app: &mut XmlCraftApp, document: u64, asked: Asked) {
    let Asked { jump, select, toggle, menu, cursor, keyboard } = asked;
    let Some(view) = app.views.get_mut(&document) else { return };
    let Some(analysis) = view.analysis.as_ref() else { return };
    let state = &mut view.outline;
    let outline = &analysis.outline;
    if let Some(index) = cursor {
        state.cursor = state.key(index);
        state.focus_pending = true;
    }
    if let Some((index, whole)) = toggle {
        let Some(key) = state.key(index) else { return };
        let close = !state.is_collapsed(key);
        let range = if whole { subtree(outline, index) } else { index..index + 1 };
        for i in range {
            if let (Some(item), Some(k)) = (outline.get(i), state.key(i))
                && item.children > 0
            {
                state.set_collapsed(k, close);
            }
        }
        if keyboard {
            state.cursor = Some(key);
            state.focus_pending = true;
        }
    }
    let mut command = None;
    let mut target = None;
    if let Some((index, action)) = menu {
        match action {
            MenuAction::CollapseChildren | MenuAction::ExpandChildren => {
                let close = action == MenuAction::CollapseChildren;
                for i in subtree(outline, index).skip(1) {
                    if let (Some(item), Some(k)) = (outline.get(i), state.key(i))
                        && item.children > 0
                    {
                        state.set_collapsed(k, close);
                    }
                }
                if let Some(k) = state.key(index) {
                    state.set_collapsed(k, false);
                }
            }
            MenuAction::Command(id) => {
                target = outline.get(index).map(|i| i.name_range.start..i.name_range.start);
                command = Some(id);
            }
            MenuAction::CommandOnSelection(id) => {
                target = outline.get(index).map(elements::element_range);
                command = Some(id);
            }
        }
        // The caret goes into the element: the outline is not to follow it there.
        state.caret = state.key(index);
    }
    let goto = jump.and_then(|i| Some((i, outline.get(i)?.name_range.start..outline.get(i)?.name_range.start)));
    let whole = select.and_then(|i| Some((i, elements::element_range(outline.get(i)?))));
    if let Some((index, _)) = goto.as_ref().or(whole.as_ref()) {
        state.caret = state.key(*index);
    }
    if let Some((_, range)) = whole.or(goto) {
        app.jump_to(range);
    }
    if let Some(range) = target {
        app.jump_to(range);
    }
    if let Some(id) = command {
        app.execute(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xmlcraft_engine::analysis::analyze;

    const LIBRARY: &str = "<library>\n  <book id=\"b1\">\n    <title>One</title>\n    <year>2019</year>\n  </book>\n  <book id=\"b2\">\n    \
                           <title>Two</title>\n  </book>\n  <shelf/>\n</library>";

    fn names(outline: &[OutlineItem], rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| {
                let item = &outline[r.index];
                let label = row_label(item);
                if r.context { format!("({label})") } else { label }
            })
            .collect()
    }

    #[test]
    fn collapsed_items_hide_what_is_under_them() {
        let a = analyze(LIBRARY, 1);
        let keys = keys(&a.outline);
        let mut collapsed = HashSet::new();
        assert_eq!(names(&a.outline, &visible_rows(&a.outline, &keys, &collapsed, "")).len(), 7);
        collapsed.insert(keys[1]);
        assert_eq!(
            names(&a.outline, &visible_rows(&a.outline, &keys, &collapsed, "")),
            ["library", "book b1", "book b2", "title", "shelf"],
            "the first book's title and year are gone, the second book's title stays"
        );
        // An item with nothing under it hides nothing, whatever the set says.
        collapsed.insert(keys[2]);
        assert_eq!(visible_rows(&a.outline, &keys, &collapsed, "").len(), 5);
        collapsed.insert(keys[0]);
        assert_eq!(names(&a.outline, &visible_rows(&a.outline, &keys, &collapsed, "")), ["library"]);
    }

    #[test]
    fn the_filter_shows_matches_with_the_items_around_them_whatever_is_collapsed() {
        let a = analyze(LIBRARY, 1);
        let keys = keys(&a.outline);
        let collapsed: HashSet<u64> = keys.iter().copied().collect();
        assert_eq!(
            names(&a.outline, &visible_rows(&a.outline, &keys, &collapsed, "title")),
            ["(library)", "(book b1)", "title", "(book b2)", "title"]
        );
        // The hint and the preview are searched too.
        assert_eq!(names(&a.outline, &visible_rows(&a.outline, &keys, &collapsed, "b2")), ["(library)", "book b2"]);
        assert_eq!(names(&a.outline, &visible_rows(&a.outline, &keys, &collapsed, "2019")), ["(library)", "(book b1)", "year"]);
        assert!(visible_rows(&a.outline, &keys, &collapsed, "nothing").is_empty());
    }

    #[test]
    fn a_small_document_opens_fully_and_a_large_one_to_depth_two() {
        let a = analyze(LIBRARY, 1);
        assert!(default_collapsed(&a.outline, &keys(&a.outline)).is_empty());
        let books: String = (0..200).map(|i| format!("<book id=\"b{i}\"><title>T</title><year>1</year></book>")).collect();
        let big = format!("<lib><shelf>{books}</shelf></lib>");
        let a = analyze(&big, 1);
        assert!(a.outline.len() > OPEN_FULLY);
        let keys = keys(&a.outline);
        let collapsed = default_collapsed(&a.outline, &keys);
        assert_eq!(collapsed.len(), 200, "each book (depth 2) is collapsed");
        let rows = visible_rows(&a.outline, &keys, &collapsed, "");
        assert_eq!(rows.len(), 202);
        assert!(rows.iter().all(|r| a.outline[r.index].depth <= OPEN_DEPTH));
    }

    #[test]
    fn paths_survive_typing_elsewhere_and_tell_brothers_apart() {
        let a = analyze(LIBRARY, 1);
        let b = analyze(&LIBRARY.replace("<library>", "<library>\n  <!-- a comment that moves every offset -->"), 2);
        assert_eq!(keys(&a.outline), keys(&b.outline));
        let k = keys(&a.outline);
        assert_ne!(k[1], k[4], "two books");
        assert_ne!(k[2], k[5], "the titles of two books");
    }

    #[test]
    fn a_problem_marks_its_own_row_or_the_collapsed_row_that_hides_it() {
        let a = analyze(LIBRARY, 1);
        let keys = keys(&a.outline);
        let year = LIBRARY.find("2019").expect("year");
        let shelf = LIBRARY.find("<shelf").expect("shelf");
        let library = LIBRARY.find("<library").expect("library");
        let problems = [(library, Marker::Warning), (year, Marker::Warning), (year + 1, Marker::Error), (shelf, Marker::Warning)];
        let open = visible_rows(&a.outline, &keys, &HashSet::new(), "");
        let m = markers(&a.outline, &open, &problems, false);
        let at = |rows: &[Row], m: &[Option<Marker>], name: &str| {
            rows.iter().zip(m).find(|(r, _)| row_label(&a.outline[r.index]) == name).and_then(|(_, m)| *m)
        };
        assert_eq!(at(&open, &m, "year"), Some(Marker::Error), "the worse of the two");
        assert_eq!(at(&open, &m, "book b1"), None, "an open item does not repeat it");
        assert_eq!(at(&open, &m, "shelf"), Some(Marker::Warning));
        assert_eq!(at(&open, &m, "library"), Some(Marker::Warning), "its own problem only");
        let collapsed: HashSet<u64> = [keys[1]].into_iter().collect();
        let rows = visible_rows(&a.outline, &keys, &collapsed, "");
        let m = markers(&a.outline, &rows, &problems, false);
        assert_eq!(at(&rows, &m, "book b1"), Some(Marker::Error), "collapsed: it carries what it hides");
        assert_eq!(at(&rows, &m, "book b2"), None);
        // Collapsing the root rolls everything up into it.
        let collapsed: HashSet<u64> = [keys[0]].into_iter().collect();
        let rows = visible_rows(&a.outline, &keys, &collapsed, "");
        assert_eq!(markers(&a.outline, &rows, &problems, false), [Some(Marker::Error)]);
        // A problem outside every element marks nothing.
        assert_eq!(markers(&a.outline, &open, &[(LIBRARY.len() + 5, Marker::Error)], false).iter().flatten().count(), 0);
    }

    #[test]
    fn the_caret_opens_the_items_around_it() {
        let a = analyze(LIBRARY, 1);
        let keys = keys(&a.outline);
        let mut collapsed: HashSet<u64> = keys.iter().copied().collect();
        let year = a.outline.iter().position(|i| i.name == "year").expect("year");
        assert!(reveal(&a.outline, &keys, &mut collapsed, year));
        assert!(!collapsed.contains(&keys[0]) && !collapsed.contains(&keys[1]), "library and the first book open");
        assert!(collapsed.contains(&keys[4]) && collapsed.contains(&keys[year]), "the second book and the year itself stay as they were");
        let rows = visible_rows(&a.outline, &keys, &collapsed, "");
        assert!(rows.iter().any(|r| r.index == year));
        assert!(!reveal(&a.outline, &keys, &mut collapsed, year), "nothing more to open");
        assert!(!reveal(&a.outline, &keys, &mut collapsed, 999), "no such item");
    }

    #[test]
    fn a_subtree_is_the_item_and_everything_under_it() {
        let a = analyze(LIBRARY, 1);
        assert_eq!(subtree(&a.outline, 1), 1..4);
        assert_eq!(subtree(&a.outline, 0), 0..7);
        assert_eq!(subtree(&a.outline, 6), 6..7);
        assert_eq!(subtree(&a.outline, 99), 99..99);
    }
}
