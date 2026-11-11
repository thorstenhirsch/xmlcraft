//! Window chrome: the tab strip (with the integrated macOS title bar), the command bar with the actions that matter for the document, the
//! activity rail that opens the panes, and the status bar. The measurements are PdfCraft's, so the two apps read as one family.

use egui::{Align, Align2, Color32, CornerRadius, Layout, Rect, Sense, Stroke, vec2};

use crate::i18n::{fmt, tl};
use crate::kit::{self, PillKind};
use crate::theme::{self, ThemePreference, Tokens};
use crate::{Panel, XmlCraftApp, icons, widgets};

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
/// How narrow a tab may get before the strip scrolls instead.
const MIN_TAB: f32 = 150.0;
/// The room the strip keeps free on its right for New, Open and the theme button.
const STRIP_RESERVE: f32 = 230.0;

/// What the tab strip remembers between frames.
#[derive(Default)]
pub(crate) struct TabStripState {
    /// The tab that was last scrolled into view (a tab that becomes the shown one is scrolled to; one the person scrolled away from is left).
    scrolled_to: Option<usize>,
    /// A tab being dragged: where it was, and where it would go now.
    drag: Option<(usize, usize)>,
}

/// What the tab context menu asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TabAction {
    Close,
    CloseOthers,
    CloseRight,
    CopyPath,
}

/// The widths of the tabs: their natural widths when they fit in `room` (with `gap` between them), else shrunk in proportion, but not
/// below [`MIN_TAB`] (past that the strip scrolls).
pub(crate) fn tab_widths(naturals: &[f32], room: f32, gap: f32) -> Vec<f32> {
    let gaps = gap * naturals.len().saturating_sub(1) as f32;
    let total: f32 = naturals.iter().sum::<f32>() + gaps;
    if total <= room || naturals.is_empty() {
        return naturals.to_vec();
    }
    let scale = ((room - gaps) / naturals.iter().sum::<f32>()).max(0.0);
    naturals.iter().map(|w| (w * scale).max(MIN_TAB.min(*w))).collect()
}

