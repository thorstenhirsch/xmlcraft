//! Preview, the tool window that shows the document as a browser does: an HTML page as it is, an XML document through the XSLT
//! stylesheet it names (when that writes HTML), an SVG image, or any other XML as a readable tree (`xmlcraft_engine::preview`). The page
//! is worked out on a thread a moment after the last change to the text, so typing never waits for it.
//!
//! Where the build has a browser view (`crate::preview_host`; the native one waits for the owner's licence decision), the page is
//! shown in the panel: from memory, JavaScript off, nothing fetched, only the document navigable. A person may allow, for one
//! document, images from the web and the page's inline scripts; neither is on by default. Without a view (now everywhere) "Open in
//! Browser" writes the page to a temporary file, with the same policy as a `<meta>`, and opens it in the system browser; the file
//! follows the page and is removed when the app closes. "Open as Document" opens the page's HTML in a new tab. The document itself is never changed.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;

use xmlcraft_engine::language::Language;
use xmlcraft_engine::preview::{Allow, Input, Prepared, Shown, prepare, stylesheet_href};

use crate::http_pane::Outcome;
use crate::i18n::{fmt, tl};
use crate::kit::{self, PillKind, Tone};
use crate::preview_host::{Bounds, BrowserCopy, Host};
use crate::tools;
use crate::{Panel, XmlCraftApp, widgets};

/// How long the text is left alone before the page is worked out again, in seconds.
const DEBOUNCE: f64 = 0.3;

/// Where the stylesheet a document names is read from.
enum Sheet {
    /// An open document with that path: its text as it is in the editor.
    Text(String),
    /// The file, read on the worker; and the `href` as written, for the warning when it cannot be read.
    File(PathBuf, String),
}

/// The page shown, and what it was worked out for.
#[derive(Clone, Debug)]
struct Page {
    key: u64,
    doc: u64,
    name: String,
    result: Result<Arc<Prepared>, String>,
}

/// The Preview tool window: the page, the browser view and the copy for the system browser.
#[derive(Default)]
pub(crate) struct PreviewTool {
    /// What the page is for now (the document, its text, the stylesheet's text when it is open, Reload); a change starts the wait.
    key: u64,
    /// When (egui time, seconds) the wait for a quiet moment ends.
    due: Option<f64>,
    /// The request being waited for, and the key it was asked for.
    waiting: Option<(u64, u64)>,
    page: Option<Page>,
    /// Bumped by Reload: the stylesheet file may have changed on disk.
    reloads: u64,
    /// What each document allows (by id); nothing unless a person turned it on, and forgotten when the app closes.
    allowed: HashMap<u64, Allow>,
    host: Host,
    copy: BrowserCopy,
    /// Where the panel shows the page this frame (egui points), set while the panel is drawn.
    wanted: Option<egui::Rect>,
    /// The address last given to the system browser.
    opened: Option<String>,
}

impl PreviewTool {
    fn allow(&self, doc: u64) -> Allow {
        self.allowed.get(&doc).copied().unwrap_or_default()
    }

    /// What `ui.state` reports: how the page is shown, whether the browser view is up (or why there is none), the copy for the system
    /// browser.
    pub(crate) fn state(&self) -> serde_json::Value {
        let page = self.page.as_ref().map(|p| match &p.result {
            Ok(page) => serde_json::json!({ "document": p.doc, "shown": page.shown.name(), "title": page.title, "warnings": page.warnings }),
            Err(why) => serde_json::json!({ "document": p.doc, "error": why }),
        });
        let view = match (Host::EMBEDDED, self.host.failure()) {
            (false, _) => "none in this build".to_owned(),
            (true, Some(why)) => format!("failed: {why}"),
            (true, None) if self.host.is_shown() => "shown".to_owned(),
            (true, None) => "hidden".to_owned(),
        };
        serde_json::json!({ "page": page, "waiting": self.waiting.is_some(), "view": view, "browser_copy": self.copy.path().is_some() })
    }
}

/// What a document is previewed as, if it can be.
fn previewable(language: Language) -> bool {
    language == Language::Html || language.is_xml()
}

