//! The ⌘K command palette: every registered command with its shortcut, found by a forgiving match.

use egui::{Align2, CornerRadius, Rect, Sense, vec2};
use xmlcraft_engine::commands::{COMMANDS, CommandSpec};

use crate::i18n::tl;
use crate::theme::{self, Tokens};
use crate::{XmlCraftApp, icons, widgets};

/// How many results the palette lists.
const MAX_HITS: usize = 12;

/// Where `needle` (already lower-cased) matches `hay`, for ranking: the position of a substring match (earlier is better), 100 for
/// a subsequence match (every letter in order), `None` when it does not match. An empty needle matches everything equally.
pub(crate) fn score(hay: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let hay = hay.to_lowercase();
    if let Some(byte) = hay.find(needle) {
        // The position in characters, so a long multi-byte prefix does not rank like a long ASCII one.
        return Some(hay[..byte].chars().count());
    }
    let mut letters = hay.chars();
    needle.chars().all(|c| letters.any(|h| h == c)).then_some(100)
}

struct Hit {
    spec: &'static CommandSpec,
    detail: String,
    ready: bool,
}

/// The commands that match `query`, best first. Matching considers the label, then the id (ranked lower).
fn hits(app: &XmlCraftApp, ctx: &egui::Context, query: &str) -> Vec<Hit> {
    let needle = query.trim().to_lowercase();
    let mut found: Vec<(usize, Hit)> = Vec::new();
    for spec in COMMANDS {
        let label = XmlCraftApp::command_label(spec);
        if let Some(s) = score(label, &needle).or_else(|| score(spec.id, &needle).map(|s| s + 50)) {
            let detail =
                spec.shortcut.map(|k| crate::commands::shortcut_label(ctx, k)).unwrap_or_else(|| tl!(spec.menu.unwrap_or("Command")).to_owned());
            found.push((s, Hit { spec, detail, ready: app.command_enabled(spec) }));
        }
    }
    // Stable: ties keep the registry's menu order; commands that cannot run now go after those that can.
    found.sort_by_key(|(s, h)| (*s, !h.ready));
    found.into_iter().take(MAX_HITS).map(|(_, h)| h).collect()
}

pub fn show(app: &mut XmlCraftApp, ctx: &egui::Context) {
    if !app.palette_open {
        return;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.palette_open = false;
        return;
    }
    let t = Tokens::get(ctx);
    let found = hits(app, ctx, &app.palette_query);
    let (down, up) = ctx.input(|i| (i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::ArrowUp)));
    // The selection is kept per query: typing starts again at the first hit.
    let last = found.len().saturating_sub(1);
    if app.palette_query != app.palette_last_query {
        app.palette_selected = 0;
        app.palette_last_query.clone_from(&app.palette_query);
    }
    app.palette_selected = app.palette_selected.min(last);
    if down {
        app.palette_selected = (app.palette_selected + 1).min(last);
    } else if up {
        app.palette_selected = app.palette_selected.saturating_sub(1);
    }
    let selected = app.palette_selected;

    let rect_id = egui::Id::new("palette_rect");
    let previous_rect = ctx.data(|d| d.get_temp::<Rect>(rect_id));
    let outside_click = ctx.input(|i| {
        i.pointer.button_pressed(egui::PointerButton::Primary)
            && i.pointer.interact_pos().is_some_and(|p| previous_rect.is_some_and(|r| !r.contains(p)))
    });
    if outside_click {
        app.palette_open = false;
        return;
    }
    let screen = ctx.content_rect();
    let mut chosen: Option<&'static str> = None;
    let area = egui::Area::new(egui::Id::new("palette"))
        .order(egui::Order::Foreground)
        .pivot(Align2::CENTER_TOP)
        .fixed_pos(egui::pos2(screen.center().x, screen.top() + 96.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).corner_radius(CornerRadius::same(12)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                ui.set_width(560.0_f32.min(screen.width() - 32.0).max(280.0));
                ui.horizontal(|ui| {
                    ui.add(icons::image("search", 18.0, t.text_muted));
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut app.palette_query)
                            .hint_text(tl!("Search commands…"))
                            .frame(egui::Frame::NONE)
                            .font(theme::regular(15.0))
                            .desired_width(f32::INFINITY),
                    );
                    // Enter runs the selected hit while the field is focused or has just let go of focus.
                    if (field.has_focus() || field.lost_focus())
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        && let Some(hit) = found.get(selected)
                        && hit.ready
                    {
                        chosen = Some(hit.spec.id);
                    }
                    // Re-requesting focus that is already there would interrupt an input-method composition.
                    if !field.has_focus() {
                        field.request_focus();
                    }
                });
                ui.separator();
                for (index, hit) in found.iter().enumerate() {
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
                    let press = theme::Press::track(ui, &resp);
                    press.wash(ui, rect, 6, index == selected);
                    let body = rect.translate(press.offset());
                    icons::paint(ui, Rect::from_min_size(body.min + vec2(8.0, 9.0), vec2(18.0, 18.0)), hit.spec.icon, 17.0, t.icon);
                    let label = XmlCraftApp::command_label(hit.spec);
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, hit.ready, label));
                    let fg = if hit.ready { t.text } else { t.text_faint };
                    let detail_font = theme::regular(12.0);
                    let detail_w = widgets::text_width(ui, &hit.detail, &detail_font);
                    ui.painter().text(body.right_center() - vec2(10.0, 0.0), Align2::RIGHT_CENTER, &hit.detail, detail_font, t.text_faint);
                    ui.painter().text(body.left_center() + vec2(36.0, 0.0), Align2::LEFT_CENTER, label, theme::regular(13.5), fg);
                    let _ = detail_w;
                    if theme::hand(resp).clicked() && hit.ready {
                        chosen = Some(hit.spec.id);
                    }
                }
                if found.is_empty() {
                    ui.label(egui::RichText::new(tl!("No matching commands")).color(t.text_muted));
                }
                ui.add_space(2.0);
            });
        });
    ctx.data_mut(|d| d.insert_temp(rect_id, area.response.rect));
    if let Some(id) = chosen {
        app.palette_open = false;
        app.palette_query.clear();
        app.execute(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substring_matches_rank_by_position() {
        assert_eq!(score("Find tools and commands…", "find"), Some(0));
        assert_eq!(score("Find tools and commands…", "tools"), Some(5));
        assert!(score("Light theme", "theme") > score("Theme", "theme"));
    }

    #[test]
    fn matching_ignores_case_and_accepts_letters_in_order() {
        assert_eq!(score("Light THEME", "theme"), Some(6));
        assert_eq!(score("Use system theme", "ust"), Some(100), "u, s, t appear in order");
        assert_eq!(score("Quit", "zz"), None);
        assert_eq!(score("", "a"), None);
    }

    #[test]
    fn an_empty_query_matches_everything_equally() {
        assert_eq!(score("anything", ""), Some(0));
        assert_eq!(score("", ""), Some(0));
    }

    #[test]
    fn multibyte_text_ranks_by_characters_not_bytes() {
        assert_eq!(score("ÄÖÜ theme", "theme"), Some(4));
        assert_eq!(score("İstanbul", "x"), None);
    }
}
