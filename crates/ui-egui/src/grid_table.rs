//! The table display of the Grid view: the children of a node that repeat (`xmlcraft_grid::Table`) as rows and columns, inside the tree
//! (under its node) or as the whole view (drilled into, with a breadcrumb back).
//!
//! Only the cells that can be seen are laid out, in both directions; the row numbers and the header stay where they are while the
//! cells scroll. A cell is edited in place (Enter, F2, a double click, or typing), and every change is the model's minimal edit of the
//! text applied to the editor buffer as one undoable step, so undo and the text view stay in step. Sorting and filtering are views;
//! "Apply order" writes the order into the text.

use xmlcraft_grid::{CellKind, Grid, GridError, Table};
use xmlcraft_text::Edit;

use crate::XmlCraftApp;
use crate::i18n::{fmt, tl};
use crate::theme::{self, Tokens};

/// The height of a row of a table.
pub(crate) const CELL_H: f32 = 22.0;
/// The height of the header row.
const HEAD_H: f32 = 24.0;
/// The width of the row-number column.
const NUMBER_W: f32 = 44.0;
/// The height of the bar above a table (filter, sort note, counts).
pub(crate) const BAR_H: f32 = 30.0;
/// The most rows an inline table shows before it scrolls.
const INLINE_ROWS: usize = 12;

/// Which table: the node and (for XML) the name of the elements that are its rows.
pub(crate) type TableKey = (Vec<usize>, Option<String>);

/// A cell, in the order the table is shown: a position among the rows shown, and a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Pos {
    pub row: usize,
    pub col: usize,
}

/// What is typed into a cell.
#[derive(Clone, Debug)]
pub(crate) struct CellEdit {
    /// The table row (not the position shown) and the column.
    pub row: usize,
    pub col: usize,
    pub text: String,
    pub focus: bool,
}

/// A column being named in the bar above the table.
#[derive(Clone, Debug)]
pub(crate) enum Naming {
    Rename(usize, String),
    Insert(Option<usize>, String),
}

/// What the view keeps for one table: its sort, filter, selection, what is typed, and the columns added that are not in the text yet.
#[derive(Clone, Debug, Default)]
pub(crate) struct TableView {
    /// The column (by label) and whether ascending.
    pub sort: Option<(String, bool)>,
    pub filter: String,
    /// The selection: where it started and where it ends (positions shown).
    pub anchor: Option<Pos>,
    pub head: Option<Pos>,
    pub editing: Option<CellEdit>,
    pub naming: Option<Naming>,
    /// Columns added (label, the label of the column they follow) that no row has yet.
    pub extra: Vec<(String, Option<String>)>,
    /// The row the context menu is about.
    menu_row: Option<usize>,
    /// Bring the selection into view at the next frame.
    reveal: bool,
    /// The rows shown, for the table, filter and sort they were worked out for.
    shown: Option<Shown>,
}

impl TableView {
    /// The rows shown, in the order shown: those the filter leaves, sorted.
    pub(crate) fn order(&mut self, table: &Table) -> Vec<usize> {
        // (Worked out once for a table, filter and sort: a large table is not sorted again at every frame.)
        let id = (std::ptr::from_ref(table) as usize, table.rows.len());
        if let Some((p, n, filter, sort, rows)) = &self.shown
            && (*p, *n) == id
            && *filter == self.filter
            && *sort == self.sort
        {
            return rows.clone();
        }
        let rows = table.filtered(&self.filter);
        let rows = match self.sort.as_ref().and_then(|(label, asc)| table.column_index(label).map(|c| (c, *asc))) {
            Some((c, asc)) => table.sorted_rows(&rows, c, asc),
            None => rows,
        };
        self.shown = Some((id.0, id.1, self.filter.clone(), self.sort.clone(), rows.clone()));
        rows
    }

    /// Bring the selection into view at the next frame.
    pub(crate) fn reveal_selection(&mut self) {
        self.reveal = true;
    }

    /// The selected rectangle (first row, last row, first column, last column), in positions shown.
    pub(crate) fn rect(&self) -> Option<(usize, usize, usize, usize)> {
        let (a, h) = (self.anchor?, self.head?);
        Some((a.row.min(h.row), a.row.max(h.row), a.col.min(h.col), a.col.max(h.col)))
    }
}

