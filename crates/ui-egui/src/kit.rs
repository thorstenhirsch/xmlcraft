//! The design kit: the controls the shell and the dialogs are made of, on the design tokens. Pill buttons (the one that matters most in
//! a view is the primary one), status chips, a segmented control, the dialog frame with its form rows, and cards. A control here
//! always reads the tokens, eases its hover and press through [`theme::Press`] and says what it is to assistive technology.

use egui::{Align, Align2, Color32, CornerRadius, Layout, Rect, Response, Sense, Stroke, StrokeKind, vec2};

use crate::editor_view::SyntaxTokens;
use crate::i18n::tl;
use crate::icons;
use crate::theme::{self, Tokens};
use crate::widgets;

/// The height of a pill button and of a field in a dialog.
pub const PILL_HEIGHT: f32 = 30.0;
/// The one spacing scale: gaps between related controls, between groups, and between sections.
pub const GAP: f32 = 8.0;
pub const GAP_GROUP: f32 = 14.0;
pub const GAP_SECTION: f32 = 22.0;

// --- pill buttons --------------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PillKind {
    /// The action that matters: accent fill.
    Primary,
    /// Any other action: card fill and an outline.
    Secondary,
    /// Quiet: nothing until hovered.
    Ghost,
    /// Throws something away: outlined and lettered in the error colour.
    Danger,
}

/// A pill: an optional icon and a label. `kind` says how much it matters.
pub fn pill(ui: &mut egui::Ui, kind: PillKind, icon: Option<&str>, label: &str) -> Response {
    pill_sized(ui, kind, icon, label, PILL_HEIGHT)
}

pub fn pill_sized(ui: &mut egui::Ui, kind: PillKind, icon: Option<&str>, label: &str, height: f32) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = theme::medium(12.5);
    let enabled = ui.is_enabled();
    let danger = SyntaxTokens::for_kind(t.kind).error;
    let text = match (kind, enabled) {
        (PillKind::Primary, true) => Color32::WHITE,
        (PillKind::Danger, true) => danger,
        (_, true) => t.text,
        (_, false) => t.text_faint,
    };
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, text);
    let icon_w = if icon.is_some() { 22.0 } else { 0.0 };
    let width = galley.size().x + icon_w + 2.0 * 14.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
    let press = theme::Press::track(ui, &resp);
    let radius = CornerRadius::same((height / 2.0) as u8);
    match kind {
        PillKind::Primary => {
            let fill = if enabled {
                press.fill(t.accent, t.accent_text, t.accent.lerp_to_gamma(Color32::BLACK, 0.22))
            } else {
                t.accent.gamma_multiply(0.4)
            };
            ui.painter().rect_filled(rect, radius, fill);
        }
        PillKind::Secondary => {
            let fill = press.fill(t.card, t.hover, t.pressed);
            let edge = t.border.lerp_to_gamma(t.text_muted, press.hover * 0.35);
            ui.painter().rect(rect, radius, fill, Stroke::new(1.0, edge), StrokeKind::Inside);
        }
        PillKind::Ghost => press.wash(ui, rect, radius, false),
        PillKind::Danger => {
            let fill = press.fill(t.card, danger.gamma_multiply(0.10), danger.gamma_multiply(0.18));
            ui.painter().rect(rect, radius, fill, Stroke::new(1.0, danger.gamma_multiply(0.55)), StrokeKind::Inside);
        }
    }
    let body = rect.translate(press.offset());
    let mut x = body.left() + 14.0;
    if let Some(icon) = icon {
        let tint = if kind == PillKind::Primary { Color32::WHITE } else { t.icon };
        icons::paint(
            ui,
            Rect::from_min_size(egui::pos2(x - 1.0, body.center().y - 8.0), vec2(16.0, 16.0)),
            icon,
            15.0,
            if enabled { tint } else { t.text_faint },
        );
        x += icon_w;
    }
    ui.painter().galley(egui::pos2(x, body.center().y - galley.size().y / 2.0), galley, text);
    theme::hand(resp)
}

