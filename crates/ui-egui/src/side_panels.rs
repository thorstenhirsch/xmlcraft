//! The panes around the document: the Outline on the left, the Entry Helpers on the right behind an icon rail, and the Messages along
//! the bottom, drawn with craft-ui task panes ([`craft_ui::panes`]) like PdfCraft's.
//!
//! On the right, outermost first: the rail, then the open pane, then the document. Panes are docked only; their sizes are
//! resizable and saved with the preferences.

use std::ops::RangeInclusive;

use craft_ui::panes::{Edge, PaneContent, PaneHost, PaneLabels, PaneRequest, PaneState, Rail, RailItem, RailRequest, SidePolicy};
use egui::{Align, Layout, Stroke, vec2};
use serde::{Deserialize, Serialize};
use xmlcraft_engine::analysis::Skipped;

use crate::i18n::tl;
use crate::theme::{self, Tokens};
use crate::{XmlCraftApp, icons, widgets};

/// Default width of the Outline pane.
pub const OUTLINE_WIDTH: f32 = 272.0;
/// Default width of the Entry Helpers pane.
pub const HELPERS_WIDTH: f32 = 330.0;
/// Default height of the Messages pane.
pub const MESSAGES_HEIGHT: f32 = 170.0;
const OUTLINE_RANGE: RangeInclusive<f32> = 240.0..=520.0;
const HELPERS_RANGE: RangeInclusive<f32> = 260.0..=520.0;
const MESSAGES_RANGE: RangeInclusive<f32> = 90.0..=600.0;

/// The panes [`PaneHost`] draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pane {
    Outline,
    EntryHelpers,
    Messages,
}

impl Pane {
    fn range(self) -> RangeInclusive<f32> {
        match self {
            Self::Outline => OUTLINE_RANGE,
            Self::EntryHelpers => HELPERS_RANGE,
            Self::Messages => MESSAGES_RANGE,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Outline => tl!("Outline"),
            Self::EntryHelpers => tl!("Entry Helpers"),
            Self::Messages => tl!("Messages"),
        }
    }
}

/// What the rail on the right can open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RightPanel {
    EntryHelpers,
}

/// The saved sizes of the panes: widths of the two side panes and the height of the Messages pane.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SidePanels {
    pub outline_width: f32,
    pub helpers_width: f32,
    pub messages_height: f32,
}

impl Default for SidePanels {
    fn default() -> Self {
        Self { outline_width: OUTLINE_WIDTH, helpers_width: HELPERS_WIDTH, messages_height: MESSAGES_HEIGHT }
    }
}

/// A width from settings or automation: finite, within the pane's limits.
fn clamp_width(pane: Pane, width: f32) -> Option<f32> {
    let range = pane.range();
    width.is_finite().then(|| width.clamp(*range.start(), *range.end()))
}

impl SidePanels {
    pub fn width(&self, pane: Pane) -> f32 {
        match pane {
            Pane::Outline => self.outline_width,
            Pane::EntryHelpers => self.helpers_width,
            Pane::Messages => self.messages_height,
        }
    }

    fn set_width(&mut self, pane: Pane, width: f32) {
        let Some(width) = clamp_width(pane, width) else { return };
        match pane {
            Pane::Outline => self.outline_width = width,
            Pane::EntryHelpers => self.helpers_width = width,
            Pane::Messages => self.messages_height = width,
        }
    }

    /// The sizes after loading: anything out of range or not a number is replaced by the pane's default or limit.
    pub fn sanitized(self) -> Self {
        Self {
            outline_width: clamp_width(Pane::Outline, self.outline_width).unwrap_or(OUTLINE_WIDTH),
            helpers_width: clamp_width(Pane::EntryHelpers, self.helpers_width).unwrap_or(HELPERS_WIDTH),
            messages_height: clamp_width(Pane::Messages, self.messages_height).unwrap_or(MESSAGES_HEIGHT),
        }
    }
}

struct Content<'a> {
    app: &'a mut XmlCraftApp,
}

