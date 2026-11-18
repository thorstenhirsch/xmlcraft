//! The Profile and Debug modes of the Query tool's XPath tab.
//!
//! **Profile**: the expression is run once with the profiler (`xmlcraft_query::profile`, on the XPath worker) and each part of it is
//! listed with how often it was evaluated, its total and own time and a bar, as the hot spots (the costliest first) or as the tree of
//! the expression. A click on a row marks its part in the expression field; the parts that cost the most are tinted there.
//!
//! **Debug**: the expression runs in the debugger (`xmlcraft_query::debug`, on its own thread). The controls step into, over and out,
//! continue to the next breakpoint, pause and stop; the part being evaluated is marked in the expression field and the breakpoints are
//! underlined there. The context item (a click selects it in the text), the variables, the call stack and the output so far fill the
//! panel. Keys, while the keyboard is in the Query panel: F11 step into, Shift+F11 step out, F6 step over, F5 continue, F9 (or
//! Ctrl/⌘+B in the expression) toggle a breakpoint at the caret. F10 is Transform with XSLT…, so it is not used here.

use std::ops::Range;
use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, Stroke};
use xmlcraft_query::debug::{Action, DebugState, EventKind, Session, Status};
use xmlcraft_query::{Profile, ResultItem};

use crate::i18n::{fmt, tl};
use crate::kit::{self, Tone};
use crate::theme::Tokens;

/// The most rows of a profile drawn.
const ROWS: usize = 400;
/// The height of a row.
const ROW: f32 = 22.0;
/// The longest expression marked (a longer one is shown plain).
const MARKED_CHARS: usize = 20_000;

/// What the XPath tab does with the expression.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    #[default]
    Evaluate,
    Profile,
    Debug,
}

/// How a profile is listed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ProfileView {
    #[default]
    Hot,
    Tree,
}

/// A profile, and what it was asked on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProfileAnswer {
    pub doc: u64,
    pub version: u64,
    pub expression: String,
    pub profile: Profile,
}

/// A debugging session, shared by the copies of the state; it ends when the last one goes.
#[derive(Clone)]
pub(crate) struct Running(pub Arc<Session>);

impl std::fmt::Debug for Running {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Running")
    }
}

impl PartialEq for Running {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Running {}

/// The Debug mode: the session, what it showed last, and the breakpoints.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DebugTool {
    pub run: Option<Running>,
    /// The document and the version of its text the session runs on, and the expression.
    pub doc: u64,
    pub version: u64,
    pub expression: String,
    pub state: Option<DebugState>,
    /// Byte offsets in the expression.
    pub breakpoints: Vec<usize>,
    pub error: Option<String>,
}

impl DebugTool {
    pub(crate) fn running(&self) -> bool {
        self.run.is_some() && self.state.as_ref().is_none_or(|s| matches!(s.status, Status::Running | Status::Paused))
    }

    /// Add or take away the breakpoint at `offset`.
    pub(crate) fn toggle(&mut self, offset: usize) {
        match self.breakpoints.iter().position(|&b| b == offset) {
            Some(i) => {
                self.breakpoints.remove(i);
            }
            None => self.breakpoints.push(offset),
        }
        self.breakpoints.sort_unstable();
        if let Some(run) = &self.run {
            run.0.set_breakpoints(self.breakpoints.clone());
        }
    }
}

/// What to mark in the expression field.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Marks {
    /// A row chosen in a profile.
    pub selected: Option<Range<usize>>,
    /// Where the debugger is.
    pub current: Option<Range<usize>>,
    pub breakpoints: Vec<usize>,
    /// The share of the time each part took itself, outer parts first.
    pub heat: Vec<(Range<usize>, f32)>,
}

