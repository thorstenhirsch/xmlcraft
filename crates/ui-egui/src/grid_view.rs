//! The Grid view of a document: its rows (name, type, value) as a tree table in place of the text, for XML, JSON, JSON Lines, YAML and
//! BSON. A value or a name is changed in place; the change is a minimal edit of the text (`xmlcraft_grid`), one undoable step, so the
//! text stays the model.
//!
//! A node whose children repeat (XML elements of one name, an array of objects or of values) can be shown as a table (`grid_table.rs`):
//! a toggle in its row switches between the table under it and its rows, and the focus button (or a double click on its name) opens the
//! table as the whole view, with a breadcrumb back. A document that is itself a list of items (JSON Lines, an array) opens as its table.
//!
//! Keys: Up/Down choose a row, Right opens it and Left closes it, Enter edits the value, F2 (Rename) the name, Delete removes the row. In
//! a table the arrows move the selection, Enter or F2 edit the cell, Delete clears the cells, Cmd/Ctrl+C and V copy and paste
//! tab-separated text. The document's text is never rewritten as a whole.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use xmlcraft_engine::grid::read_grid;
use xmlcraft_grid::{CellKind, Grid, Row, Table};

use crate::XmlCraftApp;
use crate::grid_table::{self, CellEdit, Naming, Pos, TableDo, TableKey, TableView};
use crate::i18n::{fmt, tl};
use crate::theme::{self, Tokens};

/// The height of a row.
const ROW: f32 = 24.0;
/// The width of one level of indentation.
const INDENT: f32 = 16.0;
/// A table opens as a table (not as rows) when it has this many rows or more.
const TABLE_AT: usize = 3;

/// Which cell is being edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cell {
    Name,
    Value,
}

/// The grid of one document: what is open, what is chosen, what is being typed, and its tables.
#[derive(Default)]
pub(crate) struct GridState {
    pub(crate) expanded: HashSet<Vec<usize>>,
    pub(crate) selected: Option<Vec<usize>>,
    editing: Option<(Vec<usize>, Cell, String)>,
    /// The grid read from the text at this version of it (read again when the text changes), and that text.
    cache: Option<(u64, Result<Grid, String>)>,
    text: Arc<str>,
    /// The editing field has the keyboard at the next frame.
    focus_edit: bool,
    /// Whether a node is shown as a table (true) or as rows; nodes not in it follow the row count. Kept while the app runs.
    modes: HashMap<Vec<usize>, bool>,
    /// The tables drilled into, outermost first; the last is the whole view.
    focus: Vec<TableKey>,
    /// What each table keeps (sort, filter, selection).
    tables: HashMap<TableKey, TableView>,
    /// The tables of this version of the text (none: the node holds no table).
    built: HashMap<TableKey, Option<Arc<Table>>>,
    /// The table in the tree that has the keyboard (its selection).
    active: Option<TableKey>,
    /// The document's own table was considered (opened when it is one).
    started: bool,
}

impl GridState {
    /// The table at `key` (built once for this version of the text), with the columns added in the view.
    fn table(&mut self, key: &TableKey) -> Option<Arc<Table>> {
        if let Some(t) = self.built.get(key) {
            return t.clone();
        }
        let Some((_, Ok(grid))) = &self.cache else { return None };
        let built = grid.table_named(&self.text, &key.0, key.1.as_deref()).map(|t| {
            let view = self.tables.entry(key.clone()).or_default();
            Arc::new(grid_table::with_extra(grid, &self.text, t, view))
        });
        self.built.insert(key.clone(), built.clone());
        built
    }

    /// Whether the node at `path` is shown as a table.
    fn shows_table(&mut self, path: &[usize]) -> bool {
        match self.modes.get(path) {
            Some(mode) => *mode && self.table(&(path.to_vec(), None)).is_some(),
            None => self.table(&(path.to_vec(), None)).is_some_and(|t| t.rows.len() >= TABLE_AT),
        }
    }
}

/// What the user did in a frame.
enum Do {
    Select(Vec<usize>),
    Toggle(Vec<usize>),
    Begin(Vec<usize>, Cell),
    Commit,
    Cancel,
    Remove(Vec<usize>),
    /// Show the node as a table (true) or as rows.
    Mode(Vec<usize>, bool),
    /// Open a table as the whole view.
    Focus(TableKey),
    /// Back to the `n` outermost tables drilled into (0: the tree).
    Unfocus(usize),
    Table(TableKey, TableDo),
}

/// What the tree shows: a row, or the table of the row before it (and its height).
enum Entry {
    Row(Row),
    Table(TableKey, f32),
}

