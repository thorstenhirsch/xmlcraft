//! Small custom widgets built on the design tokens (the same set PdfCraft uses, so the two apps feel alike). Dialog helpers
//! (`modal`, `begin_dialog`, pill buttons, `radio_value`) are brought over from PdfCraft with the first dialog.

use egui::{Align2, Color32, CornerRadius, Rect, Response, Sense, Stroke, vec2};

use crate::icons;
use crate::theme::{self, Tokens};

pub(crate) fn text_width(ui: &egui::Ui, text: &str, font: &egui::FontId) -> f32 {
    ui.fonts_mut(|f| f.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE).size().x)
}

/// A mode-bar tab: text with an underline when active.
pub fn mode_tab(ui: &mut egui::Ui, label: &str, active: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = if active { theme::semibold(13.5) } else { theme::medium(13.5) };
    let w = text_width(ui, label, &font);
    // The row's own height, not a fixed 48: the bar's frame stroke takes 2 pt of it, and a taller tab would have its underline
    // clipped away at the bottom edge.
    let height = ui.available_height().clamp(24.0, 48.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 22.0, height), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), active, label));
    let press = theme::Press::track(ui, &resp);
    let well = rect.shrink2(vec2(2.0, 9.0));
    if active {
        if press.down > 0.01 {
            ui.painter().rect_filled(well, CornerRadius::same(6), t.pressed.gamma_multiply(press.down));
        }
    } else {
        press.wash(ui, well, 6, false);
    }
    let body = rect.translate(press.offset());
    ui.painter().text(body.center(), Align2::CENTER_CENTER, label, font, if active { t.text } else { t.text_muted });
    if active {
        let r = Rect::from_min_max(rect.left_bottom() + vec2(10.0, -2.0), rect.right_bottom() - vec2(10.0, 0.0));
        ui.painter().rect_filled(r, CornerRadius { nw: 1, ne: 1, sw: 0, se: 0 }, t.accent);
    }
    theme::hand(resp)
}

/// Icon + label, transparent until hovered.
pub fn ghost_button(ui: &mut egui::Ui, icon: &str, label: &str) -> Response {
    ghost_button_sized(ui, icon, label, 30.0)
}

/// [`ghost_button`] at a given height (the macOS title bar is shorter than the tab strip elsewhere).
pub fn ghost_button_sized(ui: &mut egui::Ui, icon: &str, label: &str, height: f32) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = theme::medium(13.0);
    let w = text_width(ui, label, &font);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 38.0, height), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    let press = theme::Press::track(ui, &resp);
    press.wash(ui, rect, 6, false);
    let body = rect.translate(press.offset());
    icons::paint(ui, Rect::from_min_size(body.min + vec2(6.0, (height - 18.0) / 2.0), vec2(18.0, 18.0)), icon, 17.0, t.icon);
    ui.painter().text(body.left_center() + vec2(30.0, 0.0), Align2::LEFT_CENTER, label, font, t.text);
    theme::hand(resp)
}

/// Give a widget the accessible name `name`. A text field shows a hint, which assistive technology reads as a placeholder, not as the
/// field's name; this is what a screen reader (and a test) finds the field by.
pub fn name_for_assistive_technology(ui: &egui::Ui, response: &Response, name: &str) {
    ui.ctx().accesskit_node_builder(response.id, |node| node.set_label(name));
}

/// A search-field lookalike that opens the command palette. `shortcut` is shown at its right edge.
pub fn search_box(ui: &mut egui::Ui, placeholder: &str, shortcut: &str, width: f32) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), placeholder));
    let press = theme::Press::track(ui, &resp);
    let fill = press.fill(t.field, t.hover, t.pressed);
    let edge = t.border.lerp_to_gamma(t.text_muted, press.hover * 0.35);
    ui.painter().rect(rect, CornerRadius::same(16), fill, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
    icons::paint(ui, Rect::from_min_size(rect.min + vec2(10.0, 8.0), vec2(16.0, 16.0)), "search", 15.0, t.text_muted);
    let shortcut = ui.painter().layout_no_wrap(shortcut.to_owned(), theme::regular(11.5), t.text_faint);
    let shortcut_pos = rect.right_center() - vec2(12.0 + shortcut.size().x, shortcut.size().y * 0.5);
    // Reserve the measured shortcut width; a long placeholder is elided within the remaining space rather than painted
    // underneath the shortcut.
    let text_right = (shortcut_pos.x - 10.0).max(rect.left());
    let text_left = (rect.left() + 34.0).min(text_right);
    let text_rect = Rect::from_min_max(egui::pos2(text_left, rect.top()), egui::pos2(text_right, rect.bottom()));
    let mut job = egui::text::LayoutJob::simple_singleline(placeholder.to_owned(), theme::regular(13.0), t.text_faint);
    job.wrap = egui::text::TextWrapping { max_width: text_rect.width(), max_rows: 1, break_anywhere: true, ..Default::default() };
    let text = ui.fonts_mut(|f| f.layout_job(job));
    let elided = text.elided;
    ui.painter().with_clip_rect(text_rect).galley(egui::pos2(text_left, rect.center().y - text.size().y * 0.5), text, t.text_faint);
    ui.painter().with_clip_rect(rect).galley(shortcut_pos, shortcut, t.text_faint);
    let resp = resp.on_hover_cursor(egui::CursorIcon::Text);
    if elided { resp.on_hover_text(placeholder) } else { resp }
}