/// The expression laid out with its marks: a tint for the costly parts, the chosen or current part filled, breakpoints underlined.
pub(crate) fn marked(ui: &egui::Ui, text: &str, marks: &Marks, wrap: f32) -> LayoutJob {
    let t = Tokens::get(ui.ctx());
    let font = egui::FontId::monospace(13.0);
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    let base = TextFormat { font_id: font, color: t.text, ..TextFormat::default() };
    let plain = text.chars().count() > MARKED_CHARS;
    let (bad, _) = kit::tone_colors(&t, Tone::Bad);
    let heat_color = if t.dark() { Color32::from_rgb(0xE0, 0x7A, 0x30) } else { Color32::from_rgb(0xF0, 0x80, 0x20) };
    // A breakpoint is underlined over the word it is in front of (at least one character).
    let breakpoints: Vec<Range<usize>> = marks.breakpoints.iter().map(|&b| word_at(text, b)).collect();
    let style = |at: usize| -> (Color32, Stroke) {
        if plain {
            return (Color32::TRANSPARENT, Stroke::NONE);
        }
        let inside = |r: &Range<usize>| r.start <= at && at < r.end;
        let breakpoint = breakpoints.iter().any(&inside);
        let mut background = Color32::TRANSPARENT;
        if let Some((_, share)) = marks.heat.iter().rev().find(|(r, _)| inside(r)) {
            background = heat_color.gamma_multiply((0.08 + share * 0.5).min(0.6));
        }
        if marks.selected.as_ref().is_some_and(inside) || marks.current.as_ref().is_some_and(inside) {
            background = t.accent_soft;
        }
        let underline = if breakpoint {
            Stroke::new(2.0, bad)
        } else if marks.current.as_ref().is_some_and(inside) {
            Stroke::new(1.5, t.accent)
        } else {
            Stroke::NONE
        };
        (background, underline)
    };
    let mut start = 0usize;
    let mut current: Option<(Color32, Stroke)> = None;
    for (i, _) in text.char_indices() {
        let s = style(i);
        if current.is_some_and(|c| c != s) {
            if let (Some(piece), Some((background, underline))) = (text.get(start..i), current) {
                job.append(piece, 0.0, TextFormat { background, underline, ..base.clone() });
            }
            start = i;
        }
        current = Some(s);
    }
    if let (Some(piece), Some((background, underline))) = (text.get(start..), current) {
        job.append(piece, 0.0, TextFormat { background, underline, ..base.clone() });
    }
    if text.is_empty() {
        job.append("", 0.0, base);
    }
    job
}

/// The word that starts at `at` (a name, a number, a variable), or the one character there.
fn word_at(text: &str, at: usize) -> Range<usize> {
    let rest = text.get(at..).unwrap_or_default();
    let word = |c: char| c.is_alphanumeric() || matches!(c, '$' | '_' | '-' | '.' | ':' | '@');
    let length: usize = rest.chars().take_while(|c| word(*c)).map(char::len_utf8).sum();
    let length = if length == 0 { rest.chars().next().map_or(0, char::len_utf8) } else { length };
    at..at.saturating_add(length)
}

/// The heat of a profile: each part's own share of the time (or of the instructions without a clock), outer parts first.
pub(crate) fn heat(p: &Profile) -> Vec<(Range<usize>, f32)> {
    let cost = |n: &xmlcraft_query::ProfileNode| if p.timed { n.self_nanos } else { n.self_instructions };
    let total: u64 = p.nodes.iter().map(cost).sum();
    if total == 0 {
        return Vec::new();
    }
    let mut parts: Vec<(usize, Range<usize>, f32)> =
        p.nodes.iter().filter(|n| cost(n) > 0).map(|n| (n.depth, n.span.clone(), cost(n) as f32 / total as f32)).collect();
    parts.sort_by_key(|(depth, _, _)| *depth);
    parts.into_iter().map(|(_, r, s)| (r, s)).collect()
}

fn millis(nanos: u64) -> String {
    format!("{:.3}", nanos as f64 / 1_000_000.0)
}