/// A pill with a second, narrow part for the choices that go with it (Generate ▾). Returns the main part's and the arrow's responses.
pub fn split_pill(ui: &mut egui::Ui, icon: Option<&str>, label: &str, more: &str) -> (Response, Response) {
    let t = Tokens::get(ui.ctx());
    let height = PILL_HEIGHT;
    let font = theme::medium(12.5);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, t.text);
    let icon_w = if icon.is_some() { 22.0 } else { 0.0 };
    let main_w = galley.size().x + icon_w + 2.0 * 14.0 - 2.0;
    let arrow_w = 28.0;
    let (rect, _) = ui.allocate_exact_size(vec2(main_w + arrow_w, height), Sense::hover());
    let main_rect = Rect::from_min_size(rect.min, vec2(main_w, height));
    let arrow_rect = Rect::from_min_size(rect.min + vec2(main_w, 0.0), vec2(arrow_w, height));
    let main = ui.interact(main_rect, ui.next_auto_id().with("split-main"), Sense::click());
    let arrow = ui.interact(arrow_rect, ui.next_auto_id().with("split-more"), Sense::click());
    main.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    arrow.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, more));
    let (pm, pa) = (theme::Press::track(ui, &main), theme::Press::track(ui, &arrow));
    let radius = (height / 2.0) as u8;
    let left = CornerRadius { nw: radius, sw: radius, ne: 0, se: 0 };
    let right = CornerRadius { nw: 0, sw: 0, ne: radius, se: radius };
    ui.painter().rect_filled(main_rect, left, pm.fill(t.card, t.hover, t.pressed));
    ui.painter().rect_filled(arrow_rect, right, pa.fill(t.card, t.hover, t.pressed));
    ui.painter().rect_stroke(rect, CornerRadius::same(radius), Stroke::new(1.0, t.border), StrokeKind::Inside);
    ui.painter().vline(arrow_rect.left(), rect.y_range().shrink(7.0), Stroke::new(1.0, t.border));
    let mut x = main_rect.left() + 14.0;
    if let Some(icon) = icon {
        icons::paint(ui, Rect::from_min_size(egui::pos2(x - 1.0, main_rect.center().y - 8.0), vec2(16.0, 16.0)), icon, 15.0, t.icon);
        x += icon_w;
    }
    ui.painter().galley(egui::pos2(x, main_rect.center().y - galley.size().y / 2.0), galley, t.text);
    icons::paint(ui, Rect::from_center_size(arrow_rect.center(), vec2(14.0, 14.0)), "chevron-down", 13.0, t.icon);
    (theme::hand(main), theme::hand(arrow))
}

// --- chips ---------------------------------------------------------------------------------------------------------------------

/// What a chip says about its subject.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Accent,
    Good,
    Warn,
    Bad,
}

/// The text colour and the fill of a tone.
pub fn tone_colors(t: &Tokens, tone: Tone) -> (Color32, Color32) {
    let syntax = SyntaxTokens::for_kind(t.kind);
    let good = if t.dark() { Color32::from_rgb(0x6C, 0xD0, 0x8A) } else { Color32::from_rgb(0x1C, 0x7C, 0x3A) };
    let tint = |c: Color32| c.gamma_multiply(if t.dark() { 0.22 } else { 0.12 });
    match tone {
        Tone::Neutral => (t.text_muted, t.hover),
        Tone::Accent => (t.accent_text, t.accent_soft),
        Tone::Good => (good, tint(good)),
        Tone::Warn => (syntax.warning, tint(syntax.warning)),
        Tone::Bad => (syntax.error, tint(syntax.error)),
    }
}

/// A small status capsule with an optional icon: what a document is (valid, three problems, offline), at a glance. Clickable when it
/// has something to open.
pub fn chip(ui: &mut egui::Ui, tone: Tone, icon: Option<&str>, label: &str, clickable: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let (fg, bg) = tone_colors(&t, tone);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), theme::medium(11.5), fg);
    let icon_w = if icon.is_some() { 18.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + icon_w + 20.0, 24.0), if clickable { Sense::click() } else { Sense::hover() });
    resp.widget_info(|| egui::WidgetInfo::labeled(if clickable { egui::Role::Button } else { egui::Role::Label }, true, label));
    let press = theme::Press::track(ui, &resp);
    let fill = if clickable { bg.lerp_to_gamma(fg.gamma_multiply(0.35), press.hover * 0.5 + press.down * 0.3) } else { bg };
    ui.painter().rect_filled(rect, CornerRadius::same(12), fill);
    let mut x = rect.left() + 10.0;
    if let Some(icon) = icon {
        icons::paint(ui, Rect::from_min_size(egui::pos2(x - 1.0, rect.center().y - 7.0), vec2(14.0, 14.0)), icon, 13.0, fg);
        x += icon_w;
    }
    ui.painter().galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, fg);
    if clickable { theme::hand(resp) } else { resp }
}

