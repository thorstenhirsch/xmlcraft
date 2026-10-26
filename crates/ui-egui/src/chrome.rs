//! Window chrome: tab strip (with the integrated macOS title bar), mode bar, messages panel and status bar. The layout and the
//! measurements are PdfCraft's, so the two apps read as one family.

use egui::{Align, Align2, Color32, CornerRadius, Layout, Rect, Sense, Stroke, vec2};

use crate::i18n::tl;
use crate::theme::{self, ThemePreference, Tokens};
use crate::{Mode, XmlCraftApp, icons, widgets};

/// Height of the tab strip when it doubles as the macOS title bar. AppKit draws the traffic lights itself, centred in the standard
/// 28 pt title-bar band at the top of the window, and moving them takes AppKit calls the workspace forbids (`unsafe`). So on macOS
/// the strip is that band: the lights sit vertically centred in it, and the controls beside them shrink to fit.
pub const MAC_TITLE_BAR: f32 = 28.0;
/// The traffic lights as AppKit lays them out: button diameter, centre-to-centre pitch and the first button's left edge (8–10 pt
/// depending on the macOS version; the larger value keeps the gap after them from shrinking).
const MAC_LIGHT: f32 = 12.0;
const MAC_LIGHT_PITCH: f32 = 20.0;
const MAC_LIGHTS_LEFT: f32 = 10.0;
/// Where the strip's content starts to the right of the traffic lights: the end of the three lights plus the same gap the lights
/// keep from the top of the window.
pub const MAC_CONTENT_LEFT: f32 = MAC_LIGHTS_LEFT + 2.0 * MAC_LIGHT_PITCH + MAC_LIGHT + (MAC_TITLE_BAR - MAC_LIGHT) / 2.0;
/// The strip and its controls everywhere else.
const TAB_STRIP: f32 = 38.0;
const TAB_HEIGHT: f32 = 30.0;
const STRIP_BUTTON: f32 = 28.0;
/// The mode bar, the status bar and the messages panel's header.
const MODE_BAR: f32 = 48.0;
const STATUS_BAR: f32 = 26.0;
/// A tab's chrome around its name: the icon on the left, the close button on the right.
const TAB_CHROME: f32 = 64.0;

pub fn tab_strip(app: &mut XmlCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    // macOS: the strip is the 28 pt title bar, so the traffic lights are centred in it; tabs and buttons shrink to keep a margin
    // above and below.
    let mac = cfg!(target_os = "macos") && app.integrated_titlebar;
    let left = if mac { MAC_CONTENT_LEFT as i8 } else { 8 };
    let (tab_h, button) = if mac { (MAC_TITLE_BAR - 2.0, 24.0) } else { (TAB_HEIGHT, STRIP_BUTTON) };
    egui::Panel::top("tab_strip")
        .exact_size(if mac { MAC_TITLE_BAR } else { TAB_STRIP })
        .frame(egui::Frame::NONE.fill(t.titlebar).inner_margin(egui::Margin { left, right: 10, top: 0, bottom: 0 }))
        .show(ui, |ui| {
            // The strip is the window's drag handle where the system title bar is hidden.
            let drag = ui.interact(ui.max_rect(), ui.id().with("titledrag"), Sense::click_and_drag());
            if mac && drag.drag_started() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if mac && drag.double_clicked() {
                let max = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
            }
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if icons::button(ui, "house", button, app.active.is_none(), tl!("Home")).clicked() {
                    app.active = None;
                }
                let mut close = None;
                for (i, tab_info) in app.tabs.iter().enumerate() {
                    let response = tab(ui, &t, &tab_info.name, tab_info.dirty, app.active == Some(i), &mut close, i, tab_h);
                    if response.clicked() {
                        app.active = Some(i);
                    }
                    // A middle click closes the tab, as in browsers.
                    if response.middle_clicked() {
                        close = Some(i);
                    }
                }
                if let Some(i) = close {
                    app.close_tab(i);
                }
                ui.add_space(4.0);
                // Opening files arrives with the editor (M1.12): the button is here so the strip looks like it will, and says why
                // it cannot be used yet.
                ui.add_enabled_ui(false, |ui| widgets::ghost_button_sized(ui, "plus", tl!("Open"), tab_h))
                    .response
                    .on_disabled_hover_text(tl!("Opening files arrives with the editor milestone"));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (icon, label) = match app.theme_preference {
                        ThemePreference::System => ("monitor", tl!("Use system setting")),
                        ThemePreference::Light => ("sun", tl!("Light")),
                        ThemePreference::Dark => ("moon", tl!("Dark")),
                    };
                    let tip = format!("{}: {label}", tl!("Display theme"));
                    // A click cycles System, Light, Dark.
                    if icons::button(ui, icon, button, false, &tip).clicked() {
                        app.theme_preference = app.theme_preference.next();
                    }
                });
            });
        });
}