impl XmlCraftApp {
    /// Show the grid of the document being shown in place of its text.
    pub(crate) fn show_grid(&mut self, ui: &mut egui::Ui, index: usize) {
        let Some(doc) = self.session.document(index) else { return };
        let (id, version, language) = (doc.id(), doc.editor().version(), doc.language());
        // (The grid is read again when the text changed since it was.)
        let text_needed = self.views.get(&id).and_then(|v| v.grid.as_ref()).is_none_or(|g| g.cache.as_ref().is_none_or(|(v, _)| *v != version));
        let text = if text_needed { Some(doc.editor().text()) } else { None };
        let Some(state) = self.views.get_mut(&id).and_then(|v| v.grid.as_mut()) else { return };
        if let Some(text) = text {
            let grid = read_grid(&text, language).map_err(|e| e.to_string());
            if let Ok(g) = &grid {
                // What was open and chosen stays where the rows are still there.
                state.expanded.retain(|p| g.row(p).is_some());
                if state.selected.as_ref().is_some_and(|p| g.row(p).is_none()) {
                    state.selected = None;
                }
                state.focus.retain(|k| k.0.is_empty() || g.row(&k.0).is_some());
            }
            state.cache = Some((version, grid));
            state.text = Arc::from(text);
            state.built.clear();
        }
        let t = Tokens::get(ui.ctx());
        let grid = match state.cache.as_ref().map(|(_, g)| g) {
            Some(Ok(g)) => g.clone(),
            Some(Err(why)) => {
                let why = why.clone();
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new(tl!("The grid needs a document that can be read")).font(theme::semibold(15.0)).color(t.text));
                    ui.label(egui::RichText::new(why).color(t.text_muted));
                    ui.label(egui::RichText::new(tl!("Fix the problem in the text view, then switch back to the grid.")).color(t.text_muted));
                });
                return;
            }
            None => return,
        };
        // A document that is itself a list of items opens as its table.
        if !state.started {
            state.started = true;
            let root: TableKey = (Vec::new(), None);
            if state.table(&root).is_some_and(|t| t.rows.len() >= TABLE_AT) {
                state.focus.push(root);
            }
        }
        let action = if state.focus.is_empty() { tree(ui, &t, &grid, state) } else { focused(ui, &t, &grid, state) };
        let Some(action) = action else { return };
        self.apply_grid_action(ui.ctx(), id, &grid, action);
    }

    fn apply_grid_action(&mut self, ctx: &egui::Context, id: u64, grid: &Grid, action: Do) {
        let Some(state) = self.views.get_mut(&id).and_then(|v| v.grid.as_mut()) else { return };
        match action {
            Do::Select(path) => {
                state.selected = Some(path);
                state.active = None;
            }
            Do::Toggle(path) => {
                if !state.expanded.remove(&path) {
                    state.expanded.insert(path.clone());
                }
                state.selected = Some(path);
                state.active = None;
            }
            Do::Begin(path, cell) => {
                let Some(row) = grid.row(&path) else { return };
                let initial = match cell {
                    Cell::Name => row.name.clone(),
                    Cell::Value => row.value.clone(),
                };
                if cell == Cell::Value && !row.editable {
                    self.status = tl!("This cell cannot be edited: it holds other rows.").to_owned();
                    return;
                }
                if cell == Cell::Name && (row.name.is_empty() || row.name.starts_with(['[', '#', '?'])) {
                    self.status = tl!("This row has no name to change.").to_owned();
                    return;
                }
                state.selected = Some(path.clone());
                state.editing = Some((path, cell, initial));
                state.focus_edit = true;
            }
            Do::Cancel => state.editing = None,
            Do::Commit => {
                let Some((path, cell, typed)) = state.editing.take() else { return };
                self.grid_change(id, grid, &path, cell, &typed);
            }
            Do::Remove(path) => self.grid_remove(id, grid, &path),
            Do::Mode(path, table) => {
                if table {
                    state.expanded.insert(path.clone());
                }
                state.modes.insert(path, table);
            }
            Do::Focus(key) => {
                state.focus.push(key);
                state.editing = None;
            }
            Do::Unfocus(n) => {
                // Back in the tree: the node of the table left is open and chosen there.
                if n == 0
                    && let Some(key) = state.focus.first().cloned()
                    && !key.0.is_empty()
                {
                    open_to(state, &key.0);
                    state.modes.insert(key.0.clone(), false);
                    state.selected = Some(key.0);
                }
                state.focus.truncate(n);
            }
            Do::Table(key, TableDo::Drill(path, name)) => {
                let target: TableKey = (path.clone(), name);
                if grid.table_named(&state.text, &target.0, target.1.as_deref()).is_some() {
                    if state.focus.is_empty() {
                        state.focus.push(key);
                    }
                    state.focus.push(target);
                } else {
                    // Not a table: its rows, open, in the tree.
                    open_to(state, &path);
                    state.modes.insert(path.clone(), false);
                    state.selected = Some(path);
                    state.focus.clear();
                    state.active = None;
                }
            }
            Do::Table(key, what) => self.table_action(ctx, id, grid, key, what),
        }
    }

    /// Do what was done to a table: a change of the view, or edits of the text as one undoable step.
    fn table_action(&mut self, ctx: &egui::Context, id: u64, grid: &Grid, key: TableKey, what: TableDo) {
        let Some(state) = self.views.get_mut(&id).and_then(|v| v.grid.as_mut()) else { return };
        let Some(table) = state.table(&key) else { return };
        let text = state.text.clone();
        state.active = (state.focus.last() != Some(&key)).then(|| key.clone());
        let mut rebuild = false;
        let view = state.tables.entry(key.clone()).or_default();
        let order = view.order(&table);
        let last_col = table.columns.len().saturating_sub(1);
        let step = |head: Pos, dr: isize, dc: isize| Pos {
            row: head.row.saturating_add_signed(dr).min(order.len().saturating_sub(1)),
            col: head.col.saturating_add_signed(dc).min(last_col),
        };
        let mut status: Option<String> = None;
        let edits = match what {
            TableDo::Select(pos, extend) => {
                if !extend || view.anchor.is_none() {
                    view.anchor = Some(pos);
                }
                view.head = Some(pos);
                None
            }
            TableDo::SelectRow(row, extend) => {
                let start = if extend { view.anchor.map_or(row, |a| a.row) } else { row };
                view.anchor = Some(Pos { row: start, col: 0 });
                view.head = Some(Pos { row, col: last_col });
                None
            }
            TableDo::Move(dr, dc, extend) => {
                let next = step(view.head.unwrap_or(Pos { row: 0, col: 0 }), dr, dc);
                if !extend {
                    view.anchor = Some(next);
                }
                view.head = Some(next);
                view.reveal_selection();
                None
            }
            TableDo::Sort(c) => {
                let label = table.columns.get(c).map(|c| c.label()).unwrap_or_default();
                view.sort = match &view.sort {
                    Some((l, true)) if *l == label => Some((label, false)),
                    Some((l, false)) if *l == label => None,
                    _ => Some((label, true)),
                };
                view.anchor = None;
                view.head = None;
                None
            }
            TableDo::Begin(pos, initial) => {
                let Some(cell) = order.get(pos.row).and_then(|&r| table.rows.get(r)).and_then(|r| r.cells.get(pos.col)) else { return };
                if cell.kind == CellKind::Nested {
                    status = Some(tl!("This cell holds structure: click it to open it.").to_owned());
                } else if let Some(&row) = order.get(pos.row) {
                    view.anchor = Some(pos);
                    view.head = Some(pos);
                    view.editing = Some(CellEdit { row, col: pos.col, text: initial.unwrap_or_else(|| cell.text.clone()), focus: true });
                }
                None
            }
            TableDo::Cancel => {
                view.editing = None;
                None
            }
            TableDo::Commit(next) => {
                let Some(edit) = view.editing.take() else { return };
                let result = grid.set_cell(&text, &table, edit.row, edit.col, &edit.text);
                if result.is_err() {
                    // The field stays open with what was typed, so it can be put right.
                    view.editing = Some(CellEdit { focus: true, ..edit });
                } else if let (Some((dr, dc)), Some(head)) = (next, view.head) {
                    let to = step(head, dr, dc);
                    view.anchor = Some(to);
                    view.head = Some(to);
                }
                Some((result, tl!("Not changed: {why}")))
            }
            TableDo::Clear => {
                let Some((r0, r1, c0, c1)) = view.rect() else { return };
                let cells: Vec<(usize, usize)> =
                    (r0..=r1).filter_map(|p| order.get(p).copied()).flat_map(|r| (c0..=c1).map(move |c| (r, c))).collect();
                Some((grid.clear_cells(&text, &table, &cells), tl!("Not cleared: {why}")))
            }
            // (Handled with the view of the whole grid.)
            TableDo::Drill(..) => None,
            TableDo::Copy => {
                let Some((r0, r1, c0, c1)) = view.rect() else { return };
                let rows: Vec<usize> = (r0..=r1).filter_map(|p| order.get(p).copied()).collect();
                ctx.copy_text(table.to_tsv(&rows, &(c0..=c1).collect::<Vec<_>>()));
                None
            }
            TableDo::Paste(tsv) => {
                let Some((r0, _, c0, _)) = view.rect() else { return };
                let in_order = order.len() == table.rows.len() && order.iter().enumerate().all(|(i, r)| i == *r);
                if !in_order && tsv.trim_end_matches(['\n', '\r']).contains(['\n', '\r']) {
                    status = Some(tl!("Rows are pasted in the order of the document: clear the sort and the filter first.").to_owned());
                    None
                } else {
                    let row = order.get(r0).copied().unwrap_or(table.rows.len());
                    Some((grid.paste_tsv(&text, &table, row, c0, &tsv).map(|p| p.edits), tl!("Not pasted: {why}")))
                }
            }
            TableDo::InsertRow(after) => Some((grid.insert_row(&text, &table, after), tl!("No row added: {why}"))),
            TableDo::Duplicate(row) => Some((grid.duplicate_row(&text, &table, row), tl!("Not duplicated: {why}"))),
            TableDo::DeleteRows(rows) => {
                view.anchor = None;
                view.head = None;
                Some((grid.delete_rows(&text, &table, &rows), tl!("Not deleted: {why}")))
            }
            TableDo::MoveRow(row, before) => Some((grid.move_row(&text, &table, row, before), tl!("Not moved: {why}"))),
            TableDo::ApplyOrder => {
                if view.filter.trim().is_empty() {
                    let result = grid.apply_order(&text, &table, &order);
                    if result.is_ok() {
                        view.sort = None;
                        view.anchor = None;
                        view.head = None;
                    }
                    Some((result, tl!("Not put in order: {why}")))
                } else {
                    status = Some(tl!("Clear the filter first: the order names every row.").to_owned());
                    None
                }
            }
            TableDo::StartNaming(naming) => {
                view.naming = Some(naming);
                None
            }
            TableDo::CancelNaming => {
                view.naming = None;
                None
            }
            TableDo::FinishNaming => match view.naming.take() {
                Some(Naming::Rename(c, name)) => Some((grid.rename_column(&text, &table, c, name.trim()), tl!("Not renamed: {why}"))),
                Some(Naming::Insert(after, name)) => {
                    let name = name.trim().to_owned();
                    match grid.insert_column(&text, &table, &name, after) {
                        Ok(_) => {
                            view.extra.push((name, after.and_then(|a| table.columns.get(a)).map(|c| c.label())));
                            rebuild = true;
                        }
                        Err(why) => status = Some(fmt(tl!("No column added: {why}"), &[("why", &why.to_string())])),
                    }
                    None
                }
                None => None,
            },
            TableDo::DeleteColumn(c) => {
                let label = table.columns.get(c).map(|c| c.label());
                let before = view.extra.len();
                view.extra.retain(|(l, _)| Some(l) != label.as_ref());
                rebuild = view.extra.len() != before;
                Some((grid.delete_column(&text, &table, c), tl!("Not deleted: {why}")))
            }
        };
        if rebuild {
            state.built.remove(&key);
        }
        if let Some(message) = status {
            self.status = message;
        }
        if let Some((result, failed)) = edits {
            self.apply_table_edits(id, result, failed);
        }
    }

    /// Change a cell: the edits of the grid applied to the text as one undoable step.
    fn grid_change(&mut self, id: u64, grid: &Grid, path: &[usize], cell: Cell, typed: &str) {
        let Some(index) = self.session.position_of(id) else { return };
        let Some(doc) = self.session.document_mut(index) else { return };
        let text = doc.editor().text();
        let result = match cell {
            Cell::Value => grid.set_value(&text, path, typed),
            Cell::Name => grid.rename(&text, path, typed),
        };
        match result {
            Ok(edits) => {
                doc.editor_mut().break_undo_group();
                doc.editor_mut().apply_edits(&edits);
                doc.editor_mut().break_undo_group();
                self.status.clear();
            }
            Err(why) => {
                self.status = fmt(tl!("Not changed: {why}"), &[("why", &why.to_string())]);
                // The field stays open with what was typed, so it can be put right.
                if let Some(state) = self.views.get_mut(&id).and_then(|v| v.grid.as_mut()) {
                    state.editing = Some((path.to_vec(), cell, typed.to_owned()));
                    state.focus_edit = true;
                }
            }
        }
    }

    fn grid_remove(&mut self, id: u64, grid: &Grid, path: &[usize]) {
        let Some(index) = self.session.position_of(id) else { return };
        let Some(doc) = self.session.document_mut(index) else { return };
        let text = doc.editor().text();
        match grid.remove(&text, path) {
            Ok(edits) => {
                doc.editor_mut().break_undo_group();
                doc.editor_mut().apply_edits(&edits);
                doc.editor_mut().break_undo_group();
                if let Some(state) = self.views.get_mut(&id).and_then(|v| v.grid.as_mut()) {
                    state.selected = None;
                }
                self.status.clear();
            }
            Err(why) => self.status = fmt(tl!("Not removed: {why}"), &[("why", &why.to_string())]),
        }
    }

    /// Switch the document being shown between its text and its grid (View ▸ Grid).
    pub(crate) fn toggle_grid(&mut self) {
        let Some(index) = self.session.active() else { return };
        let Some(doc) = self.session.document(index) else { return };
        let (id, kind) = (doc.id(), self.applied.unwrap_or(theme::ThemeKind::Light));
        let view = self.views.entry(id).or_insert_with(|| crate::documents::DocView::new(kind));
        view.grid = if view.grid.is_some() { None } else { Some(GridState::default()) };
        // The editor has the keyboard again when the text comes back.
        self.focus_editor = view.grid.is_none();
    }

    /// Start editing the name (F2, Rename) or the value of the chosen row of the grid being shown, or the chosen cell of a table. False
    /// when no grid is shown.
    pub(crate) fn grid_begin_edit(&mut self, cell: Cell) -> bool {
        let Some(doc) = self.session.active_document() else { return false };
        let id = doc.id();
        let Some(state) = self.views.get(&id).and_then(|v| v.grid.as_ref()) else { return false };
        let Some(Ok(grid)) = state.cache.as_ref().map(|(_, g)| g.clone()) else { return true };
        // (Copying needs no context here: beginning an edit copies nothing.)
        let ctx = egui::Context::default();
        // A table with a chosen cell: that cell.
        if let Some(key) = state.focus.last().or(state.active.as_ref()).cloned()
            && let Some(head) = state.tables.get(&key).and_then(|v| v.head)
        {
            self.apply_grid_action(&ctx, id, &grid, Do::Table(key, TableDo::Begin(head, None)));
            return true;
        }
        let Some(path) = state.selected.clone() else {
            self.status = tl!("Choose a row first.").to_owned();
            return true;
        };
        self.apply_grid_action(&ctx, id, &grid, Do::Begin(path, cell));
        true
    }

    /// Whether the document being shown is shown as a grid.
    pub(crate) fn grid_shown(&self) -> bool {
        self.session.active_document().is_some_and(|d| self.views.get(&d.id()).is_some_and(|v| v.grid.is_some()))
    }
}

