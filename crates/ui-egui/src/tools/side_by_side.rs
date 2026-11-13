//! The side-by-side view of the Compare tool: the two compared texts in two read-only panes whose rows are aligned by
//! [`xmlcraft_diff::Aligned`] (a line only one side has faces a blank filler), coloured as the editor colours them, with the rows that
//! differ tinted (removed, added, changed; the parts of a changed line that differ stronger), a ribbon between the panes that maps every
//! hunk of the whole texts (a click scrolls there), and per hunk two buttons that copy it from one side to the other.
//!
//! Both panes are one scroll area, so they scroll together, vertically and horizontally. Only the rows in view are laid out.

use std::ops::Range;
use std::sync::Arc;

use egui::epaint::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId, Rect, Sense, pos2, vec2};
use xmlcraft_code_editor::LineStyler;
use xmlcraft_diff::{Aligned, AlignedHunk, Row, Side};
use xmlcraft_text::Buffer;

use crate::editor_view::{Colouring, XmlStyler};
use crate::i18n::tl;
use crate::kit::{self, Tone};
use crate::theme::{ThemeKind, Tokens};
use crate::widgets;

/// The height of a row.
pub(crate) const ROW_H: f32 = 18.0;
/// The size of the text, as in the list.
const FONT: f32 = 12.5;
/// The change ribbon between the panes.
const RIBBON: f32 = 14.0;
/// The column with the copy buttons, left of the ribbon.
const ACTIONS: f32 = 46.0;
/// The most characters of a line that are drawn (a longer line is cut, with an ellipsis).
const MAX_SHOWN: usize = 2000;
/// A tab stop.
const TAB: usize = 4;

/// The two compared texts and how each is coloured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Texts {
    pub left: Arc<str>,
    pub right: Arc<str>,
    pub colouring: (Colouring, Colouring),
}

/// What is worked out once for a comparison and kept while it is shown: the texts as buffers with their stylers (which remember what
/// they coloured), and the longest line.
struct Painting {
    serial: u64,
    theme: ThemeKind,
    left: (Buffer, XmlStyler),
    right: (Buffer, XmlStyler),
    widest: usize,
}

/// [`Painting`], kept in the tool's state: it is a cache, so a copy of the state starts without it and two states are equal whatever
/// they cached.
#[derive(Default)]
pub(crate) struct Cache(Option<Painting>);

impl Clone for Cache {
    fn clone(&self) -> Self {
        Cache(None)
    }
}

impl PartialEq for Cache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Cache {}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() { "Cache(painted)" } else { "Cache(empty)" })
    }
}

/// The state of the view between frames.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ViewState {
    /// The hunk Previous and Next change went to (an index of [`Aligned::hunks`]).
    pub current: Option<usize>,
    /// A row to bring into view at the next frame.
    pub scroll_to: Option<usize>,
    cache: Cache,
}

impl ViewState {
    /// Go to the next (or, `back`, the previous) hunk that is a difference of the documents, round at the ends. Returns whether there
    /// was one.
    pub(crate) fn step(&mut self, aligned: &Aligned, back: bool) -> bool {
        let significant: Vec<usize> = aligned.hunks.iter().enumerate().filter(|(_, h)| h.significant).map(|(i, _)| i).collect();
        let next = match (self.current, back) {
            (None, false) => significant.first(),
            (None, true) => significant.last(),
            (Some(c), false) => significant.iter().find(|&&i| i > c).or(significant.first()),
            (Some(c), true) => significant.iter().rev().find(|&&i| i < c).or(significant.last()),
        };
        let Some(&next) = next else { return false };
        self.current = Some(next);
        self.scroll_to = aligned.hunks.get(next).map(|h| h.first_row);
        true
    }

    /// "3 of 8": where the current hunk is among the differences, or how many there are.
    pub(crate) fn counter(&self, aligned: &Aligned) -> (Option<usize>, usize) {
        let total = aligned.significant_count();
        let at = self
            .current
            .filter(|c| aligned.hunks.get(*c).is_some_and(|h| h.significant))
            .map(|c| aligned.hunks.iter().take(c).filter(|h| h.significant).count() + 1);
        (at, total)
    }

    /// Forget the place (another comparison is shown).
    pub(crate) fn reset(&mut self) {
        self.current = None;
        self.scroll_to = None;
    }
}

/// What a person did in the view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// Show this line (0-based; the number of lines for the end of the text) of one side in its document.
    Go(Side, usize),
    /// Copy the hunk (an index of [`Aligned::hunks`]) to the `target` side.
    Copy { hunk: usize, target: Side },
}