/// A tab name, at most 28 characters (longer names end in "…").
pub fn tab_name(name: &str) -> String {
    if name.chars().count() > 28 { format!("{}…", name.chars().take(27).collect::<String>()) } else { name.to_string() }
}

/// A document tab: the file icon on the left, the name, and a close button (or a dot while there are unsaved changes) on the right.
#[allow(clippy::too_many_arguments)]
fn tab(ui: &mut egui::Ui, t: &Tokens, name: &str, dirty: bool, active: bool, close: &mut Option<usize>, index: usize, height: f32) -> egui::Response {
    let font = theme::regular(13.0);
    let label = tab_name(name);
    let text_w = widgets::text_width(ui, &label, &font);
    // Full strip height so the active tab sits on the mode bar instead of floating in the title bar.
    let height = ui.available_height().max(height);
    let (rect, _) = ui.allocate_exact_size(vec2(text_w + TAB_CHROME, height), Sense::hover());
    let id = ui.id().with(("document-tab", index));
    let a11y = if dirty { format!("{name} (edited)") } else { name.to_string() };
    let resp =
        craft_ui::tabs::Tab::new(id, &a11y, active).sense(Sense::click()).focus_stroke(Stroke::new(1.0, t.accent)).show_at(ui, rect, |ui, resp| {
            let press = theme::Press::track(ui, resp);
            let bg = if active {
                press.fill(t.chrome, t.chrome.lerp_to_gamma(t.hover, 0.65), t.hover)
            } else {
                press.fill(Color32::TRANSPARENT, t.hover, t.pressed)
            };
            if bg.a() > 0 {
                ui.painter().rect_filled(rect, CornerRadius { nw: 7, ne: 7, sw: 0, se: 0 }, bg);
            }
            icons::paint(
                ui,
                Rect::from_min_size(rect.min + vec2(6.0, (height - 16.0) / 2.0), vec2(16.0, 16.0)),
                "file-code",
                15.0,
                if active { t.accent } else { t.text_muted },
            );
            ui.painter().text(
                rect.min + vec2(28.0, rect.height() / 2.0),
                Align2::LEFT_CENTER,
                label,
                font,
                if active { t.text } else { t.text_muted },
            );
        });
    let x_rect = Rect::from_center_size(rect.right_center() - vec2(16.0, 0.0), vec2(20.0, 20.0));
    let x = ui.interact(x_rect, id.with("close"), Sense::click());
    let x_press = theme::Press::track(ui, &x);
    x_press.wash(ui, x_rect, 4, false);
    // Unsaved changes: a dot where the close button sits, until the tab is hovered.
    if dirty && !resp.hovered() && !x.hovered() {
        ui.painter().circle_filled(x_rect.center(), 4.0, if active { t.text } else { t.text_muted });
    } else if active || resp.hovered() || x.hovered() {
        icons::paint(ui, x_rect.translate(x_press.offset()), "x", 13.0, t.text_muted);
    }
    if theme::hand(x).clicked() {
        *close = Some(index);
    }
    let tip = if dirty { crate::i18n::fmt(tl!("{name} — unsaved changes"), &[("name", name)]) } else { name.to_owned() };
    theme::hand(resp).on_hover_text(tip)
}

