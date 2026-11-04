//! The view shown when no document is open.

use std::path::{Path, PathBuf};

use egui::{Align, Layout, Rect, Sense, Stroke, vec2};

use crate::i18n::tl;
use crate::theme::{self, Tokens};

/// How many of the recent files the card lists.
const SHOWN_RECENT: usize = 8;

/// What was chosen on the home view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// A recent file.
    Open(PathBuf),
    /// A registered command (New, New from Template…, Open…).
    Command(&'static str),
}

/// The widest the content of the home view grows.
const MAX_WIDTH: f32 = 860.0;

/// The home view: a welcome, three ways to begin (a new document, one from a template, a file), the files opened lately, and the keys that
/// matter. The thing chosen is returned.
pub fn show(ui: &mut egui::Ui, recent: &[PathBuf]) -> Option<Choice> {
    let mut chosen = None;
    let t = Tokens::get(ui.ctx());
    let width = (ui.available_width() - 64.0).clamp(280.0, MAX_WIDTH);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.with_layout(Layout::top_down(Align::Center), |ui| {
            ui.add_space(40.0);
            ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(width);
                hero(ui, &t);
                ui.add_space(26.0);
                let cards = [
                    ("file-plus", tl!("New document"), tl!("Start with an empty XML document."), "file.new"),
                    ("layout-grid", tl!("New from Template…"), tl!("XML, a schema, a DTD, XHTML, XSLT or a catalog."), "file.new_from_template"),
                    ("folder-open", tl!("Open file…"), tl!("Open XML, a schema or a DTD. Or drop a file here."), "file.open"),
                ];
                let columns = if width >= 640.0 { 3 } else { 1 };
                let gap = 14.0;
                let card_w = (width - gap * (columns as f32 - 1.0)) / columns as f32;
                for row in cards.chunks(columns) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for (icon, title, text, command) in row {
                            if action_card(ui, &t, card_w, icon, title, text) {
                                chosen = Some(Choice::Command(command));
                            }
                        }
                    });
                    ui.add_space(gap);
                }
                ui.add_space(12.0);
                recent_files(ui, &t, recent, &mut chosen);
                ui.add_space(24.0);
                tips(ui);
                ui.add_space(32.0);
            });
        });
    });
    chosen
}

fn hero(ui: &mut egui::Ui, t: &Tokens) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        ui.add(egui::Image::new(egui::include_image!("../../../assets/app-icon/xmlcraft.svg")).fit_to_exact_size(vec2(64.0, 64.0)));
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(egui::RichText::new(crate::APP_NAME).font(theme::semibold(28.0)).color(t.text));
            ui.label(
                egui::RichText::new(tl!("Edit and validate XML against DTDs and XML Schemas, and generate schemas and samples."))
                    .font(theme::regular(13.5))
                    .color(t.text_muted),
            );
        });
    });
}

/// A way to begin: an icon tile, a title and one line under it. Lifts when the pointer is on it. True when clicked.
fn action_card(ui: &mut egui::Ui, t: &Tokens, width: f32, icon: &str, title: &str, text: &str) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 132.0), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, title));
    let hover =
        ui.ctx().animate_bool_with_time(response.id.with("lift"), response.hovered(), if crate::motion::animated(ui.ctx()) { 0.12 } else { 0.0 });
    let card = rect.translate(vec2(0.0, -2.0 * hover));
    if hover > 0.0 {
        ui.painter().add(egui::Shadow { offset: [0, 6], blur: 18, spread: 0, color: t.page_shadow.gamma_multiply(hover) }.as_shape(card, 12.0));
    }
    let edge = t.border.lerp_to_gamma(t.accent, hover * 0.8);
    ui.painter().rect(card, 12.0, t.card, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
    let tile = Rect::from_min_size(card.min + vec2(16.0, 16.0), vec2(40.0, 40.0));
    ui.painter().rect_filled(tile, 10.0, t.accent_soft);
    crate::icons::paint(ui, tile, icon, 20.0, t.accent_text);
    ui.painter().text(card.min + vec2(16.0, 66.0), egui::Align2::LEFT_TOP, title, theme::semibold(14.0), t.text);
    let galley = ui.painter().layout(text.to_owned(), theme::regular(12.0), t.text_muted, card.width() - 32.0);
    ui.painter().galley(card.min + vec2(16.0, 88.0), galley, t.text_muted);
    theme::hand(response).clicked()
}