/// What the page shows, in a few words.
fn described(page: &Prepared) -> String {
    let how = match page.shown {
        Shown::Html => tl!("The page"),
        Shown::Transformed => tl!("The page its stylesheet makes"),
        Shown::Svg => tl!("The SVG image"),
        Shown::Tree => tl!("The document as a tree"),
    };
    match &page.title {
        Some(title) if page.shown != Shown::Svg => format!("{how} · {title}"),
        _ => how.to_owned(),
    }
}

impl XmlCraftApp {
    /// Preview in Browser View: show the Preview panel.
    pub(crate) fn open_preview(&mut self) {
        self.show_tool(Panel::Preview);
    }

    /// A number that changes when anything the page depends on changes: the document and its text, the open XSLT documents (the
    /// stylesheet may be one of them), Reload.
    fn preview_key(&self) -> Option<u64> {
        let doc = self.session.active_document()?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (doc.id(), doc.editor().version(), self.preview.reloads).hash(&mut hasher);
        for other in self.session.documents() {
            let sheet = other.path().and_then(|p| p.extension()).is_some_and(|e| e.eq_ignore_ascii_case("xsl") || e.eq_ignore_ascii_case("xslt"));
            if sheet && other.id() != doc.id() {
                (other.id(), other.editor().version()).hash(&mut hasher);
            }
        }
        Some(hasher.finish())
    }

    /// The stylesheet the document being shown names: the open document with that path (what is being edited), else the file.
    fn preview_sheet(&self, text: &str) -> Option<Sheet> {
        let href = stylesheet_href(text)?;
        let base = self.session.active_document().and_then(|d| d.path().map(std::path::Path::to_path_buf));
        let wanted = match base.as_deref().and_then(std::path::Path::parent) {
            Some(dir) => dir.join(&href),
            None => PathBuf::from(&href),
        };
        if let Some(open) = self.session.documents().iter().find(|d| d.path().is_some_and(|p| p == wanted)) {
            return Some(Sheet::Text(open.editor().text()));
        }
        Some(Sheet::File(wanted, href))
    }

    /// Work out the page on a thread.
    fn start_page(&mut self, key: u64) {
        let Some(doc) = self.session.active_document() else { return };
        let (id, name, language, text) = (doc.id(), doc.name().to_owned(), doc.language(), doc.editor().text());
        let sheet = if language.is_xml() { self.preview_sheet(&text) } else { None };
        let serial = self.next_http_serial();
        let started = self.spawn_http(move || {
            let mut warnings = Vec::new();
            let sheet = match sheet {
                None => None,
                Some(Sheet::Text(text)) => Some(text),
                Some(Sheet::File(path, href)) => match crate::tools::query::read_decoded(&path) {
                    Ok((_, text)) => Some(text),
                    Err(why) => {
                        warnings
                            .push(fmt(tl!("The stylesheet the document names ({href}) could not be read: {why}"), &[("href", &href), ("why", &why)]));
                        None
                    }
                },
            };
            let input = if language == Language::Html { Input::Html(&text) } else { Input::Xml { text: &text, stylesheet: sheet.as_deref() } };
            let result = xmlcraft_engine::guard(|| prepare(input))
                .map_err(|_| tl!("the preview failed (a bug); nothing was changed").to_owned())
                .and_then(|made| made.map_err(|e| fmt(tl!("The document cannot be previewed: {why}"), &[("why", &e.to_string())])))
                .map(|mut page| {
                    warnings.append(&mut page.warnings);
                    page.warnings = warnings;
                    Box::new(page)
                });
            Outcome::Rendered { serial, doc: id, name, result }
        });
        match started {
            Ok(()) => self.preview.waiting = Some((serial, key)),
            Err(why) => self.preview.page = Some(Page { key, doc: id, name: doc_name(self), result: Err(why) }),
        }
    }

    /// The answer of a page thread, whatever else is open. An answer for an older request is dropped.
    pub(crate) fn page_rendered(&mut self, serial: u64, doc: u64, name: String, result: Result<Box<Prepared>, String>) {
        let Some((_, key)) = self.preview.waiting.take_if(|(s, _)| *s == serial) else { return };
        let result: Result<Arc<Prepared>, String> = result.map(Arc::from);
        let allow = self.preview.allow(doc);
        if let Ok(page) = &result {
            self.preview.host.set_page(Some(page.clone()), allow);
            self.preview.copy.refresh(doc, &name, page, allow);
        } else {
            self.preview.host.set_page(None, allow);
        }
        self.preview.page = Some(Page { key, doc, name, result });
    }