pub fn mode_bar(app: &mut XmlCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::top("mode_bar")
        .exact_size(MODE_BAR)
        .frame(egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin::symmetric(10, 0)).stroke(Stroke::new(1.0, t.divider)))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                main_menu(app, ui);
                ui.add_space(6.0);
                ui.painter().vline(ui.cursor().left(), ui.max_rect().y_range().shrink(12.0), Stroke::new(1.0, t.divider));
                ui.add_space(10.0);
                // The editing views of the active document. Only the text view exists in the first release; Grid, Schema and
                // Browser views appear here only when they ship.
                for (mode, label) in [(Mode::Text, tl!("Text"))] {
                    if widgets::mode_tab(ui, label, app.mode == mode).clicked() {
                        app.mode = mode;
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.add_space(8.0);
                    if widgets::search_box(ui, tl!("Find tools and commands"), &crate::primary_shortcut("K"), 260.0).clicked() {
                        app.palette_open = true;
                    }
                });
            });
        });
}

fn main_menu(app: &mut XmlCraftApp, ui: &mut egui::Ui) {
    let resp = widgets::ghost_button(ui, "panel-left", tl!("Menu"));
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(230.0);
        ui.menu_button(tl!("View"), |ui| {
            if ui.add(widgets::checkbox(&mut app.left_open, tl!("Outline"))).clicked() {
                ui.close();
            }
            let mut messages = app.bottom_open;
            if ui.add(widgets::checkbox(&mut messages, tl!("Messages"))).clicked() {
                app.bottom_open = messages;
                ui.close();
            }
            let mut helpers = app.right.is_some();
            if ui.add(widgets::checkbox(&mut helpers, tl!("Entry Helpers"))).clicked() {
                app.right = helpers.then_some(crate::side_panels::RightPanel::EntryHelpers);
                ui.close();
            }
            ui.separator();
            ui.menu_button(tl!("Display theme"), |ui| {
                for (preference, label) in [
                    (ThemePreference::System, tl!("Use system setting")),
                    (ThemePreference::Light, tl!("Light")),
                    (ThemePreference::Dark, tl!("Dark")),
                ] {
                    if ui.add(widgets::radio(app.theme_preference == preference, label)).clicked() {
                        app.theme_preference = preference;
                        ui.close();
                    }
                }
            });
        });
        ui.separator();
        if ui.button(tl!("Quit")).clicked() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            ui.close();
        }
    });
}

pub fn status_bar(app: &mut XmlCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    egui::Panel::bottom("status_bar")
        .exact_size(STATUS_BAR)
        .frame(egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin::symmetric(12, 0)).stroke(Stroke::new(1.0, t.divider)))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                ui.label(egui::RichText::new(tl!("Ready")).font(theme::regular(11.5)).color(t.text_muted));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(egui::RichText::new(format!("{} {}", crate::APP_NAME, crate::VERSION)).font(theme::regular(11.5)).color(t.text_faint));
                    if !app.bottom_open && widgets::ghost_button_sized(ui, "panel-bottom", tl!("Messages"), 22.0).clicked() {
                        app.bottom_open = true;
                    }
                });
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mac_title_bar_centres_and_clears_the_traffic_lights() {
        // The lights are centred in the bar, and content keeps the same gap after them.
        let margin = (MAC_TITLE_BAR - MAC_LIGHT) / 2.0;
        let lights_end = MAC_LIGHTS_LEFT + 2.0 * MAC_LIGHT_PITCH + MAC_LIGHT;
        assert_eq!(MAC_CONTENT_LEFT - lights_end, margin);
        // `egui::Margin` is in `i8`.
        assert!(MAC_CONTENT_LEFT <= f32::from(i8::MAX));
    }

    #[test]
    fn long_tab_names_are_cut_at_a_char_boundary() {
        assert_eq!(tab_name("a.xml"), "a.xml");
        let long = "é".repeat(40);
        let cut = tab_name(&long);
        assert_eq!(cut.chars().count(), 28);
        assert!(cut.ends_with('…'));
        assert_eq!(tab_name(&"x".repeat(28)), "x".repeat(28));
        assert_eq!(tab_name(""), "");
    }
}