/// Open the rows down to `path` in the tree.
fn open_to(state: &mut GridState, path: &[usize]) {
    for depth in 1..=path.len() {
        state.expanded.insert(path.get(..depth).unwrap_or_default().to_vec());
    }
}

/// Whether a text field (a cell, a filter, a dialog) has the keyboard.
fn typing(ui: &egui::Ui) -> bool {
    ui.ctx().memory(|m| m.focused().is_some())
}

/// The tree of rows, with tables under the nodes shown as tables.
fn tree(ui: &mut egui::Ui, t: &Tokens, grid: &Grid, state: &mut GridState) -> Option<Do> {
    // The rows of a node shown as a table are not rows of the tree.
    let mut open = state.expanded.clone();
    let mut as_table = HashSet::new();
    for path in state.expanded.clone() {
        if state.shows_table(&path) {
            open.remove(&path);
            as_table.insert(path);
        }
    }
    let rows = grid.rows(&open);
    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        let key: TableKey = (row.path.clone(), None);
        let table = if as_table.contains(&row.path) { state.table(&key) } else { None };
        entries.push(Entry::Row(row));
        if let Some(table) = table {
            let view = state.tables.entry(key.clone()).or_default();
            entries.push(Entry::Table(key, grid_table::inline_height(&table, view)));
        }
    }
    let mut action: Option<Do> = None;
    // Keys: only when no cell is being edited (and no text field, a dialog's included, has the keyboard).
    if state.editing.is_none() && !typing(ui) {
        let table_keys = state.active.clone().and_then(|k| state.tables.get(&k).and_then(|v| grid_table::keys(ui, v)).map(|d| Do::Table(k, d)));
        action = table_keys.or_else(|| {
            let rows: Vec<&Row> = entries.iter().filter_map(|e| if let Entry::Row(r) = e { Some(r) } else { None }).collect();
            keys(ui, &rows, state)
        });
    }
    // The document's own table, when it is one.
    let root: TableKey = (Vec::new(), None);
    if let Some(table) = state.table(&root) {
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(fmt(tl!("The document is a list of {n} items."), &[("n", &table.rows.len().to_string())])).color(t.text_muted),
            );
            if ui.button(tl!("Show as a table")).clicked() {
                action = Some(Do::Focus(root.clone()));
            }
        });
    }
    header(ui, t);
    let heights: Vec<f32> = entries.iter().map(|e| if let Entry::Table(_, h) = e { *h } else { ROW }).collect();
    let total: f32 = heights.iter().sum();
    egui::ScrollArea::vertical().id_salt("grid-tree").auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
        let (whole, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), total), egui::Sense::hover());
        let mut y = whole.top();
        for (entry, h) in entries.iter().zip(&heights) {
            let rect = egui::Rect::from_min_size(egui::pos2(whole.left(), y), egui::vec2(whole.width(), *h));
            y += h;
            if rect.bottom() < whole.top() + viewport.top() || rect.top() > whole.top() + viewport.bottom() {
                continue;
            }
            let done = ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| match entry {
                Entry::Row(row) => {
                    let tabling = (row.child_count > 0 && state.table(&(row.path.clone(), None)).is_some()).then(|| as_table.contains(&row.path));
                    let is_selected = state.active.is_none() && state.selected.as_deref() == Some(row.path.as_slice());
                    let expanded = state.expanded.contains(&row.path);
                    let edit = state.editing.as_mut().filter(|(p, _, _)| *p == row.path);
                    draw_row(ui, t, row, is_selected, expanded, edit, &mut state.focus_edit, tabling)
                }
                Entry::Table(key, h) => {
                    let table = state.table(key)?;
                    let view = state.tables.entry(key.clone()).or_default();
                    let mut inner = ui.max_rect();
                    inner.min.x += (key.0.len() as f32) * INDENT + 12.0;
                    inner.max.x -= 8.0;
                    ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
                        let bar = grid_table::bar(ui, t, &table, view);
                        let body = grid_table::draw(ui, t, key, &table, view, h - grid_table::BAR_H - 6.0);
                        body.or(bar).map(|d| Do::Table(key.clone(), d))
                    })
                    .inner
                }
            });
            if done.inner.is_some() {
                action = done.inner;
            }
        }
    });
    action
}