/// The rows shown (the table they were worked out for, its row count, the filter and the sort, the rows).
type Shown = (usize, usize, String, Option<(String, bool)>, Vec<usize>);

/// What the user did to a table in a frame.
#[derive(Clone, Debug)]
pub(crate) enum TableDo {
    Select(Pos, bool),
    SelectRow(usize, bool),
    Sort(usize),
    Begin(Pos, Option<String>),
    Commit(Option<(isize, isize)>),
    Cancel,
    Clear,
    Drill(Vec<usize>, Option<String>),
    Copy,
    Paste(String),
    InsertRow(Option<usize>),
    Duplicate(usize),
    DeleteRows(Vec<usize>),
    MoveRow(usize, Option<usize>),
    ApplyOrder,
    StartNaming(Naming),
    FinishNaming,
    CancelNaming,
    DeleteColumn(usize),
    Move(isize, isize, bool),
}

/// The width of each column: enough for its header and the first values, within bounds.
fn widths(table: &Table) -> Vec<f32> {
    table
        .columns
        .iter()
        .enumerate()
        .map(|(c, col)| {
            let longest = table.rows.iter().take(60).filter_map(|r| r.cells.get(c)).map(|cell| cell.text.chars().count()).max().unwrap_or(0);
            let chars = longest.max(col.label().chars().count() + 2) as f32;
            (chars * 7.6 + 20.0).clamp(64.0, 300.0)
        })
        .collect()
}

/// How tall a table drawn inside the tree is.
pub(crate) fn inline_height(table: &Table, view: &mut TableView) -> f32 {
    let shown = view.order(table).len().min(INLINE_ROWS);
    BAR_H + HEAD_H + shown as f32 * CELL_H + 14.0
}

/// The label a screen reader gets for a cell.
fn cell_label(table: &Table, row: usize, col: usize) -> String {
    let column = table.columns.get(col).map(|c| c.label()).unwrap_or_default();
    let cell = table.rows.get(row).and_then(|r| r.cells.get(col));
    let value = match cell.map(|c| c.kind) {
        Some(CellKind::Missing) | None => tl!("empty").to_owned(),
        _ => cell.map(|c| c.text.clone()).unwrap_or_default(),
    };
    format!("{} {}, {column}: {value}", tl!("Row"), row + 1)
}

/// The bar above a table: its counts, the filter, the sort note, a column being named. In the focus view the breadcrumb is drawn before
/// it by the caller.
pub(crate) fn bar(ui: &mut egui::Ui, t: &Tokens, table: &Table, view: &mut TableView) -> Option<TableDo> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.set_height(BAR_H - 6.0);
        let shown = view.order(table).len();
        ui.add_space(8.0);
        let mut counts = if shown == table.rows.len() {
            let rows = if table.rows.len() == 1 { tl!("1 row").to_owned() } else { fmt(tl!("{n} rows"), &[("n", &table.rows.len().to_string())]) };
            let cols = if table.columns.len() == 1 {
                tl!("1 column").to_owned()
            } else {
                fmt(tl!("{n} columns"), &[("n", &table.columns.len().to_string())])
            };
            format!("{rows} · {cols}")
        } else {
            fmt(tl!("{shown} of {rows} rows"), &[("shown", &shown.to_string()), ("rows", &table.rows.len().to_string())])
        };
        if !table.is_whole() {
            counts.push_str(&fmt(
                tl!(" · cut at {rows} of {total}"),
                &[("rows", &table.rows.len().to_string()), ("total", &table.total_rows.to_string())],
            ));
        }
        if table.hidden_columns > 0 {
            counts.push_str(&fmt(tl!(" · {n} more columns not shown"), &[("n", &table.hidden_columns.to_string())]));
        }
        ui.label(egui::RichText::new(counts).color(t.text_muted).font(theme::regular(12.0)));
        match &mut view.naming {
            Some(naming) => {
                let (prompt, text) = match naming {
                    Naming::Rename(c, text) => {
                        (fmt(tl!("Rename column {name} to"), &[("name", &table.columns.get(*c).map(|c| c.label()).unwrap_or_default())]), text)
                    }
                    Naming::Insert(_, text) => (tl!("New column (@name for an XML attribute)").to_owned(), text),
                };
                ui.label(egui::RichText::new(prompt).color(t.text));
                let out = ui.add(egui::TextEdit::singleline(text).desired_width(160.0).font(theme::mono(12.5)));
                crate::widgets::name_for_assistive_technology(ui, &out, tl!("The column name"));
                if !out.has_focus() && !out.lost_focus() {
                    out.request_focus();
                }
                if out.lost_focus() {
                    action = Some(if ui.input(|i| i.key_pressed(egui::Key::Escape)) { TableDo::CancelNaming } else { TableDo::FinishNaming });
                }
            }
            None => {
                ui.add_space(8.0);
                let filter = ui
                    .add(egui::TextEdit::singleline(&mut view.filter).hint_text(tl!("Filter rows")).desired_width(150.0).font(theme::regular(12.0)));
                crate::widgets::name_for_assistive_technology(ui, &filter, tl!("Filter the rows of the table"));
                if view.sort.is_some() {
                    ui.label(egui::RichText::new(tl!("sorted view")).color(t.text_muted).font(theme::regular(12.0)));
                    if ui.small_button(tl!("Apply order to document")).clicked() {
                        action = Some(TableDo::ApplyOrder);
                    }
                }
            }
        }
    });
    action
}