/// The profile, filling the rest of the panel. Returns the span of the row clicked.
pub(crate) fn profile_view(
    ui: &mut egui::Ui,
    answer: &ProfileAnswer,
    view: &mut ProfileView,
    selected: Option<&Range<usize>>,
) -> Option<Range<usize>> {
    let p = &answer.profile;
    let t = Tokens::get(ui.ctx());
    let total = if p.timed {
        fmt(tl!("{instructions} engine instructions, {ms} ms"), &[("instructions", &p.instructions.to_string()), ("ms", &millis(p.nanos))])
    } else {
        fmt(tl!("{instructions} engine instructions"), &[("instructions", &p.instructions.to_string())])
    };
    let outcome = match &p.result {
        Ok(done) => fmt(tl!("{count} items."), &[("count", &done.count.to_string())]),
        Err(e) => e.to_string(),
    };
    kit::help(ui, &format!("{total}. {outcome}"));
    if p.stopped {
        kit::banner(ui, Tone::Warn, tl!("The run was stopped at its limit: the counts are those until then."));
    }
    kit::segmented(ui, view, &[(ProfileView::Hot, tl!("Hot spots")), (ProfileView::Tree, tl!("Tree"))]);
    ui.add_space(4.0);
    let order: Vec<usize> = match view {
        ProfileView::Hot => p.hot.clone(),
        ProfileView::Tree => (0..p.nodes.len()).collect(),
    };
    let cost = |n: &xmlcraft_query::ProfileNode| if p.timed { n.self_nanos } else { n.self_instructions };
    let most = order.iter().filter_map(|&i| p.nodes.get(i)).map(cost).max().unwrap_or(0).max(1);
    let mut clicked = None;
    // The header.
    // The columns from the right: hits, total, self and the bar, each wide enough for its numbers; the text has the rest.
    let columns = |rect: egui::Rect| {
        let widths = [52.0f32, 70.0, 70.0, 48.0];
        let scale = (rect.width() * 0.6 / widths.iter().sum::<f32>()).min(1.0);
        let mut right = rect.right() - widths[3] * scale;
        let bar = egui::Rect::from_min_max(egui::pos2(right + 6.0, rect.top()), egui::pos2(rect.right() - 4.0, rect.bottom()));
        let mut edges = [0.0f32; 3];
        for k in (0..3).rev() {
            edges[k] = right - 6.0;
            right -= widths[k] * scale;
        }
        (right, edges, bar)
    };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), egui::Sense::hover());
    let (_, edges, _) = columns(rect);
    let head = egui::FontId::proportional(11.0);
    ui.painter().text(rect.left_center() + egui::vec2(6.0, 0.0), egui::Align2::LEFT_CENTER, tl!("Expression"), head.clone(), t.text_faint);
    let heads = if p.timed { [tl!("Hits"), tl!("Total ms"), tl!("Self ms")] } else { [tl!("Hits"), tl!("Total instr."), tl!("Self instr.")] };
    for (i, h) in heads.iter().enumerate() {
        ui.painter().text(egui::pos2(edges[i], rect.center().y), egui::Align2::RIGHT_CENTER, h, head.clone(), t.text_faint);
    }
    egui::ScrollArea::vertical().id_salt("xmlcraft-xpath-profile").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for &i in order.iter().take(ROWS) {
            let Some(n) = p.nodes.get(i) else { continue };
            let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), egui::Sense::click());
            let (x, edges, bar_area) = columns(rect);
            let (total_cost, self_cost) = if p.timed {
                (millis(n.total_nanos), millis(n.self_nanos))
            } else {
                (n.total_instructions.to_string(), n.self_instructions.to_string())
            };
            let label = fmt(
                tl!("{text}  {hits} hits  total {total}  self {own}"),
                &[("text", &n.text), ("hits", &n.hits.to_string()), ("total", &total_cost), ("own", &self_cost)],
            );
            let chosen = selected == Some(&n.span);
            response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, chosen, label.clone()));
            if chosen {
                ui.painter().rect_filled(rect, 4.0, t.accent_soft);
            } else if response.hovered() {
                ui.painter().rect_filled(rect, 4.0, t.hover);
            }
            // The bar: the part's own cost against the costliest listed.
            let share = cost(n) as f32 / most as f32;
            let bar =
                egui::Rect::from_min_size(egui::pos2(bar_area.left(), rect.center().y - 4.0), egui::vec2(bar_area.width().max(0.0) * share, 8.0));
            ui.painter().rect_filled(bar, 2.0, t.accent.gamma_multiply(0.7));
            let indent = if *view == ProfileView::Tree { 12.0 * n.depth.min(12) as f32 } else { 0.0 };
            let text_rect = egui::Rect::from_min_max(rect.min, egui::pos2(x - 8.0, rect.max.y));
            ui.painter().with_clip_rect(text_rect).text(
                rect.left_center() + egui::vec2(6.0 + indent, 0.0),
                egui::Align2::LEFT_CENTER,
                &n.text,
                egui::FontId::monospace(12.5),
                t.text,
            );
            for (k, value) in [n.hits.to_string(), total_cost, self_cost].iter().enumerate() {
                ui.painter().text(
                    egui::pos2(edges[k], rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    value,
                    egui::FontId::monospace(12.0),
                    t.text_muted,
                );
            }
            if crate::theme::hand(response).clicked() {
                clicked = Some(n.span.clone());
            }
        }
    });
    clicked
}

