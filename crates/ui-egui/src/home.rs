//! The view shown when no document is open.

use std::path::{Path, PathBuf};

use egui::{Align, Layout, Sense, Stroke, vec2};

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

/// The welcome card, with the ways to begin: a new document, one from a template, a file. It says what exists today; the list of what
/// arrives is the roadmap's. Below it, the files opened lately. The thing chosen is returned.
pub fn show(ui: &mut egui::Ui, recent: &[PathBuf]) -> Option<Choice> {
    let mut chosen = None;
    let t = Tokens::get(ui.ctx());
    let avail = ui.available_size();
    let card_width = 460.0_f32.min(avail.x - 48.0).max(240.0);
    // At a small window height the card is reached by scrolling, with the whole list.
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.with_layout(Layout::top_down(Align::Center), |ui| {
            // The card is taller with a list under it: the space above is what is left, a third of it.
            let listed = recent.len().min(SHOWN_RECENT) as f32;
            let height = 340.0 + if recent.is_empty() { 0.0 } else { 40.0 + listed * 38.0 };
            ui.add_space(((avail.y - height) / 3.0).max(24.0));
            egui::Frame::NONE
                .fill(t.card)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(egui::CornerRadius::same(12))
                .inner_margin(egui::Margin::symmetric(28, 24))
                .shadow(egui::Shadow { offset: [0, 4], blur: 18, spread: 0, color: t.page_shadow })
                .show(ui, |ui| {
                    ui.set_width(card_width - 56.0);
                    ui.vertical_centered(|ui| {
                        ui.add(egui::Image::new(egui::include_image!("../../../assets/app-icon/xmlcraft.svg")).fit_to_exact_size(vec2(72.0, 72.0)));
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new(crate::APP_NAME).font(theme::semibold(24.0)).color(t.text));
                        ui.label(egui::RichText::new(tl!("An open-source, clean-room XML editor")).font(theme::regular(13.0)).color(t.text_muted));
                        ui.add_space(16.0);
                        ui.label(egui::RichText::new(tl!("Pre-alpha")).font(theme::semibold(11.0)).color(t.accent_text).extra_letter_spacing(0.6));
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(tl!(
                                "Editing, validation against DTDs and XML Schemas, and schema and sample generation are in place. The rest arrives milestone by milestone."
                            ))
                            .font(theme::regular(12.5))
                            .color(t.text_muted),
                        );
                        ui.add_space(14.0);
                        // The buttons wrap when the window is narrow; with room to spare they are centred.
                        ui.horizontal_wrapped(|ui| {
                            ui.add_space(((ui.available_width() - 330.0) / 2.0).max(0.0));
                            for (label, command) in
                                [(tl!("New"), "file.new"), (tl!("New from Template…"), "file.new_from_template"), (tl!("Open…"), "file.open")]
                            {
                                if ui.button(label).clicked() {
                                    chosen = Some(Choice::Command(command));
                                }
                            }
                        });
                    });
                    if !recent.is_empty() {
                        ui.add_space(18.0);
                        // The rows start at the left edge of the card, and so does what they are under.
                        ui.with_layout(Layout::top_down(Align::Min), |ui| {
                            ui.label(egui::RichText::new(tl!("Recent")).font(theme::semibold(11.0)).color(t.text_muted).extra_letter_spacing(0.6));
                            ui.add_space(4.0);
                            for path in recent.iter().take(SHOWN_RECENT) {
                                if recent_row(ui, &t, path) {
                                    chosen = Some(Choice::Open(path.clone()));
                                }
                            }
                        });
                    }
                });
        });
    });
    chosen
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

/// One recent file: its name, and the folder it is in, small and muted. Returns whether it was clicked.
fn recent_row(ui: &mut egui::Ui, t: &Tokens, path: &Path) -> bool {
    let name = path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned());
    let folder = short_folder(path);
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 38.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, egui::CornerRadius::same(6), t.accent.gamma_multiply(0.10));
    }
    let text_left = rect.left() + 10.0;
    let clip = ui.painter().with_clip_rect(rect.shrink2(vec2(6.0, 0.0)));
    clip.text(egui::pos2(text_left, rect.top() + 6.0), egui::Align2::LEFT_TOP, &name, theme::regular(13.0), t.text);
    clip.text(egui::pos2(text_left, rect.top() + 22.0), egui::Align2::LEFT_TOP, &folder, theme::regular(11.0), t.text_muted);
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