/// Draw a table in `height` (the rest of the view when drilled in) and say what was done to it.
pub(crate) fn draw(ui: &mut egui::Ui, t: &Tokens, key: &TableKey, table: &Table, view: &mut TableView, height: f32) -> Option<TableDo> {
    let order = view.order(table);
    let widths = widths(table);
    let mut lefts = Vec::with_capacity(widths.len());
    let mut x = NUMBER_W;
    for w in &widths {
        lefts.push(x);
        x += w;
    }
    let total_w = x;
    let total_h = HEAD_H + order.len() as f32 * CELL_H;
    let mut action: Option<TableDo> = None;
    let rect = view.rect();
    let mut area = egui::ScrollArea::both().id_salt(("grid-table", key)).max_height(height).auto_shrink([false, true]);
    if view.reveal {
        view.reveal = false;
        if let Some(head) = view.head {
            // Scrolled so the cell is in view (the area clamps what is too far).
            let y = (head.row as f32 * CELL_H - height / 2.0).max(0.0);
            let x = lefts.get(head.col).copied().unwrap_or(0.0) - NUMBER_W;
            area = area.scroll_offset(egui::vec2(x.max(0.0), y));
        }
    }
    area.show_viewport(ui, |ui, viewport| {
        let (whole, _) = ui.allocate_exact_size(egui::vec2(total_w.max(ui.available_width()), total_h), egui::Sense::hover());
        let origin = whole.min;
        let painter = ui.painter().with_clip_rect(ui.clip_rect());
        // The rows and columns in view.
        let first = ((viewport.top() - HEAD_H) / CELL_H).floor().max(0.0) as usize;
        let last = (((viewport.bottom() - HEAD_H) / CELL_H).ceil().max(0.0) as usize).min(order.len());
        let cols: Vec<usize> = (0..widths.len())
            .filter(|&c| {
                let (l, w) = (lefts.get(c).copied().unwrap_or(0.0), widths.get(c).copied().unwrap_or(0.0));
                l + w >= viewport.left() + NUMBER_W && l <= viewport.right()
            })
            .collect();
        let sticky_left = origin.x + viewport.left();
        let sticky_top = origin.y + viewport.top();
        let editing = view.editing.clone();
        for pos in first..last {
            let Some(&row) = order.get(pos) else { continue };
            let Some(table_row) = table.rows.get(row) else { continue };
            let y = origin.y + HEAD_H + pos as f32 * CELL_H;
            for &c in &cols {
                let (Some(&l), Some(&w)) = (lefts.get(c), widths.get(c)) else { continue };
                let cell_rect = egui::Rect::from_min_size(egui::pos2(origin.x + l, y), egui::vec2(w, CELL_H));
                let Some(cell) = table_row.cells.get(c) else { continue };
                let selected = rect.is_some_and(|(r0, r1, c0, c1)| (r0..=r1).contains(&pos) && (c0..=c1).contains(&c));
                if selected {
                    painter.rect_filled(cell_rect, 0.0, t.accent_soft);
                }
                painter.rect_stroke(cell_rect, 0.0, egui::Stroke::new(0.5, t.divider), egui::StrokeKind::Inside);
                if let Some(e) = editing.as_ref().filter(|e| e.row == row && e.col == c) {
                    let mut text = e.text.clone();
                    let out =
                        ui.put(cell_rect.shrink(1.0), egui::TextEdit::singleline(&mut text).font(theme::mono(12.5)).margin(egui::vec2(4.0, 2.0)));
                    crate::widgets::name_for_assistive_technology(ui, &out, tl!("The value of the cell"));
                    if let Some(ed) = view.editing.as_mut() {
                        ed.text = text;
                        if ed.focus {
                            out.request_focus();
                            ed.focus = false;
                        }
                    }
                    if out.lost_focus() {
                        let (esc, tab, shift) = ui.input(|i| (i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::Tab), i.modifiers.shift));
                        action = Some(if esc {
                            TableDo::Cancel
                        } else if tab {
                            TableDo::Commit(Some((0, if shift { -1 } else { 1 })))
                        } else {
                            TableDo::Commit(None)
                        });
                    }
                    continue;
                }
                let response = ui.interact(cell_rect, egui::Id::unique("xmlcraft-grid-cell").with((key, row, c)), egui::Sense::click());
                response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Cell, true, cell_label(table, row, c)));
                let centre = cell_rect.left_center() + egui::vec2(6.0, 0.0);
                let clip = painter.with_clip_rect(cell_rect.intersect(ui.clip_rect()));
                match cell.kind {
                    CellKind::Scalar => {
                        let shown: String = cell.text.chars().take(200).collect::<String>().replace(['\n', '\r'], " ");
                        let numeric = table.columns.get(c).is_some_and(|col| col.numeric);
                        if numeric {
                            clip.text(cell_rect.right_center() - egui::vec2(6.0, 0.0), egui::Align2::RIGHT_CENTER, shown, theme::mono(12.5), t.text);
                        } else {
                            clip.text(centre, egui::Align2::LEFT_CENTER, shown, theme::mono(12.5), t.text);
                        }
                    }
                    CellKind::Missing => {
                        clip.text(centre, egui::Align2::LEFT_CENTER, "—", theme::regular(12.0), t.text_faint);
                    }
                    CellKind::Nested => {
                        let galley = ui.painter().layout_no_wrap(cell.text.clone(), theme::mono(11.5), t.accent);
                        let chip = egui::Rect::from_min_size(centre - egui::vec2(0.0, 8.0), egui::vec2(galley.size().x + 10.0, 16.0));
                        clip.rect_filled(chip, 8.0, t.accent_soft);
                        clip.galley(chip.min + egui::vec2(5.0, (16.0 - galley.size().y) / 2.0), galley, t.accent);
                        // A click on the cell (the chip is a small target) opens what it holds.
                        if response.clicked()
                            && let Some(d) = &cell.drill
                        {
                            action = Some(TableDo::Drill(d.path.clone(), d.name.clone()));
                        }
                    }
                }
                if response.double_clicked() {
                    action = Some(TableDo::Begin(Pos { row: pos, col: c }, None));
                } else if response.clicked() && action.is_none() {
                    action = Some(TableDo::Select(Pos { row: pos, col: c }, ui.input(|i| i.modifiers.shift)));
                }
                if response.secondary_clicked() {
                    view.menu_row = Some(pos);
                    if !rect.is_some_and(|(r0, r1, c0, c1)| (r0..=r1).contains(&pos) && (c0..=c1).contains(&c)) {
                        action = Some(TableDo::Select(Pos { row: pos, col: c }, false));
                    }
                }
                response.context_menu(|ui| row_menu(ui, view.menu_row.unwrap_or(pos), &order, view.sort.is_some(), &mut action));
            }
            // The row number, which stays at the left.
            let number = egui::Rect::from_min_size(egui::pos2(sticky_left, y), egui::vec2(NUMBER_W, CELL_H));
            let row_selected = rect.is_some_and(|(r0, r1, _, _)| (r0..=r1).contains(&pos));
            painter.rect_filled(number, 0.0, if row_selected { t.selected } else { t.chrome });
            painter.text(
                number.right_center() - egui::vec2(6.0, 0.0),
                egui::Align2::RIGHT_CENTER,
                (row + 1).to_string(),
                theme::regular(11.5),
                t.text_muted,
            );
            let response = ui.interact(number, egui::Id::unique("xmlcraft-grid-number").with((key, row)), egui::Sense::click());
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::RowHeader, true, format!("{} {}", tl!("Row"), row + 1)));
            if response.clicked() {
                action = Some(TableDo::SelectRow(pos, ui.input(|i| i.modifiers.shift)));
            }
            if response.secondary_clicked() {
                view.menu_row = Some(pos);
            }
            response.context_menu(|ui| row_menu(ui, view.menu_row.unwrap_or(pos), &order, view.sort.is_some(), &mut action));
        }
        // The header, which stays at the top.
        let head = egui::Rect::from_min_size(egui::pos2(origin.x + viewport.left(), sticky_top), egui::vec2(viewport.width(), HEAD_H));
        painter.rect_filled(head, 0.0, t.chrome);
        painter.hline(head.x_range(), head.bottom(), egui::Stroke::new(1.0, t.divider));
        for &c in &cols {
            let (Some(&l), Some(&w), Some(col)) = (lefts.get(c), widths.get(c), table.columns.get(c)) else { continue };
            let cell = egui::Rect::from_min_size(egui::pos2(origin.x + l, sticky_top), egui::vec2(w, HEAD_H));
            let sorted = view.sort.as_ref().filter(|(label, _)| *label == col.label()).map(|(_, asc)| *asc);
            let response = ui.interact(cell, egui::Id::unique("xmlcraft-grid-head").with((key, c)), egui::Sense::click());
            let state = match sorted {
                Some(true) => tl!(", sorted ascending"),
                Some(false) => tl!(", sorted descending"),
                None => "",
            };
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ColumnHeader, true, format!("{} {}{state}", tl!("Column"), col.label())));
            if response.hovered() {
                painter.rect_filled(cell, 0.0, t.hover);
            }
            let clip = painter.with_clip_rect(cell.intersect(ui.clip_rect()));
            clip.text(cell.left_center() + egui::vec2(6.0, 0.0), egui::Align2::LEFT_CENTER, col.label(), theme::semibold(12.0), t.text);
            if let Some(asc) = sorted {
                let mark = egui::Rect::from_center_size(cell.right_center() - egui::vec2(10.0, 0.0), egui::vec2(12.0, 12.0));
                crate::icons::paint(ui, mark, if asc { "chevron-up" } else { "chevron-down" }, 12.0, t.accent);
            }
            painter.vline(cell.right(), cell.y_range(), egui::Stroke::new(0.5, t.divider));
            if response.clicked() {
                action = Some(TableDo::Sort(c));
            }
            response.context_menu(|ui| {
                if ui.button(tl!("Rename Column…")).clicked() {
                    action = Some(TableDo::StartNaming(Naming::Rename(c, col.label())));
                    ui.close();
                }
                if ui.button(tl!("Insert Column…")).clicked() {
                    action = Some(TableDo::StartNaming(Naming::Insert(Some(c), String::new())));
                    ui.close();
                }
                if ui.button(tl!("Delete Column")).clicked() {
                    action = Some(TableDo::DeleteColumn(c));
                    ui.close();
                }
            });
        }
        // The corner over the row numbers.
        let corner = egui::Rect::from_min_size(egui::pos2(sticky_left, sticky_top), egui::vec2(NUMBER_W, HEAD_H));
        painter.rect_filled(corner, 0.0, t.chrome);
        painter.text(corner.right_center() - egui::vec2(6.0, 0.0), egui::Align2::RIGHT_CENTER, "#", theme::semibold(11.5), t.text_faint);
    });
    action
}