impl PaneContent<Pane> for Content<'_> {
    fn title(&self, pane: &Pane) -> String {
        pane.title().to_owned()
    }

    fn body(&mut self, ui: &mut egui::Ui, pane: &Pane) {
        let close = pane_header(ui, pane.title());
        match pane {
            Pane::Outline => outline_body(ui, self.app),
            Pane::EntryHelpers => entry_helpers_body(ui),
            Pane::Messages => messages_body(ui, self.app),
        }
        if close {
            match pane {
                Pane::Outline => self.app.left_open = false,
                Pane::EntryHelpers => self.app.right = None,
                Pane::Messages => self.app.bottom_open = false,
            }
        }
    }

    fn default_state(&self, pane: &Pane) -> PaneState {
        state_of(&self.app.side_panels, *pane)
    }

    fn size_range(&self, pane: &Pane) -> RangeInclusive<f32> {
        pane.range()
    }

    fn movable(&self, _pane: &Pane) -> bool {
        false
    }

    fn title_bar(&self, _pane: &Pane) -> bool {
        // Both panes draw their own header and Close.
        false
    }
}

fn state_of(panels: &SidePanels, pane: Pane) -> PaneState {
    let edge = match pane {
        Pane::Outline => Edge::Start,
        Pane::EntryHelpers => Edge::End,
        Pane::Messages => Edge::Bottom,
    };
    PaneState::docked(edge, panels.width(pane))
}

/// The panes that show this frame, outermost first on a shared edge.
fn open_panes(app: &XmlCraftApp) -> Vec<Pane> {
    let mut open = Vec::new();
    if app.left_open {
        open.push(Pane::Outline);
    }
    if app.right == Some(RightPanel::EntryHelpers) {
        open.push(Pane::EntryHelpers);
    }
    if app.bottom_open {
        open.push(Pane::Messages);
    }
    open
}

/// A pane's own header: its title and a Close button. True when Close was clicked.
fn pane_header(ui: &mut egui::Ui, title: &str) -> bool {
    let t = Tokens::get(ui.ctx());
    let mut close = false;
    egui::Frame::NONE.inner_margin(egui::Margin { left: 16, right: 10, top: 12, bottom: 6 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(title).font(theme::semibold(15.0)).color(t.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                close = icons::button(ui, "x", 26.0, false, tl!("Close")).clicked();
            });
        });
    });
    close
}

fn pane_hint(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(16, 0)).show(ui, |ui| {
        ui.label(egui::RichText::new(text).font(theme::regular(12.0)).color(t.text_faint));
    });
}

/// Rows of a list: tall enough to click.
const ROW: f32 = 24.0;

/// The Outline's rows (indexes into the analysis's outline) for one filter, one document and one analysis; kept between frames so that
/// 50,000 names are not lowercased and compared again every frame.
pub(crate) struct OutlineRows {
    filter: String,
    document: u64,
    version: u64,
    rows: Vec<usize>,
}

fn outline_body(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    let t = Tokens::get(ui.ctx());
    let Some(doc) = app.session.active_document() else {
        widgets::empty_state(ui, 24.0, tl!("No document open"), tl!("The element tree of the open file appears here."));
        return;
    };
    let (caret, document) = (doc.editor().primary().head(), doc.id());
    egui::Frame::NONE.inner_margin(egui::Margin { left: 12, right: 12, top: 0, bottom: 6 }).show(ui, |ui| {
        let filter = ui.add(egui::TextEdit::singleline(&mut app.outline_filter).hint_text(tl!("Filter elements")).desired_width(f32::INFINITY));
        widgets::name_for_assistive_technology(ui, &filter, tl!("Filter elements"));
    });
    let filter = app.outline_filter.trim().to_lowercase();
    let cached = app.outline_rows.take();
    let Some(analysis) = app.active_analysis() else {
        widgets::empty_state(ui, 12.0, tl!("Reading the document…"), "");
        return;
    };
    match analysis.skipped {
        Some(Skipped::TooLarge) => {
            return widgets::empty_state(ui, 12.0, tl!("Not checked"), tl!("The document is too large for an outline while typing."));
        }
        Some(Skipped::Failed) => return widgets::empty_state(ui, 12.0, tl!("The check failed"), tl!("This is a bug. The document is unchanged.")),
        None => {}
    }
    if analysis.outline.is_empty() {
        widgets::empty_state(ui, 12.0, tl!("No elements"), tl!("The element tree appears once the document has a root element."));
        return;
    }
    let rows = match cached.filter(|c| c.filter == filter && c.document == document && c.version == analysis.version) {
        Some(rows) => rows,
        None => {
            let matching = analysis
                .outline
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    filter.is_empty()
                        || item.name.to_lowercase().contains(&filter)
                        || item.label.as_ref().is_some_and(|l| l.to_lowercase().contains(&filter))
                })
                .map(|(i, _)| i)
                .collect();
            OutlineRows { filter, document, version: analysis.version, rows: matching }
        }
    };
    // The innermost element around the caret.
    let current = analysis.outline.iter().rposition(|item| item.range.start <= caret && caret <= item.range.end);
    // Above the list, where it is seen (below a list that fills the pane it never would be).
    if analysis.outline_truncated {
        pane_hint(ui, tl!("Only the first elements are listed."));
    }
    let mut clicked = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW, rows.rows.len(), |ui, range| {
        for &index in rows.rows.get(range).unwrap_or_default() {
            let Some(item) = analysis.outline.get(index) else { continue };
            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), egui::Sense::click());
            let label = match &item.label {
                Some(label) => format!("{} {label}", item.name),
                None => item.name.clone(),
            };
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label.clone()));
            if Some(index) == current {
                ui.painter().rect_filled(rect, 0.0, t.selected);
            } else if response.hovered() {
                ui.painter().rect_filled(rect, 0.0, t.hover);
            }
            let indent = 14.0 + item.depth.min(20) as f32 * 12.0;
            let clip = ui.painter().with_clip_rect(rect);
            let name = clip.text(rect.left_center() + vec2(indent, 0.0), egui::Align2::LEFT_CENTER, &item.name, theme::medium(12.5), t.text);
            if let Some(hint) = &item.label {
                clip.text(egui::pos2(name.right() + 8.0, rect.center().y), egui::Align2::LEFT_CENTER, hint, theme::regular(12.0), t.text_faint);
            }
            if response.clicked() {
                clicked = Some(item.name_range.clone());
            }
        }
    });
    app.outline_rows = Some(rows);
    if let Some(range) = clicked {
        app.jump_to(range.start..range.start);
    }
}