/// Whether a hunk can be copied to each side: `Err` says why not.
pub(crate) struct Permits<'a> {
    pub to_left: Result<(), &'a str>,
    pub to_right: Result<(), &'a str>,
}

/// One side of a row as drawn.
struct Cell<'a> {
    line: Option<usize>,
    text: &'a str,
    parts: &'a [Range<usize>],
    wash: Option<Color32>,
}

/// A line as it is drawn: tabs to the next stop, cut at [`MAX_SHOWN`] characters; and for each byte of the line (up to where it is cut)
/// the byte and the column it is drawn at.
struct Shown {
    text: String,
    byte_at: Vec<usize>,
    column_at: Vec<usize>,
}

fn shown(line: &str) -> Shown {
    let mut out = Shown { text: String::with_capacity(line.len().min(MAX_SHOWN * 2)), byte_at: Vec::new(), column_at: Vec::new() };
    let mut column = 0usize;
    let mut cut = false;
    for (count, (i, c)) in line.char_indices().enumerate() {
        if count >= MAX_SHOWN {
            cut = true;
            break;
        }
        for _ in 0..c.len_utf8() {
            out.byte_at.push(out.text.len());
            out.column_at.push(column);
        }
        let _ = i;
        if c == '\t' {
            let width = TAB - column % TAB;
            out.text.extend(std::iter::repeat_n(' ', width));
            column += width;
        } else if c.is_control() {
            out.text.push('·');
            column += 1;
        } else {
            out.text.push(c);
            column += 1;
        }
    }
    out.byte_at.push(out.text.len());
    out.column_at.push(column);
    if cut {
        out.text.push('…');
    }
    out
}

impl Shown {
    fn byte(&self, at: usize) -> usize {
        self.byte_at.get(at).or(self.byte_at.last()).copied().unwrap_or(0)
    }

    fn column(&self, at: usize) -> usize {
        self.column_at.get(at).or(self.column_at.last()).copied().unwrap_or(0)
    }
}

fn styler(colouring: Colouring, theme: ThemeKind) -> XmlStyler {
    let mut s = XmlStyler::new(theme);
    s.set_syntax(colouring);
    s
}

fn widest(text: &str, lines: &[Range<usize>]) -> usize {
    lines.iter().map(|l| text.get(l.clone()).map_or(0, |t| t.chars().count().min(MAX_SHOWN + 1))).max().unwrap_or(0)
}