/// Corner radius of a checkbox's box. With the widgets' radius (`Tokens::radius`, 6) egui's 14 px box is a circle, the shape of a
/// radio button.
const CHECKBOX_RADIUS: u8 = 3;

/// A checkbox whose box is an outlined rounded square, never mistaken for a radio button. Every checkbox goes through here:
/// clippy.toml disallows `ui.checkbox` and `egui::Checkbox`.
pub fn checkbox<'a>(checked: &'a mut bool, atoms: impl egui::IntoAtoms<'a>) -> impl egui::Widget + 'a {
    #[expect(clippy::disallowed_methods, reason = "the one place that styles the box")]
    let widget = egui::Checkbox::new(checked, atoms);
    toggle(widget, Some(CHECKBOX_RADIUS))
}

/// A radio button with an outline and a fill, like [`checkbox`]. Every radio button goes through here or [`radio_value`]:
/// clippy.toml disallows `ui.radio`, `ui.radio_value` and `egui::RadioButton`.
pub fn radio<'a>(selected: bool, atoms: impl egui::IntoAtoms<'a>) -> impl egui::Widget + 'a {
    #[expect(clippy::disallowed_methods, reason = "the one place that styles the circle")]
    let widget = egui::RadioButton::new(selected, atoms);
    toggle(widget, None)
}

/// The box of a checkbox or the circle of a radio button. egui draws it with the widgets' visuals and has no style of its own for
/// it, so it is set in a scope around the widget, keeping what the caller set:
/// - an outline, darker on hover;
/// - on the light theme, the plain fill (`Tokens::hover`), as dialogs have: the field colour is the panels' white, so the box
///   would vanish;
/// - for a checkbox, a corner radius small enough to keep the box square.
fn toggle<'a>(widget: impl egui::Widget + 'a, radius: Option<u8>) -> impl egui::Widget + 'a {
    move |ui: &mut egui::Ui| {
        ui.scope(|ui| {
            let t = Tokens::get(ui.ctx());
            let w = &mut ui.visuals_mut().widgets;
            if let Some(radius) = radius {
                for v in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
                    v.corner_radius = CornerRadius::same(radius);
                }
            }
            if !t.dark() && w.inactive.bg_fill == t.field {
                w.inactive.bg_fill = t.hover;
            }
            if w.inactive.bg_stroke.is_empty() {
                w.inactive.bg_stroke = Stroke::new(1.0, t.border);
            }
            if w.hovered.bg_stroke.is_empty() {
                w.hovered.bg_stroke = Stroke::new(1.0, t.text_muted);
            }
            ui.add(widget)
        })
        .inner
    }
}

/// A small upper-case section heading inside a pane.
pub fn section_title(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(10.0);
    ui.label(egui::RichText::new(text.to_uppercase()).font(theme::semibold(10.5)).color(t.text_faint).extra_letter_spacing(0.6));
    ui.add_space(2.0);
}

/// Centred, muted text for an empty pane: a title and a hint below it, `top` points below the top edge.
pub fn empty_state(ui: &mut egui::Ui, top: f32, title: &str, hint: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(top);
    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(16, 0)).show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.label(egui::RichText::new(title).font(theme::medium(13.0)).color(t.text_muted));
            ui.add_space(2.0);
            ui.label(egui::RichText::new(hint).font(theme::regular(12.0)).color(t.text_faint));
        });
    });
}