fn messages_body(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    let t = Tokens::get(ui.ctx());
    let Some(doc) = app.session.active_document() else {
        widgets::empty_state(ui, 6.0, tl!("No messages"), tl!("Well-formedness and validation results appear here."));
        return;
    };
    let buffer = doc.editor().buffer();
    let Some(analysis) = app.active_analysis() else {
        widgets::empty_state(ui, 6.0, tl!("Checking…"), "");
        return;
    };
    match analysis.skipped {
        Some(Skipped::TooLarge) => {
            widgets::empty_state(ui, 6.0, tl!("Not checked"), tl!("The document is too large to check while typing."));
            return;
        }
        Some(Skipped::Failed) => {
            widgets::empty_state(ui, 6.0, tl!("The check failed"), tl!("This is a bug. The document is unchanged."));
            return;
        }
        None => {}
    }
    if analysis.diagnostics.is_empty() {
        widgets::empty_state(ui, 6.0, tl!("No problems"), tl!("The document is well-formed."));
        return;
    }
    let syntax = crate::editor_view::SyntaxTokens::for_kind(t.kind);
    let mut clicked = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW, analysis.diagnostics.len(), |ui, range| {
        for index in range {
            let Some(diagnostic) = analysis.diagnostics.get(index) else { continue };
            let position = buffer.position(diagnostic.span.start.min(buffer.len()));
            let message = diagnostic.message();
            let place = crate::i18n::fmt(
                tl!("Ln {line}, Col {col}"),
                &[("line", &(position.line + 1).to_string()), ("col", &(position.col + 1).to_string())],
            );
            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), egui::Sense::click());
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{message} ({place})")));
            if response.hovered() {
                ui.painter().rect_filled(rect, 0.0, t.hover);
            }
            let (icon, color) = match diagnostic.severity() {
                xmlcraft_syntax::Severity::Error => ("circle-x", syntax.error),
                xmlcraft_syntax::Severity::Warning => ("triangle-alert", syntax.warning),
                xmlcraft_syntax::Severity::Info => ("info", syntax.info),
            };
            icons::paint(ui, egui::Rect::from_min_size(rect.left_center() + vec2(12.0, -8.0), vec2(16.0, 16.0)), icon, 15.0, color);
            let place_galley = ui.painter().layout_no_wrap(place, theme::regular(11.5), t.text_faint);
            let right = rect.right() - 14.0;
            ui.painter().galley(
                egui::pos2(right - place_galley.size().x, rect.center().y - place_galley.size().y / 2.0),
                place_galley.clone(),
                t.text_faint,
            );
            let clip = ui.painter().with_clip_rect(egui::Rect::from_min_max(rect.min, egui::pos2(right - place_galley.size().x - 10.0, rect.max.y)));
            clip.text(rect.left_center() + vec2(36.0, 0.0), egui::Align2::LEFT_CENTER, message, theme::regular(12.5), t.text);
            if response.clicked() {
                clicked = Some(diagnostic.span.clone());
            }
        }
    });
    if let Some(span) = clicked {
        app.jump_to(span);
    }
}