/// The place a tab dragged to the pointer at `x` would take among tabs at `rects`.
pub(crate) fn drop_index(rects: &[Rect], x: f32) -> usize {
    rects.iter().position(|r| x <= r.right()).unwrap_or(rects.len().saturating_sub(1))
}

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
            let drag = ui.interact(ui.max_rect(), ui.scope_id().with("titledrag"), Sense::click_and_drag());
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
                let tabs = app.tabs();
                // The tabs shrink to share the room, then the strip scrolls (the shown tab is brought into view, and the wheel scrolls it).
                let room = (ui.available_width() - STRIP_RESERVE).max(MIN_TAB);
                let font = theme::regular(13.0);
                let naturals: Vec<f32> = tabs.iter().map(|i| widgets::text_width(ui, &tab_name(&i.name), &font) + TAB_CHROME).collect();
                let widths = tab_widths(&naturals, room, 4.0);
                let mut rects: Vec<Rect> = Vec::new();
                let mut dragging: Option<usize> = None;
                let mut dropped = false;
                let mut tab_action: Option<(usize, TabAction)> = None;
                let scroll = egui::ScrollArea::horizontal()
                    .id_salt("document-tabs")
                    .max_width(room)
                    .auto_shrink([true, false])
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for (i, tab_info) in tabs.iter().enumerate() {
                                let width = widths.get(i).copied().unwrap_or(MIN_TAB);
                                let response = tab(ui, &t, &tab_info.name, tab_info.dirty, active == Some(i), &mut close, i, tab_h, width);
                                if response.clicked() {
                                    app.session.set_active(Some(i));
                                    app.focus_editor = true;
                                }
                                // A middle click closes the tab, as in browsers.
                                if response.middle_clicked() {
                                    close = Some(i);
                                }
                                if response.dragged() {
                                    dragging = Some(i);
                                }
                                dropped |= response.drag_stopped();
                                if active == Some(i) && app.tab_strip.scrolled_to != Some(i) {
                                    response.scroll_to_me(Some(Align::Center));
                                    app.tab_strip.scrolled_to = Some(i);
                                }
                                let has_path = app.session.document(i).is_some_and(|d| d.path().is_some());
                                response.clone().context_menu(|ui| {
                                    if ui.button(tl!("Close")).clicked() {
                                        tab_action = Some((i, TabAction::Close));
                                        ui.close();
                                    }
                                    if ui.button(tl!("Close Others")).clicked() {
                                        tab_action = Some((i, TabAction::CloseOthers));
                                        ui.close();
                                    }
                                    if ui.button(tl!("Close Tabs to the Right")).clicked() {
                                        tab_action = Some((i, TabAction::CloseRight));
                                        ui.close();
                                    }
                                    if has_path && ui.button(tl!("Copy Path")).clicked() {
                                        tab_action = Some((i, TabAction::CopyPath));
                                        ui.close();
                                    }
                                });
                                rects.push(response.rect);
                            }
                        });
                        // The wheel scrolls the strip sideways where the system gives it only up and down.
                        if ui.rect_contains_pointer(ui.max_rect()) {
                            let delta = ui.input(|i| i.smooth_scroll_delta);
                            if delta.x == 0.0 && delta.y != 0.0 {
                                ui.scroll_with_delta(vec2(delta.y, 0.0));
                            }
                        }
                    });
                // A tab dragged along the strip: where it would go is marked, and it goes there when it is let go.
                if let (Some(from), Some(pointer)) = (dragging, ui.ctx().pointer_latest_pos()) {
                    let to = drop_index(&rects, pointer.x);
                    app.tab_strip.drag = Some((from, to));
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    if let Some(target) = rects.get(to).filter(|_| to != from) {
                        let x = if to > from { target.right() } else { target.left() };
                        ui.painter().vline(x, scroll.inner_rect.y_range(), Stroke::new(2.0, t.accent));
                    }
                }
                if dropped && let Some((from, to)) = app.tab_strip.drag.take() {
                    app.session.move_document(from, to);
                    app.tab_strip.scrolled_to = None;
                }
                if dragging.is_none() && !dropped {
                    app.tab_strip.drag = None;
                }
                if let Some(i) = close {
                    app.request_close(i);
                }
                if let Some((i, action)) = tab_action {
                    app.session.set_active(Some(i));
                    match action {
                        TabAction::Close => app.request_close(i),
                        TabAction::CloseOthers => {
                            let count = app.session.documents().len();
                            let left = app.close_clean_indices((0..count).filter(|j| *j != i).collect());
                            app.status = if left == 0 {
                                tl!("Closed the other tabs").to_owned()
                            } else {
                                fmt(tl!("Left {n} other tabs open with unsaved changes"), &[("n", &left.to_string())])
                            };
                        }
                        TabAction::CloseRight => {
                            let count = app.session.documents().len();
                            let left = app.close_clean_indices((i.saturating_add(1)..count).collect());
                            app.status = if left == 0 {
                                tl!("Closed the tabs to the right").to_owned()
                            } else {
                                fmt(tl!("Left {n} tabs open with unsaved changes"), &[("n", &left.to_string())])
                            };
                        }
                        TabAction::CopyPath => {
                            if let (Some(path), Some(ctx)) = (app.session.document(i).and_then(|d| d.path()), app.ctx.clone()) {
                                let path = path.display().to_string();
                                ctx.copy_text(path.clone());
                                app.status = fmt(tl!("Copied {path}"), &[("path", &path)]);
                            }
                        }
                    }
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

/// The icon of a document by what its name says it is: a schema, a DTD, a stylesheet, or plain XML.
pub(crate) fn file_icon(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, extension)| extension.to_ascii_lowercase()).as_deref() {
        Some("xsd") => "book-open",
        Some("dtd" | "ent" | "mod") => "file-code",
        Some("xsl" | "xslt") => "wand-sparkles",
        _ => "code-xml",
    }
}

/// A tab name, at most 28 characters (longer names end in "…").
pub fn tab_name(name: &str) -> String {
    if name.chars().count() > 28 { format!("{}…", name.chars().take(27).collect::<String>()) } else { name.to_string() }
}