/// A table as the whole view: the breadcrumb back, the bar and the table.
fn focused(ui: &mut egui::Ui, t: &Tokens, grid: &Grid, state: &mut GridState) -> Option<Do> {
    let key = state.focus.last()?.clone();
    let Some(table) = state.table(&key) else {
        // The node holds no table any more (its rows were deleted): back to where it was.
        state.focus.pop();
        return None;
    };
    let mut action: Option<Do> = None;
    // The breadcrumb: the tree, then every table drilled into (`library › book`).
    let crumbs: Vec<String> = state
        .focus
        .iter()
        .enumerate()
        .map(|(i, k)| {
            // A table drilled into from another is named by the steps below that one (`book › author`).
            let below = i.checked_sub(1).and_then(|p| state.focus.get(p)).map_or(0, |p| p.0.len().min(k.0.len()));
            let mut names: Vec<String> = grid.names_along(&k.0).into_iter().skip(below).collect();
            if names.is_empty() && i == 0 {
                names.push(tl!("document").to_owned());
            }
            if let Some(item) = state.built.get(k).cloned().flatten().and_then(|t| t.item_name.clone()).or_else(|| k.1.clone()) {
                names.push(item);
            }
            names.join(" › ")
        })
        .collect();
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        if ui.link(tl!("Tree")).clicked() {
            action = Some(Do::Unfocus(0));
        }
        for (i, label) in crumbs.iter().enumerate() {
            ui.label(egui::RichText::new("›").color(t.text_faint));
            if i + 1 == crumbs.len() {
                ui.label(egui::RichText::new(label).font(theme::semibold(13.0)).color(t.text));
            } else if ui.link(label).clicked() {
                action = Some(Do::Unfocus(i + 1));
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(8.0);
            if ui.button(tl!("Show as rows")).clicked() {
                action = Some(Do::Unfocus(0));
            }
        });
    });
    let depth = state.focus.len();
    let view = state.tables.entry(key.clone()).or_default();
    // The selection stays among the rows shown.
    let shown = view.order(&table).len();
    if view.head.is_some_and(|h| h.row >= shown || h.col >= table.columns.len()) {
        view.anchor = None;
        view.head = None;
    }
    if action.is_none() && view.editing.is_none() && view.naming.is_none() && !typing(ui) {
        action = grid_table::keys(ui, view).map(|d| Do::Table(key.clone(), d));
        if action.is_none() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            action = Some(Do::Unfocus(depth.saturating_sub(1)));
        }
    }
    let bar = grid_table::bar(ui, t, &table, view);
    let height = ui.available_height() - 4.0;
    let body = grid_table::draw(ui, t, &key, &table, view, height);
    action.or(body.or(bar).map(|d| Do::Table(key, d)))
}