fn row_menu(ui: &mut egui::Ui, pos: usize, order: &[usize], sorted: bool, action: &mut Option<TableDo>) {
    let Some(&row) = order.get(pos) else { return };
    let mut item = |ui: &mut egui::Ui, label: &str, enabled: bool, what: TableDo| {
        if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
            *action = Some(what);
            ui.close();
        }
    };
    item(ui, tl!("Insert Row Above"), true, TableDo::InsertRow(row.checked_sub(1)));
    item(ui, tl!("Insert Row Below"), true, TableDo::InsertRow(Some(row)));
    item(ui, tl!("Duplicate Row"), true, TableDo::Duplicate(row));
    item(ui, tl!("Delete Row"), true, TableDo::DeleteRows(vec![row]));
    ui.separator();
    // Moving is in the order of the text, which a sorted view does not show.
    item(ui, tl!("Move Row Up"), !sorted && row > 0, TableDo::MoveRow(row, row.checked_sub(1)));
    item(ui, tl!("Move Row Down"), !sorted && row + 1 < order.len(), TableDo::MoveRow(row, (row + 2 < order.len()).then_some(row + 2)));
}

/// The keys of a table that has the selection (no field has the keyboard).
pub(crate) fn keys(ui: &egui::Ui, view: &TableView) -> Option<TableDo> {
    let head = view.head?;
    let mut action = None;
    ui.input(|i| {
        let shift = i.modifiers.shift;
        for event in &i.events {
            match event {
                egui::Event::Copy => action = Some(TableDo::Copy),
                egui::Event::Paste(text) => action = Some(TableDo::Paste(text.clone())),
                egui::Event::Text(text) if !i.modifiers.command && !text.chars().all(char::is_control) => {
                    action = Some(TableDo::Begin(head, Some(text.clone())));
                }
                _ => {}
            }
        }
        if action.is_some() {
            return;
        }
        let pressed = |k| i.key_pressed(k);
        action = if pressed(egui::Key::ArrowDown) {
            Some(TableDo::Move(1, 0, shift))
        } else if pressed(egui::Key::ArrowUp) {
            Some(TableDo::Move(-1, 0, shift))
        } else if pressed(egui::Key::ArrowRight) || (pressed(egui::Key::Tab) && !shift) {
            Some(TableDo::Move(0, 1, shift && !pressed(egui::Key::Tab)))
        } else if pressed(egui::Key::ArrowLeft) || (pressed(egui::Key::Tab) && shift) {
            Some(TableDo::Move(0, -1, shift && !pressed(egui::Key::Tab)))
        } else if pressed(egui::Key::Enter) || pressed(egui::Key::F2) {
            Some(TableDo::Begin(head, None))
        } else if pressed(egui::Key::Delete) || pressed(egui::Key::Backspace) {
            Some(TableDo::Clear)
        } else {
            None
        };
    });
    action
}

