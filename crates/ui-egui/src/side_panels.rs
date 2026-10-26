//! The panes around the document: the Outline on the left, the Entry Helpers on the right behind an icon rail, and the Messages along
//! the bottom, drawn with craft-ui task panes ([`craft_ui::panes`]) like PdfCraft's.
//!
//! On the right, outermost first: the rail, then the open pane, then the document. Panes are docked only; their sizes are
//! resizable and saved with the preferences.

use std::ops::RangeInclusive;

use craft_ui::panes::{Edge, PaneContent, PaneHost, PaneLabels, PaneRequest, PaneState, Rail, RailItem, RailRequest, SidePolicy};
use egui::{Align, Layout, Stroke};
use serde::{Deserialize, Serialize};

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
            Pane::Outline => outline_body(ui),
            Pane::EntryHelpers => entry_helpers_body(ui),
            Pane::Messages => messages_body(ui),
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

fn outline_body(ui: &mut egui::Ui) {
    widgets::empty_state(ui, 24.0, tl!("No document open"), tl!("The element tree of the open file appears here."));
}

fn messages_body(ui: &mut egui::Ui) {
    // Well-formedness errors will come first, then validity errors; each row jumps to its place in the document.
    widgets::empty_state(ui, 6.0, tl!("No messages"), tl!("Well-formedness and validation results appear here."));
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