/// A document tab: the file icon on the left, the name, and a close button (or a dot while there are unsaved changes) on the right.
#[allow(clippy::too_many_arguments)]
fn tab(
    ui: &mut egui::Ui,
    t: &Tokens,
    name: &str,
    dirty: bool,
    active: bool,
    close: &mut Option<usize>,
    index: usize,
    height: f32,
    width: f32,
) -> egui::Response {
    let font = theme::regular(13.0);
    let label = tab_name(name);
    // Full strip height so the active tab sits on the mode bar instead of floating in the title bar.
    let height = ui.available_height().max(height);
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let id = ui.scope_id().with(("document-tab", index));
    let a11y = if dirty { format!("{name} (edited)") } else { name.to_string() };
    let resp = craft_ui::tabs::Tab::new(id, &a11y, active).sense(Sense::click_and_drag()).focus_stroke(Stroke::new(1.0, t.accent)).show_at(
        ui,
        rect,
        |ui, resp| {
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
                file_icon(name),
                15.0,
                if active { t.accent } else { t.text_muted },
            );
            // (A tab that has been made narrower cuts its name before the close button.)
            let name_room = Rect::from_min_max(rect.min + vec2(28.0, 0.0), egui::pos2(rect.right() - 30.0, rect.bottom()));
            ui.painter().with_clip_rect(name_room).text(
                rect.min + vec2(28.0, rect.height() / 2.0),
                Align2::LEFT_CENTER,
                label,
                font,
                if active { t.text } else { t.text_muted },
            );
        },
    );
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

/// How wide the window must be for the command bar to write its actions out; narrower, they are icons.
const COMPACT_BELOW: f32 = 1000.0;

/// An action of the command bar, drawn from the command registry (its icon, its shortcut in the tooltip, disabled when it cannot run).
/// True when it was clicked.
fn action(app: &mut XmlCraftApp, ui: &mut egui::Ui, id: &'static str, label: &str, kind: PillKind, compact: bool) -> bool {
    let Some(spec) = xmlcraft_engine::commands::command(id) else { return false };
    let enabled = app.command_enabled(spec);
    let tip = match crate::commands::command_shortcut_label(ui.ctx(), id) {
        s if s.is_empty() => spec.label.to_owned(),
        s => format!("{} ({s})", spec.label),
    };
    let clicked = ui
        .add_enabled_ui(enabled, |ui| {
            if compact && kind != PillKind::Primary {
                icons::button(ui, spec.icon, 32.0, false, &tip)
            } else {
                kit::pill(ui, kind, Some(spec.icon), label).on_hover_text(&tip)
            }
        })
        .inner
        .clicked();
    clicked && enabled
}

pub fn command_bar(app: &mut XmlCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let editing = app.session.active().is_some();
    egui::Panel::top("command_bar")
        .exact_size(MODE_BAR)
        .frame(egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin::symmetric(12, 0)).stroke(Stroke::new(1.0, t.divider)))
        .show(ui, |ui| {
            let compact = ui.available_width() < COMPACT_BELOW;
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                main_menu(app, ui, compact);
                if editing {
                    ui.add_space(4.0);
                    ui.painter().vline(ui.cursor().left(), ui.max_rect().y_range().shrink(12.0), Stroke::new(1.0, t.divider));
                    ui.add_space(8.0);
                    // What matters most for an XML document, left to right: say whether it is valid; then change it or what it follows.
                    if action(app, ui, "xml.validate", tl!("Validate"), PillKind::Primary, compact) {
                        app.execute("xml.validate");
                    }
                    if action(app, ui, "xml.check", tl!("Check"), PillKind::Ghost, compact) {
                        app.execute("xml.check");
                    }
                    if action(app, ui, "xml.pretty_print", tl!("Format"), PillKind::Ghost, compact) {
                        app.execute("xml.pretty_print");
                    }
                    if action(app, ui, "xml.assign_schema", tl!("Assign Schema…"), PillKind::Ghost, compact) {
                        app.execute("xml.assign_schema");
                    }
                    ui.add_space(2.0);
                    kind_button(app, ui, compact);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.add_space(2.0);
                    let shortcut = crate::commands::command_shortcut_label(ui.ctx(), "view.palette");
                    let width = if compact { 190.0 } else { 260.0 };
                    if widgets::search_box(ui, tl!("Find tools and commands"), &shortcut, width).clicked() {
                        app.execute("view.palette");
                    }
                });
            });
        });
}