fn header(ui: &mut egui::Ui, t: &Tokens) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 0.0, t.chrome);
    painter.hline(rect.x_range(), rect.bottom(), egui::Stroke::new(1.0, t.divider));
    let columns = columns(rect);
    for (label, x) in [(tl!("Name"), columns.0), (tl!("Type"), columns.1), (tl!("Value"), columns.2)] {
        painter.text(egui::pos2(x, rect.center().y), egui::Align2::LEFT_CENTER, label, theme::semibold(12.0), t.text_muted);
    }
}

/// Where the Name, Type and Value columns start.
fn columns(rect: egui::Rect) -> (f32, f32, f32) {
    let width = rect.width();
    (rect.left() + 12.0, rect.left() + width * 0.38, rect.left() + width * 0.52)
}

fn keys(ui: &egui::Ui, rows: &[&Row], state: &GridState) -> Option<Do> {
    let selected = state.selected.as_ref()?;
    let at = rows.iter().position(|r| &r.path == selected)?;
    let row = rows.get(at)?;
    let pressed = |key| ui.input(|i| i.key_pressed(key));
    if pressed(egui::Key::ArrowDown) {
        return rows.get(at + 1).map(|r| Do::Select(r.path.clone()));
    }
    if pressed(egui::Key::ArrowUp) {
        return at.checked_sub(1).and_then(|i| rows.get(i)).map(|r| Do::Select(r.path.clone()));
    }
    if pressed(egui::Key::ArrowRight) && row.child_count > 0 && !state.expanded.contains(&row.path) {
        return Some(Do::Toggle(row.path.clone()));
    }
    if pressed(egui::Key::ArrowLeft) && state.expanded.contains(&row.path) {
        return Some(Do::Toggle(row.path.clone()));
    }
    if pressed(egui::Key::Enter) {
        return Some(Do::Begin(row.path.clone(), Cell::Value));
    }
    if pressed(egui::Key::Delete) || pressed(egui::Key::Backspace) {
        return Some(Do::Remove(row.path.clone()));
    }
    None
}