fn entry_helpers_body(ui: &mut egui::Ui) {
    pane_hint(ui, tl!("What the schema allows at the cursor appears here."));
    for section in [tl!("Elements"), tl!("Attributes"), tl!("Entities")] {
        egui::Frame::NONE.inner_margin(egui::Margin::symmetric(16, 0)).show(ui, |ui| {
            widgets::section_title(ui, section);
            let t = Tokens::get(ui.ctx());
            ui.label(egui::RichText::new(tl!("Nothing to show")).font(theme::regular(12.5)).color(t.text_muted));
        });
    }
}

/// The rail on the right: one icon per pane that can open there.
fn right_rail(app: &mut XmlCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let items = [RailItem::new(RightPanel::EntryHelpers, icons::source("braces"), tl!("Entry Helpers"))];
    let rail = Rail::new("rail")
        .edge(Edge::End)
        .frame(egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin::symmetric(6, 8)).stroke(Stroke::new(1.0, t.divider)))
        .show(ui, &items, app.right.as_ref(), |_ui| {});
    match rail.request {
        Some(RailRequest::Open(panel)) => app.right = Some(panel),
        Some(RailRequest::Close(_)) => app.right = None,
        _ => {}
    }
}

/// Draws the rail and the open panes. Call after the top and bottom bars and before the central panel.
pub fn show(app: &mut XmlCraftApp, ui: &mut egui::Ui) {
    // The rail first: it is the outermost panel on the right.
    right_rail(app, ui);
    let open = open_panes(app);
    if open.is_empty() {
        return;
    }
    let t = Tokens::get(ui.ctx());
    let labels = PaneLabels { resize: tl!("Resize panels").to_owned(), ..PaneLabels::default() };
    let panels = app.side_panels;
    let output = PaneHost::new("xmlcraft-side-panels")
        .policy(SidePolicy::Columns)
        .f6(false)
        .labels(labels)
        .frame(egui::Frame::NONE.fill(t.panel))
        .body_margin(egui::Margin::ZERO)
        .show(ui, &open, |pane| Some(state_of(&panels, *pane)), &mut Content { app });
    for request in output.requests {
        match request {
            PaneRequest::Resize(pane, width) => app.side_panels.set_width(pane, width),
            PaneRequest::Close(Pane::Outline) => app.left_open = false,
            PaneRequest::Close(Pane::EntryHelpers) => app.right = None,
            PaneRequest::Close(Pane::Messages) => app.bottom_open = false,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_are_clamped_to_each_panes_limits() {
        let mut p = SidePanels::default();
        p.set_width(Pane::Outline, 9000.0);
        assert_eq!(p.outline_width, 520.0);
        p.set_width(Pane::Outline, 1.0);
        assert_eq!(p.outline_width, 240.0);
        p.set_width(Pane::EntryHelpers, 300.0);
        assert_eq!(p.helpers_width, 300.0);
    }

    #[test]
    fn non_finite_widths_are_ignored() {
        let mut p = SidePanels::default();
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            p.set_width(Pane::Outline, bad);
            p.set_width(Pane::EntryHelpers, bad);
        }
        assert_eq!(p, SidePanels::default());
    }

    #[test]
    fn a_damaged_saved_layout_is_repaired_on_load() {
        let loaded = SidePanels { outline_width: f32::NAN, helpers_width: 1.0e9, messages_height: -3.0 }.sanitized();
        assert_eq!(loaded, SidePanels { outline_width: OUTLINE_WIDTH, helpers_width: 520.0, messages_height: 90.0 });
        assert_eq!(SidePanels::default().sanitized(), SidePanels::default());
    }

    #[test]
    fn saved_layout_round_trips_and_tolerates_missing_fields() {
        let panels = SidePanels { outline_width: 300.0, helpers_width: 410.0, messages_height: 220.0 };
        let json = serde_json::to_string(&panels).expect("serialises");
        assert_eq!(serde_json::from_str::<SidePanels>(&json).expect("parses"), panels);
        assert_eq!(serde_json::from_str::<SidePanels>("{}").expect("parses"), SidePanels::default());
        assert_eq!(serde_json::from_str::<SidePanels>(r#"{"outline_width": 280.0}"#).expect("parses").helpers_width, HELPERS_WIDTH);
    }
}