/// The edits of a table action, as one undoable step of the document `id`: applied, the caret put on what changed, or the reason why
/// not in the status line.
impl XmlCraftApp {
    pub(crate) fn apply_table_edits(&mut self, id: u64, edits: Result<Vec<Edit>, GridError>, failed: &str) -> bool {
        let edits = match edits {
            Ok(e) => e,
            Err(why) => {
                self.status = fmt(failed, &[("why", &why.to_string())]);
                return false;
            }
        };
        if edits.is_empty() {
            return true;
        }
        let Some(index) = self.session.position_of(id) else { return false };
        let Some(doc) = self.session.document_mut(index) else { return false };
        let editor = doc.editor_mut();
        editor.break_undo_group();
        editor.apply_edits(&edits);
        editor.break_undo_group();
        // The caret on the longest piece written (where it is in the new text: after what the edits before it changed).
        let mut shift: isize = 0;
        let mut best: Option<(usize, usize)> = None;
        for e in &edits {
            let start = e.range.start.saturating_add_signed(shift);
            if best.is_none_or(|(_, len)| e.text.len() > len) {
                best = Some((start, e.text.len()));
            }
            shift += e.delta();
        }
        if let Some((start, len)) = best {
            editor.set_selection(start, start + len);
        }
        self.status.clear();
        true
    }
}

/// The table at `key` with the columns added in the view (those still not in the text).
pub(crate) fn with_extra(grid: &Grid, text: &str, table: Table, view: &mut TableView) -> Table {
    let mut table = table;
    view.extra.retain(|(label, after)| {
        if table.column_index(label).is_some() {
            return false;
        }
        let after = after.as_ref().and_then(|a| table.column_index(a));
        match grid.insert_column(text, &table, label, after) {
            Ok(t) => {
                table = t;
                true
            }
            Err(_) => false,
        }
    });
    table
}