/// A chip that is a switch: filled with the accent when `selected`, outlined otherwise. For filters.
pub fn toggle_chip(ui: &mut egui::Ui, selected: bool, label: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = theme::medium(11.5);
    let color = if selected { Color32::WHITE } else { t.text_muted };
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, color);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 22.0, 24.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, label));
    let press = theme::Press::track(ui, &resp);
    if selected {
        ui.painter().rect_filled(rect, CornerRadius::same(12), t.accent.lerp_to_gamma(t.accent_text, press.hover * 0.5));
    } else {
        ui.painter().rect(
            rect,
            CornerRadius::same(12),
            press.fill(Color32::TRANSPARENT, t.hover, t.pressed),
            Stroke::new(1.0, t.border),
            StrokeKind::Inside,
        );
    }
    ui.painter().galley(rect.center() - galley.size() / 2.0 + press.offset(), galley, color);
    theme::hand(resp)
}

// --- a segmented control -------------------------------------------------------------------------------------------------------

/// Exclusive choices side by side: the chosen one is filled. Each segment is a labelled button. True when the choice changed.
pub fn segmented<T: Copy + PartialEq>(ui: &mut egui::Ui, current: &mut T, options: &[(T, &str)]) -> bool {
    let t = Tokens::get(ui.ctx());
    let font = theme::medium(12.5);
    let widths: Vec<f32> = options.iter().map(|(_, label)| widgets::text_width(ui, label, &font) + 28.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + 6.0;
    let (track, _) = ui.allocate_exact_size(vec2(total, PILL_HEIGHT + 2.0), Sense::hover());
    ui.painter().rect(track, CornerRadius::same(((PILL_HEIGHT + 2.0) / 2.0) as u8), t.field, Stroke::new(1.0, t.border), StrokeKind::Inside);
    let mut changed = false;
    let mut x = track.left() + 3.0;
    for ((value, label), width) in options.iter().zip(widths) {
        let rect = Rect::from_min_size(egui::pos2(x, track.top() + 3.0), vec2(width, PILL_HEIGHT - 4.0));
        x += width;
        let resp = ui.interact(rect, ui.next_auto_id().with(("segment", *label)), Sense::click());
        let selected = *current == *value;
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, selected, *label));
        let press = theme::Press::track(ui, &resp);
        let radius = CornerRadius::same(((PILL_HEIGHT - 4.0) / 2.0) as u8);
        if selected {
            ui.painter().rect_filled(rect, radius, t.accent);
        } else {
            press.wash(ui, rect, radius, false);
        }
        let color = if selected { Color32::WHITE } else { t.text };
        ui.painter().text(rect.translate(press.offset()).center(), Align2::CENTER_CENTER, label, font.clone(), color);
        if theme::hand(resp).clicked() && !selected {
            *current = *value;
            changed = true;
        }
    }
    changed
}

// --- cards ---------------------------------------------------------------------------------------------------------------------

/// A surface that groups related controls: the card fill, an outline, 10-point corners. `selected` draws the accent outline.
pub fn card<R>(ui: &mut egui::Ui, selected: bool, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let t = Tokens::get(ui.ctx());
    let stroke = if selected { Stroke::new(1.5, t.accent) } else { Stroke::new(1.0, t.border) };
    egui::Frame::NONE
        .fill(t.card)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

// --- dialogs -------------------------------------------------------------------------------------------------------------------

/// What a dialog's footer was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Footer {
    Nothing,
    Cancel,
    Primary,
}