/// What the document can be turned into or checked against. An XML document gets Schema… (generate a schema from it, or open the one it
/// names); a schema or a DTD gets XML… (generate a sample document from it).
fn kind_button(app: &mut XmlCraftApp, ui: &mut egui::Ui, compact: bool) {
    use xmlcraft_engine::doc_kind::DocKind;
    match app.active_doc_kind() {
        Some(DocKind::Xml) => schema_button(app, ui, compact),
        Some(DocKind::Xsd | DocKind::Dtd) if action(app, ui, "xml.generate_sample", tl!("XML…"), PillKind::Secondary, compact) => {
            app.execute("xml.generate_sample");
        }
        Some(DocKind::Xsd | DocKind::Dtd | DocKind::Json | DocKind::Yaml | DocKind::Html) => {}
        None => {}
    }
}

/// Schema…: the actions on the schema of an XML document, in a menu.
fn schema_button(app: &mut XmlCraftApp, ui: &mut egui::Ui, compact: bool) {
    let (main, arrow) = if compact {
        let button = icons::button(ui, "file-code", 32.0, false, tl!("Schema…"));
        (button.clone(), button)
    } else {
        kit::split_pill(ui, Some("file-code"), tl!("Schema…"), tl!("More schema actions"))
    };
    // One popup for a single button; the split pill has two parts that open the same menu.
    let anchors = if compact { vec![main] } else { vec![main, arrow] };
    for anchor in anchors {
        egui::Popup::menu(&anchor).show(|ui| {
            ui.set_min_width(220.0);
            for id in ["xml.generate_schema", "xml.load_schema", "xml.schema_resolution"] {
                let Some(spec) = xmlcraft_engine::commands::command(id) else { continue };
                let enabled = app.command_enabled(spec);
                let mut item = ui.add_enabled(enabled, egui::Button::new(spec.label));
                if (id == "xml.load_schema" || id == "xml.schema_resolution") && !enabled {
                    item = item.on_disabled_hover_text(tl!("This document names no schema."));
                }
                if item.clicked() {
                    app.execute(id);
                    ui.close();
                }
            }
        });
    }
}

fn main_menu(app: &mut XmlCraftApp, ui: &mut egui::Ui, compact: bool) {
    let resp = if compact { icons::button(ui, "list", 32.0, false, tl!("Menu")) } else { widgets::ghost_button(ui, "list", tl!("Menu")) };
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

/// The problems of the document being shown, as (errors, warnings): the well-formedness check and, when it has been asked for, the
/// validation.
pub(crate) fn problem_counts(app: &XmlCraftApp) -> (usize, usize) {
    let (mut errors, mut warnings) = app.active_analysis().map_or((0, 0), |a| (a.error_count(), a.warning_count()));
    if let Some(v) = app.active_validation() {
        errors += v.error_count();
        warnings += v.warning_count();
    }
    (errors, warnings)
}

/// The width of the rail that opens the panes.
const RAIL_WIDTH: f32 = 48.0;

/// The rail on the left: one toggle for each pane (the outline, what the grammar allows here, the problems), with the number of
/// problems on the last. A pane that is open is marked.
pub fn activity_rail(app: &mut XmlCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let (errors, warnings) = if app.session.active().is_some() { problem_counts(app) } else { (0, 0) };
    let syntax = crate::editor_view::SyntaxTokens::for_kind(t.kind);
    let items = [
        ("list-tree", tl!("Outline"), "view.outline", app.panel_visible(Panel::Outline), 0usize, syntax.error),
        ("braces", tl!("Entry Helpers"), "view.entry_helpers", app.panel_visible(Panel::EntryHelpers), 0, syntax.error),
        (
            "triangle-alert",
            tl!("Problems"),
            "view.messages",
            app.panel_visible(Panel::Problems),
            errors + warnings,
            if errors > 0 { syntax.error } else { syntax.warning },
        ),
    ];
    egui::Panel::left("activity_rail")
        .exact_size(RAIL_WIDTH)
        .resizable(false)
        .frame(egui::Frame::NONE.fill(t.chrome).inner_margin(egui::Margin::symmetric(0, 10)).stroke(Stroke::new(1.0, t.divider)))
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                for (icon, label, command, open, count, badge) in items {
                    let response = rail_button(ui, icon, label, open, count);
                    if open {
                        // The mark of an open pane: a bar on the rail's outer edge.
                        let bar = Rect::from_min_size(egui::pos2(ui.max_rect().left(), response.rect.top() + 7.0), vec2(3.0, 22.0));
                        ui.painter().rect_filled(bar, CornerRadius { nw: 0, sw: 0, ne: 2, se: 2 }, t.accent);
                    }
                    if count > 0 {
                        let text = if count > 99 { "99+".to_owned() } else { count.to_string() };
                        let galley = ui.painter().layout_no_wrap(text, theme::semibold(9.5), Color32::WHITE);
                        let size = vec2((galley.size().x + 8.0).max(15.0), 15.0);
                        let pill = Rect::from_min_size(egui::pos2(response.rect.right() - size.x + 1.0, response.rect.top() + 1.0), size);
                        ui.painter().rect_filled(pill, CornerRadius::same(8), badge);
                        ui.painter().galley(pill.center() - galley.size() / 2.0, galley, Color32::WHITE);
                    }
                    if response.clicked() {
                        app.execute(command);
                    }
                }
            });
        });
}