fn recent_files(ui: &mut egui::Ui, t: &Tokens, recent: &[PathBuf], chosen: &mut Option<Choice>) {
    ui.label(egui::RichText::new(tl!("Recent files").to_uppercase()).font(theme::semibold(10.5)).color(t.text_faint).extra_letter_spacing(0.6));
    ui.add_space(8.0);
    if recent.is_empty() {
        crate::kit::card(ui, false, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(tl!("Files you open appear here")).font(theme::medium(13.0)).color(t.text_muted));
                ui.label(egui::RichText::new(tl!("Drop a file anywhere in the window to open it.")).font(theme::regular(12.0)).color(t.text_faint));
                ui.add_space(8.0);
            });
        });
        return;
    }
    egui::Frame::NONE.fill(t.card).stroke(Stroke::new(1.0, t.border)).corner_radius(10).inner_margin(egui::Margin::symmetric(6, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        for path in recent.iter().take(SHOWN_RECENT) {
            if recent_row(ui, t, path) {
                *chosen = Some(Choice::Open(path.clone()));
            }
        }
    });
}

/// The keys that matter, one line: what a person new to the app would otherwise not find. The keys are the registry's, so they are right
/// for the platform.
fn tips(ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 6.0);
        for (command, what) in
            [("xml.validate", tl!("validates")), ("xml.next_problem", tl!("goes to the next problem")), ("view.palette", tl!("finds any command"))]
        {
            let keys = crate::commands::command_shortcut_label(ui.ctx(), command);
            if !keys.is_empty() {
                let _ = crate::kit::chip(ui, crate::kit::Tone::Neutral, None, &format!("{keys}  {what}"), false);
            }
        }
    });
}

/// The folder a file is in, as its last two components (the whole path is in the tooltip): what tells two files of one name apart.
fn short_folder(path: &Path) -> String {
    let Some(parent) = path.parent() else { return String::new() };
    let names: Vec<String> = parent
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    match names.as_slice() {
        [] => parent.display().to_string(),
        [only] => only.clone(),
        [.., last_but_one, last] if names.len() == 2 => format!("{last_but_one}/{last}"),
        [.., last_but_one, last] => format!("…/{last_but_one}/{last}"),
    }
}

/// One recent file: an icon for its kind, its name, and the folder it is in, small and muted. Returns whether it was clicked.
fn recent_row(ui: &mut egui::Ui, t: &Tokens, path: &Path) -> bool {
    let name = path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned());
    let folder = short_folder(path);
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 48.0), Sense::click());
    let press = theme::Press::track(ui, &response);
    press.wash(ui, rect, 8, false);
    let tile = Rect::from_min_size(rect.min + vec2(10.0, 8.0), vec2(32.0, 32.0));
    ui.painter().rect_filled(tile, 8.0, t.hover);
    crate::icons::paint(ui, tile, crate::chrome::file_icon(&name), 16.0, t.icon);
    let clip = ui.painter().with_clip_rect(rect.shrink2(vec2(6.0, 0.0)));
    clip.text(egui::pos2(rect.left() + 54.0, rect.top() + 9.0), egui::Align2::LEFT_TOP, &name, theme::medium(13.0), t.text);
    clip.text(egui::pos2(rect.left() + 54.0, rect.top() + 27.0), egui::Align2::LEFT_TOP, &folder, theme::regular(11.5), t.text_muted);
    crate::icons::paint(
        ui,
        Rect::from_center_size(egui::pos2(rect.right() - 20.0, rect.center().y), vec2(16.0, 16.0)),
        "chevron-right",
        14.0,
        t.text_faint.gamma_multiply(0.4 + 0.6 * press.hover),
    );
    // What a screen reader says: the name and where the file is, so that two files of one name can be told apart.
    let label = format!("{name}, {}", path.display());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
    response.on_hover_text(path.display().to_string()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_is_shown_by_its_last_two_names() {
        let folder = |p: &str| short_folder(Path::new(p));
        assert_eq!(folder("/a/b/c/d/file.xml"), "…/c/d");
        assert_eq!(folder("/a/b/file.xml"), "a/b");
        assert_eq!(folder("/a/file.xml"), "a");
        assert_eq!(folder("/file.xml"), "/");
        assert_eq!(folder("file.xml"), "");
        assert_eq!(folder(""), "");
    }
}