/// The frame every dialog shares: a header (a tinted icon, the title, a line saying what the dialog does, and a close button), a body
/// the caller fills, and the footer the caller asks for with [`footer`]. Returns what `body` returned and whether the dialog asked to close
/// (Escape, a click outside, the close button).
pub fn dialog<R>(
    ctx: &egui::Context,
    id: &str,
    width: f32,
    icon: &str,
    title: &str,
    subtitle: &str,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> (R, bool) {
    let t = Tokens::get(ctx);
    let frame = egui::Frame::new()
        .fill(t.card)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin { left: 24, right: 24, top: 20, bottom: 18 })
        .shadow(ctx.global_style().visuals.window_shadow);
    let response = egui::Modal::new(egui::Id::unique(id)).frame(frame).show(ctx, |ui| {
        ui.set_width(width);
        begin_dialog(ui);
        let mut close = false;
        ui.horizontal(|ui| {
            let (badge, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
            ui.painter().rect_filled(badge, CornerRadius::same(10), t.accent_soft);
            icons::paint(ui, Rect::from_center_size(badge.center(), vec2(20.0, 20.0)), icon, 18.0, t.accent_text);
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(egui::RichText::new(title).font(theme::semibold(17.0)).color(t.text));
                if !subtitle.is_empty() {
                    ui.add(egui::Label::new(egui::RichText::new(subtitle).font(theme::regular(12.5)).color(t.text_muted)).wrap());
                }
            });
            ui.with_layout(Layout::right_to_left(Align::TOP), |ui| {
                if icons::button(ui, "x", 26.0, false, tl!("Close")).clicked() {
                    close = true;
                }
            });
        });
        ui.add_space(GAP_GROUP);
        ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, t.divider));
        ui.add_space(GAP_GROUP);
        (body(ui), close)
    });
    let escaped = response.should_close();
    let (inner, close) = response.inner;
    (inner, close || escaped)
}

/// Outlined controls at a shared height, with a focus stroke and 4-point corners. Dialog fills match the theme's field colour, so radio
/// buttons, check boxes, combo boxes and number fields need the outline or they vanish into the card.
fn begin_dialog(ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let w = &mut ui.visuals_mut().widgets;
    w.inactive.bg_stroke = Stroke::new(1.0, t.border);
    w.inactive.weak_bg_fill = t.field;
    // Slider rails and check-box interiors use the plain fill.
    w.inactive.bg_fill = t.hover;
    w.hovered.bg_stroke = Stroke::new(1.0, t.text_muted);
    for state in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.corner_radius = CornerRadius::same(6);
    }
    ui.visuals_mut().selection.stroke = Stroke::new(1.5, t.accent);
    ui.spacing_mut().interact_size.y = PILL_HEIGHT;
    ui.spacing_mut().item_spacing = vec2(GAP, GAP - 2.0);
}

/// A form section: a small caps title and the rows under it, with the space above that separates sections.
pub fn section<R>(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let t = Tokens::get(ui.ctx());
    ui.add_space(GAP_GROUP);
    ui.label(egui::RichText::new(title.to_uppercase()).font(theme::semibold(10.5)).color(t.text_faint).extra_letter_spacing(0.6));
    ui.add_space(6.0);
    add(ui)
}

/// The width of the label column of a form.
pub const LABEL_WIDTH: f32 = 148.0;

/// One form row: the label at the left in a column of its own, the control after it, and a help line under the control. Everything
/// aligns on the label column, whatever the control is.
pub fn row<R>(ui: &mut egui::Ui, label: &str, help: &str, control: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let t = Tokens::get(ui.ctx());
    let out = ui
        .horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = GAP_GROUP;
            let (cell, _) = ui.allocate_exact_size(vec2(LABEL_WIDTH, PILL_HEIGHT), Sense::hover());
            ui.painter().text(cell.left_center(), Align2::LEFT_CENTER, label, theme::medium(12.5), t.text);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                let out = control(ui);
                if !help.is_empty() {
                    ui.add(egui::Label::new(egui::RichText::new(help).font(theme::regular(11.5)).color(t.text_faint)).wrap());
                }
                out
            })
            .inner
        })
        .inner;
    ui.add_space(6.0);
    out
}

/// A line of help under a control, or an explanation under a section title.
pub fn help(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add(egui::Label::new(egui::RichText::new(text).font(theme::regular(11.5)).color(t.text_faint)).wrap());
}

/// A problem found in the dialog's input: an outlined strip with an icon, under the form, above the footer.
pub fn banner(ui: &mut egui::Ui, tone: Tone, text: &str) {
    let t = Tokens::get(ui.ctx());
    let (fg, bg) = tone_colors(&t, tone);
    egui::Frame::NONE.fill(bg).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            let icon = match tone {
                Tone::Bad => "circle-x",
                Tone::Warn => "triangle-alert",
                Tone::Good => "circle-check",
                _ => "info",
            };
            icons::paint(ui, Rect::from_min_size(ui.cursor().min + vec2(0.0, 1.0), vec2(16.0, 16.0)), icon, 15.0, fg);
            ui.add_space(22.0);
            ui.add(egui::Label::new(egui::RichText::new(text).font(theme::regular(12.5)).color(t.text)).wrap());
        });
    });
}