/// A toggle on the rail: a rounded square that washes on hover, and is filled with the accent tint (the icon in the accent) while its pane is
/// open. Its accessible name says it toggles the pane, so that it is not mistaken for the pane's own title.
fn rail_button(ui: &mut egui::Ui, icon: &str, pane: &str, open: bool, count: usize) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::click());
    let mut name = crate::i18n::fmt(tl!("Toggle {pane}"), &[("pane", pane)]);
    if count > 0 {
        name = crate::i18n::fmt(tl!("{name}, {count} problems"), &[("name", &name), ("count", &count.to_string())]);
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, open, &name));
    let press = theme::Press::track(ui, &resp);
    press.wash(ui, rect, 8, open);
    icons::paint(ui, rect.translate(press.offset()), icon, 18.0, if open { t.accent_text } else { t.icon });
    theme::hand(resp).on_hover_text(pane)
}

pub fn status_bar(app: &mut XmlCraftApp, ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    let mut picked = None;
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
                    // About the document being shown: where the caret is, and how the file is written.
                    if let Some(doc) = app.session.active_document() {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        picked = status_parts(ui, doc);
                    }
                });
            });
        });
    if let Some(choice) = picked {
        app.apply_status_choice(choice);
    }
}

/// What a click in the status bar chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StatusChoice {
    LineEnding(xmlcraft_text::eol::Eol),
    Indent { tabs: bool, size: usize },
    Encoding(xmlcraft_text::encoding::Encoding, bool),
}

/// A piece of the status bar that can be clicked: its text, with a menu.
fn status_chip(ui: &mut egui::Ui, text: &str, tip: &str) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let response = ui.add(egui::Button::new(egui::RichText::new(text).font(theme::regular(11.5)).color(t.text_muted)).frame(false));
    theme::hand(response).on_hover_text(tip)
}