    /// Change what the document being previewed allows; the page is served again with it.
    fn set_preview_allow(&mut self, doc: u64, allow: Allow) {
        if allow == Allow::default() {
            self.preview.allowed.remove(&doc);
        } else {
            self.preview.allowed.insert(doc, allow);
        }
        if let Some(page) = &self.preview.page
            && page.doc == doc
        {
            let current = page.result.as_ref().ok().cloned();
            if let Some(shown) = &current {
                self.preview.copy.refresh(doc, &page.name.clone(), shown, allow);
            }
            self.preview.host.set_page(current, allow);
        }
    }

    /// Open in Browser: write the page to its temporary file and give that to the system browser.
    fn open_preview_in_browser(&mut self, ctx: &egui::Context) {
        let Some(page) = &self.preview.page else { return };
        let Ok(shown) = page.result.clone() else { return };
        let (doc, name) = (page.doc, page.name.clone());
        let allow = self.preview.allow(doc);
        match self.preview.copy.write(doc, &name, &shown, allow) {
            Ok(path) => {
                let url = xmlcraft_platform::file_uri::uri_of(&path.to_string_lossy());
                ctx.open_url(egui::OpenUrl::new_tab(url.clone()));
                self.preview.opened = Some(url);
                self.status = fmt(tl!("Opened {name} in the browser"), &[("name", &name)]);
            }
            Err(why) => self.status = fmt(tl!("The page cannot be opened in the browser: {why}"), &[("why", &why)]),
        }
    }

    /// Open as Document: the page's HTML in a new tab.
    fn open_preview_as_document(&mut self) {
        let Some(page) = &self.preview.page else { return };
        let Ok(shown) = page.result.clone() else { return };
        let stem = page.name.rsplit_once('.').map_or(page.name.as_str(), |(stem, _)| stem).to_owned();
        let extension = if shown.media_type == xmlcraft_engine::preview::SVG_MEDIA_TYPE { "svg" } else { "html" };
        self.open_generated(&format!("{stem}-preview.{extension}"), &shown.body);
    }

    /// After the frame is drawn: show the browser view where the panel wants it, unless something of egui's is above that place.
    pub(crate) fn sync_preview(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let wanted = self.preview.wanted.take();
        let open = self.workbench.is_visible(Panel::Preview);
        let blocked = self.dialog.is_some() || self.palette_open;
        let bounds = wanted.filter(|rect| !blocked && !covered(ctx, *rect)).and_then(|rect| Bounds::of(rect, ctx.pixels_per_point()));
        let pressed = ctx.input(|i| i.pointer.any_pressed());
        self.preview.host.sync(frame, open, bounds, pressed);
    }

    /// The app is closing: the view goes, and so do the copies for the system browser. (Called when a close request goes through.)
    pub fn close_preview(&mut self) {
        self.preview.host.close();
        self.preview.copy.remove();
    }

    /// Whether the browser view is shown now (tests).
    pub fn preview_view_shown(&self) -> bool {
        self.preview.host.is_shown()
    }

    /// The page the Preview shows (how, its title, its warnings), or why there is none, once worked out (tests).
    pub fn preview_page(&self) -> Option<Result<Arc<Prepared>, String>> {
        self.preview.page.as_ref().map(|p| p.result.clone())
    }

    /// The address last opened in the system browser, and the copy's file (tests).
    pub fn preview_browser_copy(&self) -> (Option<String>, Option<PathBuf>) {
        (self.preview.opened.clone(), self.preview.copy.path().map(std::path::Path::to_path_buf))
    }
}

fn doc_name(app: &XmlCraftApp) -> String {
    app.session.active_document().map(|d| d.name().to_owned()).unwrap_or_default()
}

