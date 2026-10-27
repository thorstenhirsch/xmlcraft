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
                let active = app.session.active();
                if icons::button(ui, "house", button, active.is_none(), tl!("Home")).clicked() {
                    app.session.set_active(None);
                }
                let mut close = None;
                for (i, tab_info) in app.tabs().iter().enumerate() {
                    let response = tab(ui, &t, &tab_info.name, tab_info.dirty, active == Some(i), &mut close, i, tab_h);
                    if response.clicked() {
                        app.session.set_active(Some(i));
                        app.focus_editor = true;
                    }
                    // A middle click closes the tab, as in browsers.
                    if response.middle_clicked() {
                        close = Some(i);
                    }
                }
                if let Some(i) = close {
                    app.request_close(i);
                }
                ui.add_space(4.0);
                // New and Open sit next to the tabs, as in a browser.
                if widgets::ghost_button_sized(ui, "plus", tl!("New"), tab_h).clicked() {
                    app.execute("file.new");
                }
                if widgets::ghost_button_sized(ui, "folder-open", tl!("Open"), tab_h).clicked() {
                    app.execute("file.open");
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let (icon, label) = match app.theme_preference {
                        ThemePreference::System => ("monitor", tl!("Use system setting")),
                        ThemePreference::Light => ("sun", tl!("Light")),
                        ThemePreference::Dark => ("moon", tl!("Dark")),
                    };
                    let tip = format!("{}: {label}", tl!("Display theme"));
                    // A click cycles System, Light, Dark.
                    if icons::button(ui, icon, button, false, &tip).clicked() {
                        app.execute(match app.theme_preference.next() {
                            ThemePreference::System => "view.theme.system",
                            ThemePreference::Light => "view.theme.light",
                            ThemePreference::Dark => "view.theme.dark",
                        });
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
                if widgets::mode_tab(ui, tl!("Text"), app.mode == Mode::Text).clicked() {
                    app.mode = Mode::Text;
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.add_space(8.0);
                    let shortcut = crate::commands::command_shortcut_label(ui.ctx(), "view.palette");
                    if widgets::search_box(ui, tl!("Find tools and commands"), &shortcut, 260.0).clicked() {
                        app.execute("view.palette");
                    }
                });
            });
        });
}

fn main_menu(app: &mut XmlCraftApp, ui: &mut egui::Ui) {
    let resp = widgets::ghost_button(ui, "panel-left", tl!("Menu"));
    egui::Popup::menu(&resp).show(|ui| {
        ui.set_min_width(230.0);
        // Straight from the command registry; a menu with no commands yet is not shown.
        for menu in xmlcraft_engine::commands::MENUS {
            if xmlcraft_engine::commands::in_menu(menu).next().is_some() {
                ui.menu_button(crate::i18n::t(menu), |ui| crate::commands::registry_menu(app, ui, menu));
            }
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
                let status = if app.status.is_empty() { tl!("Ready") } else { app.status.as_str() };
                ui.label(egui::RichText::new(status).font(theme::regular(11.5)).color(t.text_muted));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.label(egui::RichText::new(format!("{} {}", crate::APP_NAME, crate::VERSION)).font(theme::regular(11.5)).color(t.text_faint));
                    if !app.bottom_open && widgets::ghost_button_sized(ui, "panel-bottom", tl!("Messages"), 22.0).clicked() {
                        app.execute("view.messages");
                    }
                    // About the document being shown: where the caret is, and how the file is written.
                    if let Some(doc) = app.session.active_document() {
                        ui.spacing_mut().item_spacing.x = 14.0;
                        for part in document_summary(doc).into_iter().rev() {
                            ui.label(egui::RichText::new(part).font(theme::regular(11.5)).color(t.text_muted));
                        }
                        // Whether the document is well-formed: a click shows the list.
                        let syntax = crate::editor_view::SyntaxTokens::for_kind(t.kind);
                        let (text, color) = match app.active_analysis() {
                            None => (tl!("Checking…").to_owned(), t.text_faint),
                            Some(a) if a.skipped.is_some() => (tl!("Not checked").to_owned(), t.text_faint),
                            Some(a) if a.error_count() > 0 => {
                                (crate::i18n::fmt(tl!("{count} errors"), &[("count", &a.error_count().to_string())]), syntax.error)
                            }
                            Some(_) => (tl!("Well-formed").to_owned(), t.text_muted),
                        };
                        let badge = ui.add(egui::Label::new(egui::RichText::new(text).font(theme::medium(11.5)).color(color)).sense(Sense::click()));
                        if badge.clicked() {
                            app.bottom_open = true;
                        }
                    }
                });
            });
        });
}

/// What the status bar says about a document, left to right: the caret, the encoding, the line ending and the indentation.
pub(crate) fn document_summary(doc: &xmlcraft_engine::Document) -> Vec<String> {
    let editor = doc.editor();
    let caret = editor.primary();
    let buffer = editor.buffer();
    let line = buffer.line_of(caret.head());
    let column = xmlcraft_editor::motion::display_column(buffer, caret.head(), editor.settings().tab_width);
    let mut position = crate::i18n::fmt(tl!("Ln {line}, Col {col}"), &[("line", &(line + 1).to_string()), ("col", &(column + 1).to_string())]);
    if !caret.is_empty() {
        let chars = buffer.slice(caret.start()..caret.end()).map_or(0, |s| s.chars().count());
        position.push_str(&crate::i18n::fmt(tl!(" ({n} selected)"), &[("n", &chars.to_string())]));
    }
    let encoding = doc.encoding();
    let mut name = encoding.encoding.name().to_owned();
    if encoding.bom {
        name.push_str(" BOM");
    }
    let endings = doc.line_endings();
    let eol = if endings.is_mixed() { tl!("Mixed line endings").to_owned() } else { editor.settings().eol.label().to_owned() };
    let indent = if editor.settings().use_tabs {
        tl!("Tabs").to_owned()
    } else {
        crate::i18n::fmt(tl!("Spaces: {n}"), &[("n", &editor.settings().indent_size.to_string())])
    };
    vec![position, name, eol, indent]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_summary_describes_the_caret_and_the_file() {
        let mut session = xmlcraft_engine::Session::new();
        session.open(None, b"\xEF\xBB\xBF<a>\r\n\t<b/>\r\n</a>", None).expect("opens");
        let doc = session.active_document_mut().expect("shown");
        doc.editor_mut().set_selection(5, 7);
        let summary = document_summary(doc);
        assert_eq!(summary, ["Ln 2, Col 6 (2 selected)", "UTF-8 BOM", "CRLF", "Tabs"]);
        doc.editor_mut().place_caret(0, false);
        assert_eq!(document_summary(doc)[0], "Ln 1, Col 1");
    }

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
