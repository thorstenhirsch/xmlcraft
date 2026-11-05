//! The Grid view of a document: its rows (name, type, value) as a tree table in place of the text, for XML, JSON and YAML. A value or a
//! name is changed in place; the change is a minimal edit of the text (`xmlcraft_grid`), one undoable step, so the text stays the model.
//!
//! Keys: Up/Down choose a row, Right opens it and Left closes it, Enter edits the value, F2 (Rename) the name, Delete removes the row. The
//! document's text is never rewritten as a whole.

use std::collections::HashSet;

use xmlcraft_engine::compare::family_of;
use xmlcraft_grid::{Grid, Row};

use crate::XmlCraftApp;
use crate::i18n::{fmt, tl};
use crate::theme::{self, Tokens};

/// The height of a row.
const ROW: f32 = 24.0;
/// The width of one level of indentation.
const INDENT: f32 = 16.0;

/// Which cell is being edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cell {
    Name,
    Value,
}

/// The grid of one document: what is open, what is chosen, what is being typed.
#[derive(Default)]
pub(crate) struct GridState {
    pub(crate) expanded: HashSet<Vec<usize>>,
    pub(crate) selected: Option<Vec<usize>>,
    editing: Option<(Vec<usize>, Cell, String)>,
    /// The grid read from the text at this version of it (read again when the text changes).
    cache: Option<(u64, Result<Grid, String>)>,
    /// The editing field has the keyboard at the next frame.
    focus_edit: bool,
}

/// What the user did in a frame.
enum Do {
    Select(Vec<usize>),
    Toggle(Vec<usize>),
    Begin(Vec<usize>, Cell),
    Commit,
    Cancel,
    Remove(Vec<usize>),
}

impl XmlCraftApp {
    /// Show the grid of the document being shown in place of its text.
    pub(crate) fn show_grid(&mut self, ui: &mut egui::Ui, index: usize) {
        let Some(doc) = self.session.document(index) else { return };
        let (id, version, family) = (doc.id(), doc.editor().version(), family_of(doc.language()));
        // (The grid is read again when the text changed since it was.)
        let text_needed = self.views.get(&id).and_then(|v| v.grid.as_ref()).is_none_or(|g| g.cache.as_ref().is_none_or(|(v, _)| *v != version));
        let text = if text_needed { Some(doc.editor().text()) } else { None };
        let Some(state) = self.views.get_mut(&id).and_then(|v| v.grid.as_mut()) else { return };
        if let Some(text) = text {
            state.cache = Some((version, Grid::read(&text, family).map_err(|e| e.to_string())));
            if let Some((_, Ok(g))) = &state.cache {
                // What was open and chosen stays where the rows are still there.
                state.expanded.retain(|p| g.row(p).is_some());
                if state.selected.as_ref().is_some_and(|p| g.row(p).is_none()) {
                    state.selected = None;
                }
            }
        }
        let t = Tokens::get(ui.ctx());
        let Some((_, grid)) = state.cache.clone() else { return };
        let grid = match grid {
            Ok(g) => g,
            Err(why) => {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new(tl!("The grid needs a document that can be read")).font(theme::semibold(15.0)).color(t.text));
                    ui.label(egui::RichText::new(why).color(t.text_muted));
                    ui.label(egui::RichText::new(tl!("Fix the problem in the text view, then switch back to the grid.")).color(t.text_muted));
                });
                return;
            }
        };
        let rows = grid.rows(&state.expanded);
        let mut action: Option<Do> = None;
        // Keys: only when no cell is being edited.
        // (And not while a text field has the keyboard, a dialog's included: Backspace there is not for the grid.)
        let editing = state.editing.is_some() || ui.ctx().memory(|m| m.focused().is_some());
        if !editing {
            action = keys(ui, &rows, state);
        }
        header(ui, &t);
        let selected = state.selected.clone();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW, rows.len(), |ui, range| {
            for row in rows.get(range).unwrap_or_default() {
                let is_selected = selected.as_deref() == Some(row.path.as_slice());
                let edit = state.editing.as_mut().filter(|(p, _, _)| *p == row.path);
                let done = draw_row(ui, &t, row, is_selected, state.expanded.contains(&row.path), edit, &mut state.focus_edit);
                if done.is_some() {
                    action = done;
                }
            }
        });
        let Some(action) = action else { return };
        self.apply_grid_action(id, &grid, action);
    }

    fn apply_grid_action(&mut self, id: u64, grid: &Grid, action: Do) {
        let Some(state) = self.views.get_mut(&id).and_then(|v| v.grid.as_mut()) else { return };
        match action {
            Do::Select(path) => state.selected = Some(path),
            Do::Toggle(path) => {
                if !state.expanded.remove(&path) {
                    state.expanded.insert(path.clone());
                }
                state.selected = Some(path);
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

    /// Start editing the name (F2, Rename) or the value of the chosen row of the grid being shown. False when no grid is shown.
    pub(crate) fn grid_begin_edit(&mut self, cell: Cell) -> bool {
        let Some(doc) = self.session.active_document() else { return false };
        let id = doc.id();
        let Some(state) = self.views.get(&id).and_then(|v| v.grid.as_ref()) else { return false };
        let Some(path) = state.selected.clone() else {
            self.status = tl!("Choose a row first.").to_owned();
            return true;
        };
        let Some(Ok(grid)) = state.cache.as_ref().map(|(_, g)| g.clone()) else { return true };
        self.apply_grid_action(id, &grid, Do::Begin(path, cell));
        true
    }

    /// Whether the document being shown is shown as a grid.
    pub(crate) fn grid_shown(&self) -> bool {
        self.session.active_document().is_some_and(|d| self.views.get(&d.id()).is_some_and(|v| v.grid.is_some()))
    }
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

fn keys(ui: &egui::Ui, rows: &[Row], state: &GridState) -> Option<Do> {
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

fn draw_row(
    ui: &mut egui::Ui,
    t: &Tokens,
    row: &Row,
    selected: bool,
    expanded: bool,
    editing: Option<&mut (Vec<usize>, Cell, String)>,
    focus_edit: &mut bool,
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
                action = Some(Do::Begin(row.path.clone(), if on_value { Cell::Value } else { Cell::Name }));
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