/// Whether a window, menu, popup or drag of egui's is over `rect` (the browser view would hide it). Tooltips are let be.
fn covered(ctx: &egui::Context, rect: egui::Rect) -> bool {
    let dragging_over = ctx.dragged_id().is_some() && ctx.pointer_latest_pos().is_some_and(|p| rect.contains(p));
    let layers = ctx.memory(|m| m.areas().visible_layer_ids());
    dragging_over
        || layers.into_iter().any(|layer| {
            matches!(layer.order, egui::Order::Middle | egui::Order::Foreground | egui::Order::Debug)
                && egui::AreaState::load(ctx, layer.id).is_some_and(|state| state.rect().intersects(rect))
        })
}

/// The body of the Preview panel.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    let bound = tools::bound(app);
    let language = app.session.active_document().map(|d| d.language());
    let Some((bound, language)) = bound.zip(language) else {
        kit::tool_body(ui, false, |ui| {
            tools::needs(ui, tl!("Preview needs a document"), tl!("Open an HTML or XML document to see it as a browser does."))
        });
        return;
    };
    if !previewable(language) {
        kit::tool_body(ui, false, |ui| {
            tools::bound_line(ui, &bound.name);
            tools::needs(ui, tl!("Preview shows HTML and XML"), tl!("This document is neither."));
        });
        return;
    }
    // A new key starts the wait for a quiet moment; when it is over, the thread starts.
    let key = app.preview_key().unwrap_or_default();
    let now = ui.input(|i| i.time);
    if app.preview.key != key {
        app.preview.key = key;
        app.preview.due = Some(now + DEBOUNCE);
    }
    if let Some(due) = app.preview.due {
        if now >= due && app.preview.waiting.is_none() {
            app.preview.due = None;
            app.start_page(key);
        } else if now < due {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(due - now));
        }
    }
    let current = app.preview.page.as_ref().filter(|p| p.doc == bound.id).map(|p| (p.result.clone(), p.key == key));
    let shown = current.as_ref().and_then(|(r, _)| r.as_ref().ok().cloned());
    let can_browse = shown.is_some() && !cfg!(target_arch = "wasm32");
    let why_not = if cfg!(target_arch = "wasm32") { tl!("The web build writes no files.") } else { tl!("There is no page yet.") };
    let browse = kit::tool_footer(ui, tl!("Open in Browser"), Some("external-link"), can_browse, why_not);
    let embedded = Host::EMBEDDED && app.preview.host.failure().is_none();
    let offline = app.network_is_offline();
    let mut allow = app.preview.allow(bound.id);
    let mut asked = Asked::default();
    kit::tool_body(ui, false, |ui| {
        tools::bound_line(ui, &bound.name);
        ui.add_space(4.0);
        let updating = app.preview.waiting.is_some() || app.preview.due.is_some() || !current.as_ref().is_some_and(|(_, fresh)| *fresh);
        ui.horizontal(|ui| {
            match &current {
                Some((Ok(page), _)) => {
                    ui.add(egui::Label::new(egui::RichText::new(described(page)).font(crate::theme::medium(12.0))).truncate());
                }
                Some((Err(_), _)) | None => {}
            }
            if updating {
                ui.spinner();
            }
        });
        ui.horizontal_wrapped(|ui| {
            let images = ui.add_enabled(!offline || allow.remote_images, widgets::checkbox(&mut allow.remote_images, tl!("Images from the web")));
            let images = if offline { images.on_disabled_hover_text(tl!("Work Offline is on: nothing is downloaded.")) } else { images };
            images.on_hover_text(tl!("Load the images this document names at https: addresses (the network). Off by default."));
            ui.add(widgets::checkbox(&mut allow.scripts, tl!("Scripts of the page")))
                .on_hover_text(tl!("Run the page's own inline scripts, never scripts from elsewhere and never a connection. Off by default."));
            asked.reload = kit::pill_sized(ui, PillKind::Ghost, None, tl!("Reload"), 24.0).clicked();
            asked.as_document = ui
                .add_enabled_ui(shown.is_some(), |ui| kit::pill_sized(ui, PillKind::Ghost, Some("file-code"), tl!("Open as Document"), 24.0))
                .inner
                .clicked();
        });
        if let Some((Ok(page), _)) = &current
            && let Some(first) = page.warnings.first()
        {
            kit::help(ui, first);
        }
        ui.add_space(kit::GAP);
        match &current {
            Some((Err(message), _)) => kit::banner(ui, Tone::Bad, message),
            Some((Ok(_), _)) if embedded => {
                // The browser view goes here, after the frame (`sync_preview`).
                let rect = ui.available_rect_before_wrap().intersect(ui.clip_rect());
                let t = crate::theme::Tokens::get(ui.ctx());
                ui.painter().rect_filled(rect, 4.0, t.panel);
                let response = ui.allocate_rect(rect, egui::Sense::hover());
                widgets::name_for_assistive_technology(ui, &response, tl!("The browser view"));
                app.preview.wanted = Some(rect);
            }
            Some((Ok(_), _)) => {
                let why = match app.preview.host.failure() {
                    Some(failure) => fmt(tl!("The browser view could not be made ({why}). Open the page in your browser."), &[("why", failure)]),
                    None if cfg!(target_arch = "wasm32") => tl!("The web build has no browser view. Open the page as a document.").to_owned(),
                    None => tl!("This build has no browser view. Open the page in your browser; it is kept up to date.").to_owned(),
                };
                widgets::empty_state(ui, kit::GAP_GROUP, tl!("The page opens in your browser"), &why);
            }
            None => {}
        }
    });
    if allow != app.preview.allow(bound.id) {
        app.set_preview_allow(bound.id, allow);
    }
    if asked.reload {
        app.preview.reloads = app.preview.reloads.wrapping_add(1);
    }
    if asked.as_document {
        app.open_preview_as_document();
    }
    if browse && can_browse {
        app.open_preview_in_browser(ui.ctx());
    }
}