/// A small icon button painted in `rect` with a name for assistive technology.
fn icon_button(ui: &egui::Ui, t: &Tokens, rect: egui::Rect, id: egui::Id, icon: &str, tint: egui::Color32, label: String) -> bool {
    let response = ui.interact(rect, id, egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label.clone()));
    if response.hovered() {
        ui.painter().rect_filled(rect, 4.0, t.hover);
    }
    crate::icons::paint(ui, egui::Rect::from_center_size(rect.center(), egui::vec2(14.0, 14.0)), icon, 14.0, tint);
    response.clicked()
}

#[allow(clippy::too_many_arguments)]
fn draw_row(
    ui: &mut egui::Ui,
    t: &Tokens,
    row: &Row,
    selected: bool,
    expanded: bool,
    editing: Option<&mut (Vec<usize>, Cell, String)>,
    focus_edit: &mut bool,
    // Some when the row's children are a table: whether it is shown as one.
    table: Option<bool>,
) -> Option<Do> {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), egui::Sense::click());
    let label = if row.value.is_empty() {
        format!("{} ({})", row.name, row.kind.label())
    } else {
        format!("{}: {} ({})", row.name, row.value, row.kind.label())
    };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label.clone()));
    if selected {
        ui.painter().rect_filled(rect, 0.0, t.accent_soft);
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 0.0, t.hover);
    }
    let (name_x, type_x, value_x) = columns(rect);
    let indent = row.depth as f32 * INDENT;
    let centre = rect.center().y;
    // The mark that opens and closes a row.
    let mut action = None;
    if row.child_count > 0 {
        let mark = if expanded { "chevron-down" } else { "chevron-right" };
        let mark_rect = egui::Rect::from_min_size(egui::pos2(name_x + indent - 2.0, rect.top()), egui::vec2(16.0, ROW));
        crate::icons::paint(ui, egui::Rect::from_center_size(mark_rect.center(), egui::vec2(12.0, 12.0)), mark, 12.0, t.text_muted);
        if response.clicked() && response.interact_pointer_pos().is_some_and(|p| mark_rect.expand(2.0).contains(p)) {
            action = Some(Do::Toggle(row.path.clone()));
        }
    }
    let text_x = name_x + indent + 16.0;
    let clip = ui.painter().with_clip_rect(rect);
    clip.text(egui::pos2(text_x, centre), egui::Align2::LEFT_CENTER, &row.name, theme::semibold(13.0), t.text);
    clip.text(egui::pos2(type_x, centre), egui::Align2::LEFT_CENTER, row.kind.label(), theme::regular(12.0), t.text_faint);
    let value_rect = egui::Rect::from_min_max(egui::pos2(value_x, rect.top() + 2.0), egui::pos2(rect.right() - 8.0, rect.bottom() - 2.0));
    let name_rect = egui::Rect::from_min_max(egui::pos2(text_x, rect.top() + 2.0), egui::pos2(type_x - 8.0, rect.bottom() - 2.0));
    // The table toggle and the focus button, at the right end of a row whose children are a table.
    if let Some(shown) = table {
        let focus_rect = egui::Rect::from_min_size(egui::pos2(rect.right() - 30.0, rect.top() + 2.0), egui::vec2(24.0, ROW - 4.0));
        let toggle_rect = focus_rect.translate(egui::vec2(-28.0, 0.0));
        let (icon, tint, toggle_label) = if shown {
            ("list-tree", t.accent, fmt(tl!("Show {name} as rows"), &[("name", &row.name)]))
        } else {
            ("layout-grid", t.icon, fmt(tl!("Show {name} as a table"), &[("name", &row.name)]))
        };
        if icon_button(ui, t, toggle_rect, egui::Id::unique("xmlcraft-grid-table-toggle").with(&row.path), icon, tint, toggle_label) {
            action = Some(Do::Mode(row.path.clone(), !shown));
        }
        let focus_label = fmt(tl!("Open {name} as a table"), &[("name", &row.name)]);
        if icon_button(ui, t, focus_rect, egui::Id::unique("xmlcraft-grid-table-focus").with(&row.path), "external-link", t.icon, focus_label) {
            action = Some(Do::Focus((row.path.clone(), None)));
        }
    }
    match editing {
        Some((_, cell, typed)) => {
            let field = if *cell == Cell::Name { name_rect } else { value_rect };
            let edit = egui::TextEdit::singleline(typed).font(theme::mono(13.0)).desired_width(field.width());
            let out = ui.put(field, edit);
            let what = if *cell == Cell::Name { tl!("The name of the row") } else { tl!("The value of the row") };
            crate::widgets::name_for_assistive_technology(ui, &out, what);
            if *focus_edit {
                out.request_focus();
                *focus_edit = false;
            }
            if out.lost_focus() {
                action = Some(if ui.input(|i| i.key_pressed(egui::Key::Escape)) { Do::Cancel } else { Do::Commit });
            }
        }
        None => {
            let shown =
                if row.value.chars().count() > 300 { format!("{}…", row.value.chars().take(300).collect::<String>()) } else { row.value.clone() };
            let colour = if row.editable { t.text } else { t.text_muted };
            clip.text(egui::pos2(value_x, centre), egui::Align2::LEFT_CENTER, shown.replace(['\n', '\r'], " "), theme::mono(13.0), colour);
            if response.double_clicked() {
                let on_value = response.interact_pointer_pos().is_some_and(|p| p.x >= type_x + 40.0);
                action = Some(if !on_value && table.is_some() {
                    // A double click on the name of a table opens it.
                    Do::Focus((row.path.clone(), None))
                } else {
                    Do::Begin(row.path.clone(), if on_value { Cell::Value } else { Cell::Name })
                });
            } else if response.clicked() && action.is_none() {
                action = Some(Do::Select(row.path.clone()));
            }
        }
    }
    response.context_menu(|ui| {
        if ui.button(tl!("Edit value")).clicked() {
            action = Some(Do::Begin(row.path.clone(), Cell::Value));
            ui.close();
        }
        if ui.button(tl!("Rename")).clicked() {
            action = Some(Do::Begin(row.path.clone(), Cell::Name));
            ui.close();
        }
        if ui.button(tl!("Remove")).clicked() {
            action = Some(Do::Remove(row.path.clone()));
            ui.close();
        }
    });
    action
}