/// What the Debug mode asked for in a frame.
#[derive(Default)]
pub(crate) struct DebugAsk {
    pub action: Option<Action>,
    pub pause: bool,
    pub stop: bool,
    pub jump: Option<Range<usize>>,
    /// Toggle a breakpoint at the caret.
    pub toggle: bool,
}

fn event_name(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Enter => tl!("evaluating"),
        EventKind::Iterate => tl!("next item of"),
        EventKind::Exit => tl!("finished"),
    }
}

/// One item: a line, clickable when it has a place in the current text.
fn item_line(ui: &mut egui::Ui, item: &ResultItem, prefix: &str, current: bool) -> Option<Range<usize>> {
    let t = Tokens::get(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), egui::Sense::click());
    let clickable = current && item.span.is_some();
    let text = format!("{prefix}{}", super::query::line(item));
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, clickable, text.clone()));
    if clickable && response.hovered() {
        ui.painter().rect_filled(rect, 4.0, t.hover);
    }
    ui.painter().with_clip_rect(rect).text(
        rect.left_center() + egui::vec2(6.0, 0.0),
        egui::Align2::LEFT_CENTER,
        &text,
        egui::FontId::monospace(12.5),
        if clickable { t.text } else { t.text_muted },
    );
    (clickable && crate::theme::hand(response).clicked()).then(|| item.span.clone()).flatten()
}