/// The view, filling the rest of the panel. `serial` names the comparison (the cache is made again for another). Returns what was done.
pub(crate) fn show(
    ui: &mut egui::Ui,
    state: &mut ViewState,
    aligned: &Aligned,
    texts: &Texts,
    serial: u64,
    permits: &Permits<'_>,
    active: bool,
) -> Option<Action> {
    let t = Tokens::get(ui.ctx());
    let theme = t.kind;
    if state.cache.0.as_ref().is_none_or(|p| p.serial != serial || p.theme != theme) {
        state.cache.0 = Some(Painting {
            serial,
            theme,
            left: (Buffer::from_text(&texts.left), styler(texts.colouring.0, theme)),
            right: (Buffer::from_text(&texts.right), styler(texts.colouring.1, theme)),
            widest: widest(&texts.left, &aligned.left_lines).max(widest(&texts.right, &aligned.right_lines)),
        });
    }
    let painting = state.cache.0.as_mut()?;
    let font = FontId::monospace(FONT);
    let char_w = ui.ctx().fonts_mut(|f| f.glyph_width(&font, '0')).max(1.0);
    let digits = aligned.left_lines.len().max(aligned.right_lines.len()).max(1).to_string().len();
    let gutter = digits as f32 * char_w + 14.0;
    let total_w = ui.available_width().max(2.0 * gutter + ACTIONS + RIBBON + 40.0);
    let pane_w = ((total_w - ACTIONS - RIBBON) / 2.0).floor();
    let text_w = pane_w - gutter;
    let rows = aligned.rows.len();
    let content_w = total_w + (painting.widest as f32 * char_w + 16.0 - text_w).max(0.0);
    let content_h = rows as f32 * ROW_H;
    let (good, bad, warn) = (kit::tone_colors(&t, Tone::Good), kit::tone_colors(&t, Tone::Bad), kit::tone_colors(&t, Tone::Warn));
    let neutral = t.hover;
    let mut scroll = egui::ScrollArea::both().id_salt("xmlcraft-compare-side-by-side").auto_shrink([false, false]);
    if let Some(row) = state.scroll_to.take() {
        scroll = scroll.vertical_scroll_offset((row as f32 - 3.0).max(0.0) * ROW_H);
    }
    let mut action = None;
    let current = state.current;
    scroll.show_viewport(ui, |ui, viewport| {
        ui.set_min_size(vec2(content_w, content_h));
        let area = Rect::from_min_size(ui.max_rect().min + viewport.min.to_vec2(), viewport.size());
        let painter = ui.painter().with_clip_rect(area);
        painter.rect_filled(area, 0.0, t.card);
        let left_pane = Rect::from_min_size(area.min, vec2(pane_w, area.height()));
        let actions = Rect::from_min_size(pos2(left_pane.right(), area.top()), vec2(ACTIONS, area.height()));
        let ribbon = Rect::from_min_size(pos2(actions.right(), area.top()), vec2(RIBBON, area.height()));
        let right_pane = Rect::from_min_size(pos2(ribbon.right(), area.top()), vec2(pane_w, area.height()));
        for pane in [left_pane, right_pane] {
            painter.rect_filled(Rect::from_min_size(pane.min, vec2(gutter, pane.height())), 0.0, t.panel);
            painter.vline(pane.left() + gutter, pane.y_range(), egui::Stroke::new(1.0, t.divider));
        }
        painter.rect_filled(actions, 0.0, t.panel);
        let first = ((viewport.min.y / ROW_H).floor().max(0.0) as usize).min(rows);
        let last = (((viewport.max.y / ROW_H).ceil().max(0.0) as usize) + 1).min(rows);
        let hunk_tone = |h: Option<&AlignedHunk>, row: &Row| -> (Option<Color32>, Option<Color32>, Color32) {
            let significant = h.is_some_and(|h| h.significant);
            match row {
                Row::Equal(..) => (None, None, Color32::TRANSPARENT),
                _ if !significant => (Some(neutral), Some(neutral), t.text_faint.gamma_multiply(0.3)),
                Row::Changed { .. } => (Some(warn.1), Some(warn.1), warn.0.gamma_multiply(0.28)),
                Row::LeftOnly(_) => (Some(bad.1), None, bad.0.gamma_multiply(0.28)),
                Row::RightOnly(_) => (None, Some(good.1), good.0.gamma_multiply(0.28)),
            }
        };
        for r in first..last {
            let Some(row) = aligned.rows.get(r) else { break };
            let y = area.top() + r as f32 * ROW_H - viewport.min.y;
            let hunk_index = aligned.hunk_of_row(r);
            let hunk = hunk_index.and_then(|i| aligned.hunks.get(i));
            let (left_wash, right_wash, strong) = hunk_tone(hunk, row);
            let empty: &[Range<usize>] = &[];
            let (left_parts, right_parts) = match row {
                Row::Changed { left_parts, right_parts, .. } => (left_parts.as_slice(), right_parts.as_slice()),
                Row::LeftOnly(_) | Row::RightOnly(_) => (empty, empty),
                Row::Equal(..) => (empty, empty),
            };
            let line_text = |text: &'_ str, lines: &[Range<usize>], line: Option<usize>| -> String {
                line.and_then(|l| lines.get(l)).and_then(|l| text.get(l.clone())).unwrap_or_default().to_owned()
            };
            let (left_text, right_text) =
                (line_text(&texts.left, &aligned.left_lines, row.left()), line_text(&texts.right, &aligned.right_lines, row.right()));
            let cells = [
                (left_pane, Cell { line: row.left(), text: &left_text, parts: left_parts, wash: left_wash }, &mut painting.left),
                (right_pane, Cell { line: row.right(), text: &right_text, parts: right_parts, wash: right_wash }, &mut painting.right),
            ];
            for (pane, cell, (buffer, styler)) in cells {
                let row_rect = Rect::from_min_size(pos2(pane.left(), y), vec2(pane.width(), ROW_H));
                let text_rect = Rect::from_min_max(pos2(pane.left() + gutter + 1.0, y), pos2(pane.right(), y + ROW_H));
                let Some(line) = cell.line else {
                    if !matches!(row, Row::Equal(..)) {
                        // The filler facing a line only the other side has.
                        painter.rect_filled(text_rect, 0.0, t.pasteboard);
                    }
                    continue;
                };
                if let Some(wash) = cell.wash {
                    painter.rect_filled(row_rect, 0.0, wash);
                }
                painter.text(
                    pos2(pane.left() + gutter - 6.0, y + ROW_H / 2.0),
                    egui::Align2::RIGHT_CENTER,
                    (line + 1).to_string(),
                    font.clone(),
                    t.text_faint,
                );
                let drawn = shown(cell.text);
                let x0 = text_rect.left() + 4.0 - viewport.min.x;
                let text_painter = painter.with_clip_rect(text_rect.intersect(area));
                for part in cell.parts {
                    let (a, b) = (drawn.column(part.start), drawn.column(part.end));
                    let r = Rect::from_min_max(pos2(x0 + a as f32 * char_w, y + 1.0), pos2(x0 + b.max(a + 1) as f32 * char_w, y + ROW_H - 1.0));
                    text_painter.rect_filled(r, 2.0, strong);
                }
                let mut job = LayoutJob::default();
                let mut at = 0usize;
                let plain = TextFormat { font_id: font.clone(), color: t.text, ..TextFormat::default() };
                for span in styler.style_line(buffer, line, cell.text) {
                    let (a, b) = (drawn.byte(span.range.start).max(at), drawn.byte(span.range.end));
                    if a >= b {
                        continue;
                    }
                    if let Some(before) = drawn.text.get(at..a) {
                        job.append(before, 0.0, plain.clone());
                    }
                    if let Some(inside) = drawn.text.get(a..b) {
                        job.append(inside, 0.0, TextFormat { color: span.color, ..plain.clone() });
                    }
                    at = b;
                }
                if let Some(rest) = drawn.text.get(at..) {
                    job.append(rest, 0.0, plain.clone());
                }
                let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
                text_painter.galley(pos2(x0, y + (ROW_H - galley.size().y) / 2.0), galley, t.text);
            }
            if matches!(row, Row::Equal(..)) {
                continue;
            }
            // The row, for a click (into the side clicked; a right click into the second) and for assistive technology.
            let whole = Rect::from_min_max(pos2(area.left(), y), pos2(area.right(), y + ROW_H)).intersect(area);
            let left_part = Rect::from_min_max(whole.min, pos2(actions.left(), whole.bottom()));
            let right_part = Rect::from_min_max(pos2(ribbon.right(), whole.top()), whole.max);
            let mark = match row {
                Row::Changed { .. } => "≠",
                Row::LeftOnly(_) => "−",
                Row::RightOnly(_) => "+",
                Row::Equal(..) => "",
            };
            let number = |l: Option<usize>| l.map_or_else(String::new, |l| (l + 1).to_string());
            let label = format!("{mark} {} {} │ {} {}", number(row.left()), left_text, number(row.right()), right_text);
            let (go_left, go_right) =
                (row.left().or(hunk.map(|h| h.left.start)).unwrap_or(0), row.right().or(hunk.map(|h| h.right.start)).unwrap_or(0));
            for (part, side, go) in [(left_part, Side::Left, go_left), (right_part, Side::Right, go_right)] {
                if part.width() <= 0.0 {
                    continue;
                }
                let response = ui.interact(part, ui.scope_id().with(("xmlcraft-compare-row", r, side == Side::Left)), Sense::click());
                if side == Side::Left {
                    let label = label.clone();
                    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, active, label.clone()));
                }
                if response.hovered() {
                    painter.rect_stroke(part.shrink(0.5), 0.0, egui::Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
                }
                if !active {
                    continue;
                }
                if response.secondary_clicked() {
                    action = Some(Action::Go(Side::Right, go_right));
                } else if response.clicked() {
                    action = Some(Action::Go(side, go));
                }
            }
        }
        // The copy buttons, at the first row of each hunk in view, and a bar beside the hunk Previous and Next went to.
        let start = aligned.hunks.partition_point(|h| h.last_row < first);
        for (i, hunk) in aligned.hunks.iter().enumerate().skip(start).take_while(|(_, h)| h.first_row < last) {
            let y = area.top() + hunk.first_row as f32 * ROW_H - viewport.min.y;
            if current == Some(i) {
                let bottom = area.top() + (hunk.last_row + 1) as f32 * ROW_H - viewport.min.y;
                painter.rect_filled(Rect::from_min_max(pos2(actions.left(), y), pos2(actions.left() + 3.0, bottom)), 1.0, t.accent);
            }
            let button = |ui: &mut egui::Ui, rect: Rect, text: &str, name: &str, permit: &Result<(), &str>| -> bool {
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                let enabled = active && permit.is_ok();
                let response = child.add_enabled(enabled, egui::Button::new(egui::RichText::new(text).size(12.0)).min_size(rect.size()));
                widgets::name_for_assistive_technology(&child, &response, name);
                let response = match permit {
                    Err(why) => response.on_disabled_hover_text(*why),
                    Ok(()) if !active => response.on_disabled_hover_text(tl!("A document changed since: compare again.")),
                    Ok(()) => response.on_hover_text(name),
                };
                response.clicked()
            };
            let to_right = Rect::from_min_size(pos2(actions.left() + 4.0, y + 1.0), vec2(19.0, ROW_H - 2.0));
            let to_left = Rect::from_min_size(pos2(actions.left() + 24.0, y + 1.0), vec2(19.0, ROW_H - 2.0));
            if button(ui, to_right, "→", tl!("Copy to the right"), &permits.to_right) {
                action = Some(Action::Copy { hunk: i, target: Side::Right });
            }
            if button(ui, to_left, "←", tl!("Copy to the left"), &permits.to_left) {
                action = Some(Action::Copy { hunk: i, target: Side::Left });
            }
        }
        // The ribbon: every hunk of the texts, at its place in the whole, and the part in view.
        painter.rect_filled(ribbon, 0.0, t.pasteboard);
        if rows > 0 {
            let scale = ribbon.height() / rows as f32;
            let mut last_pixel = f32::NEG_INFINITY;
            for (i, hunk) in aligned.hunks.iter().enumerate() {
                let y0 = ribbon.top() + hunk.first_row as f32 * scale;
                let y1 = (ribbon.top() + (hunk.last_row + 1) as f32 * scale).max(y0 + 2.0);
                if y1 <= last_pixel && current != Some(i) {
                    continue;
                }
                last_pixel = y1;
                let colour = match (hunk.significant, hunk.left.is_empty(), hunk.right.is_empty()) {
                    (false, _, _) => t.text_faint.gamma_multiply(0.5),
                    (true, true, _) => good.0,
                    (true, _, true) => bad.0,
                    _ => warn.0,
                };
                let inset = if current == Some(i) { 1.0 } else { 3.0 };
                painter.rect_filled(Rect::from_min_max(pos2(ribbon.left() + inset, y0), pos2(ribbon.right() - inset, y1)), 1.0, colour);
            }
            if content_h > area.height() {
                let v0 = ribbon.top() + viewport.min.y / content_h * ribbon.height();
                let v1 = ribbon.top() + viewport.max.y.min(content_h) / content_h * ribbon.height();
                painter.rect_stroke(
                    Rect::from_min_max(pos2(ribbon.left() + 0.5, v0), pos2(ribbon.right() - 0.5, v1.max(v0 + 4.0))),
                    2.0,
                    egui::Stroke::new(1.0, t.text_muted),
                    egui::StrokeKind::Inside,
                );
            }
            let response = ui.interact(ribbon, ui.scope_id().with("xmlcraft-compare-ribbon"), Sense::click());
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!("Change ribbon")));
            if response.clicked()
                && let Some(at) = response.interact_pointer_pos()
            {
                let row = (((at.y - ribbon.top()) / scale).max(0.0) as usize).min(rows.saturating_sub(1));
                state.scroll_to = Some(row);
                ui.ctx().request_repaint();
            }
        }
    });
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use xmlcraft_diff::{LineOptions, side_by_side};

    #[test]
    fn a_line_is_drawn_with_tabs_to_the_next_stop_and_its_bytes_map_to_columns() {
        let s = shown("a\tbü\tc");
        assert_eq!(s.text, "a   bü  c");
        assert_eq!(s.column(1), 1, "the tab starts at column 1");
        assert_eq!(s.column(2), 4, "b after the tab");
        assert_eq!(s.column(5), 6, "ü is two bytes, one column");
        assert_eq!(s.byte(5), "a   bü".len());
        assert_eq!(s.column(99), 9, "past the end is the end");
        let long = "x".repeat(MAX_SHOWN + 5);
        assert!(shown(&long).text.ends_with('…'));
    }

    #[test]
    fn next_and_previous_go_round_the_differences_and_skip_the_rest() {
        let mut a = side_by_side("a\nb\nc\nd\ne\n", "A\nb\nC\nd\nE\n", &LineOptions::default());
        a.hunks[1].significant = false;
        let mut s = ViewState::default();
        assert_eq!(s.counter(&a), (None, 2));
        assert!(s.step(&a, false));
        assert_eq!((s.current, s.counter(&a)), (Some(0), (Some(1), 2)));
        assert!(s.step(&a, false));
        assert_eq!((s.current, s.counter(&a)), (Some(2), (Some(2), 2)), "the hunk that is not a difference is skipped");
        assert!(s.step(&a, false));
        assert_eq!(s.current, Some(0), "round at the end");
        assert!(s.step(&a, true));
        assert_eq!(s.current, Some(2), "and back");
        assert_eq!(s.scroll_to, Some(4), "to its first row (the fifth)");
        let same = side_by_side("a\n", "a\n", &LineOptions::default());
        assert!(!ViewState::default().step(&same, false));
    }
}