/// The summary of the document at the right of the status bar: the caret, then the encoding, line ending and indentation, each a menu.
/// Returns what was chosen in one of them.
fn status_parts(ui: &mut egui::Ui, doc: &xmlcraft_engine::Document) -> Option<StatusChoice> {
    use xmlcraft_text::encoding::Encoding;
    use xmlcraft_text::eol::Eol;
    let t = Tokens::get(ui.ctx());
    let parts = document_summary(doc);
    let settings = doc.editor().settings();
    let (encoding, bom) = (doc.encoding().encoding, doc.encoding().bom);
    let mixed = doc.line_endings().is_mixed();
    let mut chosen = None;
    // Right to left: the indentation, the line ending, the encoding, the caret.
    for (i, part) in parts.iter().enumerate().rev() {
        match i {
            1 => {
                let response = status_chip(ui, part, tl!("Encoding the file is written in"));
                egui::Popup::menu(&response).show(|ui| {
                    ui.set_min_width(180.0);
                    let items = [
                        ("UTF-8", Encoding::UTF_8, false),
                        ("UTF-8 BOM", Encoding::UTF_8, true),
                        ("UTF-16 LE BOM", Encoding::UTF_16LE, true),
                        ("UTF-16 BE BOM", Encoding::UTF_16BE, true),
                        ("Windows-1252", Encoding::WINDOWS_1252, false),
                    ];
                    for (label, enc, with_bom) in items {
                        let on = encoding == enc && bom == with_bom;
                        if ui.add(egui::Button::new(label).selected(on)).clicked() {
                            chosen = Some(StatusChoice::Encoding(enc, with_bom));
                            ui.close();
                        }
                    }
                });
            }
            2 => {
                let response = status_chip(ui, part, tl!("Line endings: click to convert the file"));
                egui::Popup::menu(&response).show(|ui| {
                    ui.set_min_width(180.0);
                    for (label, eol) in [("LF", Eol::Lf), ("CRLF", Eol::CrLf)] {
                        let on = !mixed && settings.eol == eol;
                        if ui.add(egui::Button::new(crate::i18n::fmt(tl!("Convert to {eol}"), &[("eol", label)])).selected(on)).clicked() {
                            chosen = Some(StatusChoice::LineEnding(eol));
                            ui.close();
                        }
                    }
                });
            }
            3 => {
                let response = status_chip(ui, part, tl!("Indentation of new lines"));
                egui::Popup::menu(&response).show(|ui| {
                    ui.set_min_width(180.0);
                    for size in [2usize, 4, 8] {
                        let on = !settings.use_tabs && settings.indent_size == size;
                        if ui.add(egui::Button::new(crate::i18n::fmt(tl!("Spaces: {n}"), &[("n", &size.to_string())])).selected(on)).clicked() {
                            chosen = Some(StatusChoice::Indent { tabs: false, size });
                            ui.close();
                        }
                    }
                    if ui.add(egui::Button::new(tl!("Tabs")).selected(settings.use_tabs)).clicked() {
                        chosen = Some(StatusChoice::Indent { tabs: true, size: settings.indent_size });
                        ui.close();
                    }
                });
            }
            _ => {
                ui.label(egui::RichText::new(part.as_str()).font(theme::regular(11.5)).color(t.text_muted));
            }
        }
    }
    chosen
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
    fn a_document_has_the_icon_of_what_its_name_says_it_is() {
        assert_eq!(file_icon("library.xsd"), "book-open");
        assert_eq!(file_icon("LIBRARY.XSD"), "book-open", "whatever the case");
        assert_eq!(file_icon("rules.dtd"), "file-code");
        assert_eq!(file_icon("xhtml-lat1.ent"), "file-code");
        assert_eq!(file_icon("page.xsl"), "wand-sparkles");
        assert_eq!(file_icon("page.xslt"), "wand-sparkles");
        for plain in ["a.xml", "Untitled-1", "", "xsd", ".hidden", "archive.tar.gz"] {
            assert_eq!(file_icon(plain), "code-xml", "{plain:?}");
        }
    }

    #[test]
    fn tabs_keep_their_width_while_they_fit_then_shrink_together_and_stop_at_the_least_width() {
        let naturals = [300.0, 200.0, 400.0];
        assert_eq!(tab_widths(&naturals, 1000.0, 4.0), naturals, "they fit");
        let shrunk = tab_widths(&naturals, 700.0, 4.0);
        assert!(shrunk.iter().sum::<f32>() + 8.0 <= 700.5, "{shrunk:?}");
        assert!(shrunk[0] < 300.0 && shrunk[2] < 400.0 && shrunk[2] > shrunk[0] && shrunk[1] >= MIN_TAB, "in proportion: {shrunk:?}");
        // A tab that is naturally narrower than the least stays as it is, and none goes below the least.
        let many = vec![150.0; 40];
        assert!(tab_widths(&many, 400.0, 4.0).iter().all(|w| (*w - MIN_TAB).abs() < f32::EPSILON), "they stop at the least, and the strip scrolls");
        assert_eq!(tab_widths(&[60.0], 10.0, 4.0), [60.0]);
        assert!(tab_widths(&[], 10.0, 4.0).is_empty());
        assert!(tab_widths(&naturals, f32::NAN, 4.0).iter().all(|w| w.is_finite()), "no NaN comes out");
    }

    #[test]
    fn the_place_a_dragged_tab_would_take_is_the_one_under_the_pointer() {
        let rect = |l: f32, r: f32| Rect::from_min_max(egui::pos2(l, 0.0), egui::pos2(r, 30.0));
        let rects = [rect(0.0, 100.0), rect(104.0, 204.0), rect(208.0, 308.0)];
        assert_eq!([-50.0, 0.0, 99.0, 103.0, 150.0, 250.0, 400.0].map(|x| drop_index(&rects, x)), [0, 0, 0, 1, 1, 2, 2]);
        assert_eq!(drop_index(&[], 10.0), 0);
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