/// What a frame of the panel asked for.
#[derive(Default)]
struct Asked {
    reload: bool,
    as_document: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_and_xml_can_be_previewed_and_data_cannot() {
        assert!(previewable(Language::Html) && previewable(Language::Xml));
        assert!(!previewable(Language::Yaml) && !previewable(Language::Json(xmlcraft_engine::language::JsonFlavor::Json)));
    }

    #[test]
    fn a_window_or_menu_over_the_place_covers_it_and_a_tooltip_or_one_elsewhere_does_not() {
        let ctx = egui::Context::default();
        let place = egui::Rect::from_min_size(egui::pos2(100.0, 100.0), egui::vec2(200.0, 200.0));
        let frame = |what: Option<(egui::Order, egui::Pos2)>| {
            let input =
                egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0))), ..Default::default() };
            let mut output = ctx.run_ui(input, |ui| {
                if let Some((order, at)) = what {
                    egui::Area::new(egui::Id::unique("over")).order(order).fixed_pos(at).show(ui.ctx(), |ui| ui.label("over"));
                }
            });
            // (Nothing is painted: the font textures egui made are let go.)
            output.textures_delta.clear();
        };
        frame(None);
        frame(None);
        assert!(!covered(&ctx, place), "nothing above it");
        frame(Some((egui::Order::Foreground, egui::pos2(150.0, 150.0))));
        assert!(covered(&ctx, place), "a menu over it");
        frame(None);
        frame(None);
        assert!(!covered(&ctx, place), "gone a frame after it closed");
        frame(Some((egui::Order::Middle, egui::pos2(120.0, 280.0))));
        assert!(covered(&ctx, place), "a window over a corner of it");
        frame(None);
        frame(None);
        frame(Some((egui::Order::Foreground, egui::pos2(600.0, 500.0))));
        assert!(!covered(&ctx, place), "a menu elsewhere");
        frame(None);
        frame(None);
        frame(Some((egui::Order::Tooltip, egui::pos2(150.0, 150.0))));
        assert!(!covered(&ctx, place), "a tooltip is let be");
    }

    #[test]
    fn what_is_shown_is_said_with_the_title() {
        let page = |shown, title: Option<&str>| Prepared {
            body: String::new(),
            media_type: xmlcraft_engine::preview::HTML_MEDIA_TYPE,
            shown,
            title: title.map(str::to_owned),
            warnings: Vec::new(),
        };
        assert_eq!(described(&page(Shown::Html, Some("Home"))), "The page · Home");
        assert_eq!(described(&page(Shown::Tree, None)), "The document as a tree");
        assert_eq!(described(&page(Shown::Svg, Some("SVG"))), "The SVG image");
    }
}