/// The Debug mode's controls and panes, filling the rest of the panel.
pub(crate) fn debug_view(ui: &mut egui::Ui, tool: &DebugTool, current_text: bool) -> DebugAsk {
    let mut ask = DebugAsk::default();
    let state = tool.state.as_ref();
    let status = state.map(|s| s.status.clone());
    let paused = status == Some(Status::Paused) && tool.run.is_some();
    let running = status == Some(Status::Running) && tool.run.is_some();
    ui.horizontal_wrapped(|ui| {
        let button = |ui: &mut egui::Ui, enabled: bool, label: &str| ui.add_enabled(enabled, egui::Button::new(label)).clicked();
        if button(ui, paused, tl!("Step into (F11)")) {
            ask.action = Some(Action::Into);
        }
        if button(ui, paused, tl!("Step over (F6)")) {
            ask.action = Some(Action::Over);
        }
        if button(ui, paused, tl!("Step out (⇧F11)")) {
            ask.action = Some(Action::Out);
        }
        if button(ui, paused, tl!("Continue (F5)")) {
            ask.action = Some(Action::Continue);
        }
        if button(ui, running, tl!("Pause")) {
            ask.pause = true;
        }
        if button(ui, tool.running(), tl!("Stop")) {
            ask.stop = true;
        }
        if button(ui, true, tl!("Toggle breakpoint (F9)")) {
            ask.toggle = true;
        }
    });
    if let Some(e) = &tool.error {
        kit::banner(ui, Tone::Bad, e);
    }
    let Some(state) = state else {
        kit::help(
            ui,
            tl!(
                "Start debugging to run the expression step by step. With breakpoints (Ctrl/⌘+B or F9 at the caret in the expression) it runs to the first one."
            ),
        );
        return ask;
    };
    match &state.status {
        Status::Running => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tl!("Running…"));
            });
        }
        Status::Paused => {
            if let Some(at) = &state.at {
                kit::help(
                    ui,
                    &fmt(
                        tl!("Paused, {what} {text} (stop {stops}, {instructions} instructions)"),
                        &[
                            ("what", event_name(at.kind)),
                            ("text", &at.text),
                            ("stops", &state.stops.to_string()),
                            ("instructions", &state.instructions.to_string()),
                        ],
                    ),
                );
            }
        }
        Status::Finished | Status::Stopped => {
            let how = if state.status == Status::Stopped { tl!("Stopped.") } else { tl!("Finished.") };
            let outcome = match &state.result {
                Some(Ok(done)) => fmt(tl!("{count} items."), &[("count", &done.count.to_string())]),
                Some(Err(e)) => e.to_string(),
                None => String::new(),
            };
            kit::help(ui, &format!("{how} {outcome}"));
        }
    }
    if !current_text {
        kit::help(ui, tl!("The document changed since the session started: its nodes are not selected in the text."));
    }
    let height = (ui.available_height() / 4.0 - 26.0).max(48.0);
    let pane = |ui: &mut egui::Ui, title: &str, salt: &str, add: &mut dyn FnMut(&mut egui::Ui)| {
        kit::section(ui, title, |ui| {
            egui::ScrollArea::vertical().id_salt(salt).max_height(height).auto_shrink([false, true]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                add(ui);
            });
        });
    };
    let mut jump = None;
    pane(ui, tl!("Context"), "xmlcraft-xdebug-context", &mut |ui| match &state.context {
        Some(c) => {
            let place = match (c.position, c.last) {
                (Some(p), Some(l)) => fmt(tl!("item {position} of {last}: "), &[("position", &p.to_string()), ("last", &l.to_string())]),
                _ => String::new(),
            };
            if let Some(r) = item_line(ui, &c.item, &place, current_text) {
                jump = Some(r);
            }
        }
        None => kit::help(ui, tl!("No context item here (an inline function has none).")),
    });
    pane(ui, tl!("Variables"), "xmlcraft-xdebug-variables", &mut |ui| {
        if state.variables.is_empty() {
            kit::help(ui, tl!("No variable read yet."));
        }
        for v in &state.variables {
            match v.items.first() {
                Some(first) => {
                    let more = if v.count > 1 { fmt(tl!(" (and {more} more)"), &[("more", &(v.count - 1).to_string())]) } else { String::new() };
                    if let Some(r) = item_line(ui, first, &format!("{} = ", v.name), current_text) {
                        jump = Some(r);
                    }
                    if !more.is_empty() {
                        kit::help(ui, &more);
                    }
                }
                None => kit::help(ui, &format!("{} = ()", v.name)),
            }
        }
    });
    pane(ui, tl!("Call stack"), "xmlcraft-xdebug-stack", &mut |ui| {
        for frame in &state.call_stack {
            ui.add(egui::Label::new(egui::RichText::new(&frame.function).monospace()).truncate());
        }
    });
    pane(ui, tl!("Output so far"), "xmlcraft-xdebug-output", &mut |ui| {
        let items: &[ResultItem] = match &state.result {
            Some(Ok(done)) => &done.items,
            _ => &state.partial,
        };
        if items.is_empty() {
            kit::help(ui, tl!("Nothing yet."));
        }
        for item in items {
            if let Some(r) = item_line(ui, item, "", current_text) {
                jump = Some(r);
            }
        }
        if state.partial_count > state.partial.len() && state.result.is_none() {
            kit::help(ui, &fmt(tl!("and {more} more"), &[("more", &(state.partial_count - state.partial.len()).to_string())]));
        }
    });
    ask.jump = jump;
    ask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_breakpoint_is_toggled_and_kept_in_order() {
        let mut tool = DebugTool::default();
        tool.toggle(7);
        tool.toggle(2);
        assert_eq!(tool.breakpoints, [2, 7]);
        tool.toggle(7);
        assert_eq!(tool.breakpoints, [2]);
    }

    #[test]
    fn a_breakpoint_marks_the_word_it_is_in_front_of() {
        let text = "//item[price > 10]/é";
        assert_eq!(word_at(text, 7), 7..12, "price");
        assert_eq!(word_at(text, 13), 13..14, "one character where no word starts");
        assert_eq!(word_at(text, 19), 19..21, "a character of two bytes");
        assert_eq!(word_at(text, 99), 99..99, "past the end: nothing");
    }
}
