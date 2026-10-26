//! The view shown when no document is open.

use egui::{Align, Layout, Stroke, vec2};

use crate::i18n::tl;
use crate::theme::{self, Tokens};

/// The pre-alpha welcome card. It says what exists today; the list of what arrives is the roadmap's.
pub fn show(ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let avail = ui.available_size();
    let card_width = 460.0_f32.min(avail.x - 48.0).max(240.0);
    ui.with_layout(Layout::top_down(Align::Center), |ui| {
        ui.add_space(((avail.y - 340.0) / 3.0).max(24.0));
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
                            "The window and its panes are in place. Editing, schemas and validation arrive milestone by milestone."
                        ))
                        .font(theme::regular(12.5))
                        .color(t.text_muted),
                    );
                });
            });
    });
}