/// The footer of a dialog: the secondary action, then the primary at the far right, Cancel to its left. `primary_enabled` false greys the
/// primary and `why_not` (when not empty) says why on hover. Escape takes Cancel (the dialog frame does); a dialog with a field answers Enter itself.
pub fn footer(ui: &mut egui::Ui, cancel: &str, primary: &str, primary_icon: Option<&str>, primary_enabled: bool, why_not: &str) -> Footer {
    let t = Tokens::get(ui.ctx());
    ui.add_space(GAP_SECTION);
    ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, t.divider));
    ui.add_space(GAP_GROUP);
    let mut action = Footer::Nothing;
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        let ok = ui.add_enabled_ui(primary_enabled, |ui| pill(ui, PillKind::Primary, primary_icon, primary)).inner;
        let ok = if !primary_enabled && !why_not.is_empty() { ok.on_disabled_hover_text(why_not) } else { ok };
        if ok.clicked() {
            action = Footer::Primary;
        }
        if pill(ui, PillKind::Secondary, None, cancel).clicked() {
            action = Footer::Cancel;
        }
    });
    action
}

/// A footer with one action, at the far right: for a dialog that only shows something. True when it was clicked.
pub fn close_footer(ui: &mut egui::Ui, label: &str) -> bool {
    let t = Tokens::get(ui.ctx());
    ui.add_space(GAP_SECTION);
    ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, t.divider));
    ui.add_space(GAP_GROUP);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| pill(ui, PillKind::Primary, None, label).clicked()).inner
}

/// A small dialog that asks something: a tinted icon, the question, a line under it, what the caller puts in `body` (a field, say), and the
/// answers, which the caller draws with [`actions`]. Returns what `body` returned and whether Escape or a click outside asked to close.
#[allow(clippy::too_many_arguments)]
pub fn question<R>(
    ctx: &egui::Context,
    id: &str,
    width: f32,
    icon: &str,
    tone: Tone,
    title: &str,
    text: &str,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> (R, bool) {
    let t = Tokens::get(ctx);
    let (fg, bg) = tone_colors(&t, tone);
    let frame = egui::Frame::new()
        .fill(t.card)
        .stroke(Stroke::new(1.0, t.border))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(egui::Margin { left: 22, right: 22, top: 20, bottom: 18 })
        .shadow(ctx.global_style().visuals.window_shadow);
    let response = egui::Modal::new(egui::Id::unique(id)).frame(frame).show(ctx, |ui| {
        ui.set_width(width);
        begin_dialog(ui);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 14.0;
            let (badge, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
            ui.painter().rect_filled(badge, CornerRadius::same(18), bg);
            icons::paint(ui, Rect::from_center_size(badge.center(), vec2(20.0, 20.0)), icon, 18.0, fg);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.add(egui::Label::new(egui::RichText::new(title).font(theme::semibold(15.0)).color(t.text)).wrap());
                if !text.is_empty() {
                    ui.add(egui::Label::new(egui::RichText::new(text).font(theme::regular(12.5)).color(t.text_muted)).wrap());
                }
            });
        });
        body(ui)
    });
    let escaped = response.should_close();
    (response.inner, escaped)
}

/// The answers to a question, the first being the default: drawn from the right edge, so the default is at the far right with the others
/// to its left. Returns the one clicked.
pub fn actions(ui: &mut egui::Ui, answers: &[(&str, PillKind)]) -> Option<usize> {
    ui.add_space(GAP_SECTION);
    let mut clicked = None;
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        for (index, (label, kind)) in answers.iter().enumerate() {
            if pill(ui, *kind, None, label).clicked() {
                clicked = Some(index);
            }
        }
    });
    clicked
}

/// A small square toggle with a short mark ("Aa", "W", ".*"): the search options. Filled with the accent tint while on.
pub fn mark_toggle(ui: &mut egui::Ui, on: &mut bool, mark: &str, name: &str) -> Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 26.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, *on, name));
    let press = theme::Press::track(ui, &resp);
    press.wash(ui, rect, 6, *on);
    if *on {
        ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, t.accent.gamma_multiply(0.6)), StrokeKind::Inside);
    }
    ui.painter().text(
        rect.translate(press.offset()).center(),
        Align2::CENTER_CENTER,
        mark,
        theme::mono(12.0),
        if *on { t.accent_text } else { t.text_muted },
    );
    let resp = theme::hand(resp).on_hover_text(name);
    if resp.clicked() {
        *on = !*on;
    }
    resp
}
