//! Tool windows: dock panels that are not the editor, each with its form state in the app (not in a dialog), drawn in the frame
//! of `kit::tool_body` and `kit::tool_footer`. Which panel each one is, and where it docks, is in the panel table
//! (`workbench::Panel::spec`).
//!
//! A hidden tool keeps its form and its last result, and an answer from a worker lands in the tool whatever else is open. A tool
//! follows the document being shown ([`bound`]) and never holds a snapshot of it. This module holds what the tools share.

use crate::i18n::{fmt, tl};
use crate::kit;
use crate::{Panel, XmlCraftApp, widgets};

pub(crate) mod compare;
pub(crate) mod query;
pub(crate) mod search;

/// The document a tool follows: the one being shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Bound {
    pub id: u64,
    pub name: String,
    /// The text is XML (not JSON, YAML or HTML).
    pub xml: bool,
}

/// The document being shown, if there is one. Looked at every frame: a tool never holds a snapshot of it.
pub(crate) fn bound(app: &XmlCraftApp) -> Option<Bound> {
    let doc = app.session.active_document()?;
    Some(Bound { id: doc.id(), name: doc.name().to_owned(), xml: doc.language().is_xml() })
}

/// The line at the top of a tool that says which document it works on: `→ order.xml`.
pub(crate) fn bound_line(ui: &mut egui::Ui, name: &str) {
    let t = crate::theme::Tokens::get(ui.ctx());
    ui.add(egui::Label::new(egui::RichText::new(format!("→ {name}")).font(crate::theme::medium(12.0)).color(t.text_muted)).truncate());
}

/// A compact empty state: what the tool needs.
pub(crate) fn needs(ui: &mut egui::Ui, title: &str, hint: &str) {
    widgets::empty_state(ui, kit::GAP_GROUP, title, hint);
}

/// The width from which a tool puts its form at the left and its results at the right.
pub(crate) const TWO_COLUMNS: f32 = 760.0;
/// The share of the width the form takes in two columns.
const FORM_SHARE: f32 = 0.4;

/// A form and its results: side by side in a wide panel (the form about 40 %, the results the rest, each as high as the panel), else
/// the form above the results, which take what is left. Both get the tool's `state`. Returns what both returned.
pub(crate) fn columns<S, A, B>(
    ui: &mut egui::Ui,
    state: &mut S,
    form: impl FnOnce(&mut egui::Ui, &mut S) -> A,
    results: impl FnOnce(&mut egui::Ui, &mut S) -> B,
) -> (A, B) {
    if ui.available_width() < TWO_COLUMNS {
        let a = form(ui, state);
        let b = results(ui, state);
        return (a, b);
    }
    let gap = kit::GAP_GROUP;
    let (width, height) = (ui.available_width(), ui.available_height());
    let left = (width * FORM_SHARE).floor();
    let right = (width - left - 2.0 * gap - 1.0).max(0.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let a = ui.allocate_ui_with_layout(egui::vec2(left, height), egui::Layout::top_down(egui::Align::Min), |ui| form(ui, state)).inner;
        ui.add_space(gap);
        ui.add(egui::Separator::default().vertical().spacing(0.0));
        ui.add_space(gap);
        let b = ui.allocate_ui_with_layout(egui::vec2(right, height), egui::Layout::top_down(egui::Align::Min), |ui| results(ui, state)).inner;
        (a, b)
    })
    .inner
}

/// Escape in a field of a tool gives the keyboard back to the editor (the panel stays open). egui takes the keyboard from the field
/// when Escape arrives, before the field draws, so what is asked is whether the panel had it at the end of the last frame.
///
/// Call before drawing the body of `panel` (whose `ui` this is): true when Escape is pressed now and the panel had the keyboard.
pub(crate) fn escape_pending(ui: &egui::Ui, panel: Panel) -> bool {
    let had = ui.ctx().data(|d| d.get_temp::<bool>(focus_key(panel))).unwrap_or(false);
    had && ui.input(|i| i.key_pressed(egui::Key::Escape))
}

/// Call after drawing the body of `panel`: whether the keyboard is in one of its fields, for [`escape_pending`] at the next frame.
pub(crate) fn note_focus(ui: &egui::Ui, panel: Panel) {
    let rect = ui.max_rect();
    let inside = ui.memory(|m| m.focused()).and_then(|id| ui.ctx().read_response(id)).is_some_and(|r| rect.contains(r.rect.center()));
    ui.ctx().data_mut(|d| d.insert_temp(focus_key(panel), inside));
}

fn focus_key(panel: Panel) -> egui::Id {
    egui::Id::unique("tool-had-keyboard").with(panel.key())
}

impl XmlCraftApp {
    /// Show a tool window where it goes (the shown tab, when it is in a group), or say in the status bar why not.
    pub(crate) fn show_tool(&mut self, panel: Panel) -> bool {
        match self.workbench.reveal(panel) {
            Ok(()) => true,
            Err(e) => {
                self.status = fmt(tl!("{panel} cannot be shown: {reason}"), &[("panel", panel.title()), ("reason", &e.to_string())]);
                false
            }
        }
    }
}
