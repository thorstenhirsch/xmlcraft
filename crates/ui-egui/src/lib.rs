#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! The egui shell of XmlCraft: the window chrome, panes and theme, with the look of PdfCraft.

mod chrome;
mod commands;
mod control;
mod dialog;
mod documents;
pub mod editor_view;
mod home;
mod i18n;
mod icon_data;
mod icons;
mod motion;
mod palette;
mod pickers;
mod side_panels;
pub mod theme;
pub use control::{ControlClient, Reply as ControlReply};
mod widgets;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use xmlcraft_engine::Session;

use crate::dialog::Dialog;
use crate::documents::DocView;
use crate::pickers::Pickers;
use crate::side_panels::{RightPanel, SidePanels};
use crate::theme::{ThemeKind, ThemePreference};

pub const APP_NAME: &str = "XmlCraft";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The key under which the preferences are saved in eframe's storage.
const STORAGE_KEY: &str = "xmlcraft";

/// The editing views of a document. Only the text view exists in the first release.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    #[default]
    Text,
}

/// A document tab as the strip shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabInfo {
    pub name: String,
    pub dirty: bool,
}

/// What is saved between runs. Anything missing or malformed falls back to its default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Prefs {
    theme: ThemePreference,
    left_open: bool,
    right: Option<RightPanel>,
    bottom_open: bool,
    side_panels: SidePanels,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            left_open: true,
            right: Some(RightPanel::EntryHelpers),
            bottom_open: true,
            side_panels: SidePanels::default(),
        }
    }
}

impl Prefs {
    /// Read saved preferences; anything unreadable gives the defaults rather than an error.
    fn load(saved: Option<&str>) -> Self {
        let mut prefs: Self = saved.and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
        prefs.side_panels = prefs.side_panels.sanitized();
        prefs
    }
}

pub struct XmlCraftApp {
    pub theme_preference: ThemePreference,
    pub mode: Mode,
    pub left_open: bool,
    pub right: Option<RightPanel>,
    pub bottom_open: bool,
    pub palette_open: bool,
    pub palette_query: String,
    palette_last_query: String,
    palette_selected: usize,
    /// The open documents and the commands that act on them.
    pub session: Session,
    /// What the editor keeps for each open document (layout caches, colouring), by document id.
    views: HashMap<u64, DocView>,
    /// The question being asked, if any.
    dialog: Option<Dialog>,
    pickers: Pickers,
    /// Answers the next file picker with these paths instead of showing one (tests and automation).
    pub pick_override: Option<Vec<std::path::PathBuf>>,
    /// The egui context, for commands that show panels and dialogs.
    ctx: Option<egui::Context>,
    /// Put the keyboard focus in the editor on the next frame.
    focus_editor: bool,
    /// The app is closing, one document at a time, asking about each that has unsaved changes.
    quitting: bool,
    /// Every question is answered: the next close request goes through.
    quit_confirmed: bool,
    /// Whether the window had the focus last frame, to notice getting it back.
    was_focused: bool,
    /// Commands for the window that a command asked for; sent on the next frame.
    viewport_commands: Vec<egui::ViewportCommand>,
    /// What the status bar says instead of "Ready": the last command's trouble, if any.
    pub status: String,
    /// The opt-in control channel (`--control`), when one is attached.
    control: Option<control::Control>,
    /// The file that tells clients where the control channel is; removed when the window closes for good.
    control_file: Option<std::path::PathBuf>,
    /// On macOS the tab strip is the title bar.
    pub integrated_titlebar: bool,
    side_panels: SidePanels,
    /// The colours currently installed in egui, so they are applied only when they change.
    applied: Option<ThemeKind>,
}

impl XmlCraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        theme::install_fonts(&cc.egui_ctx);
        let prefs = Prefs::load(cc.storage.and_then(|s| s.get_string(STORAGE_KEY)).as_deref());
        let mut app = Self::with_prefs(prefs);
        app.ctx = Some(cc.egui_ctx.clone());
        app
    }

    fn with_prefs(prefs: Prefs) -> Self {
        Self {
            theme_preference: prefs.theme,
            mode: Mode::default(),
            left_open: prefs.left_open,
            right: prefs.right,
            bottom_open: prefs.bottom_open,
            palette_open: false,
            palette_query: String::new(),
            palette_last_query: String::new(),
            palette_selected: 0,
            session: Session::new(),
            views: HashMap::new(),
            dialog: None,
            pickers: Pickers::default(),
            pick_override: None,
            ctx: None,
            focus_editor: false,
            quitting: false,
            quit_confirmed: false,
            was_focused: true,
            viewport_commands: Vec::new(),
            status: String::new(),
            control: None,
            control_file: None,
            integrated_titlebar: cfg!(target_os = "macos"),
            side_panels: prefs.side_panels,
            applied: None,
        }
    }

    fn prefs(&self) -> Prefs {
        Prefs {
            theme: self.theme_preference,
            left_open: self.left_open,
            right: self.right,
            bottom_open: self.bottom_open,
            side_panels: self.side_panels,
        }
    }

    /// Attach the opt-in control channel (`--control`): the returned client sends requests that the app answers at the start of
    /// each frame. Called once, before the first frame.
    pub fn attach_control(&mut self, ctx: &egui::Context) -> ControlClient {
        let (control, client) = control::attach(ctx);
        self.control = Some(control);
        client
    }

    /// Remember the control file, so it is removed when the app closes (it names a port nobody listens on after that).
    pub fn set_control_file(&mut self, file: std::path::PathBuf) {
        self.control_file = Some(file);
    }

    /// Remove the control file, once. Called when a close request is about to go through; on macOS the process ends in
    /// `terminate:` without returning to `main`, so the app cannot leave this to its caller.
    pub fn release_control_file(&mut self) {
        if let Some(file) = self.control_file.take() {
            let _ = std::fs::remove_file(file);
        }
    }

    /// The colours currently installed, once a frame has applied them.
    pub fn applied_theme(&self) -> Option<ThemeKind> {
        self.applied
    }

    /// Install the colours for the current preference and system setting, when they changed.
    fn sync_theme(&mut self, ctx: &egui::Context) {
        let system = ctx.system_theme().map(|t| t == egui::Theme::Dark);
        let kind = self.theme_preference.resolve(system, ThemeKind::Light);
        if self.applied != Some(kind) {
            theme::apply(ctx, kind);
            self.applied = Some(kind);
            // Open documents are coloured again, in the new theme's colours.
            for view in self.views.values_mut() {
                view.styler.set_theme(kind);
                view.view.highlight_version += 1;
            }
        }
    }
}

impl eframe::App for XmlCraftApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sync_theme(ctx);
        if let Some(mut control) = self.control.take() {
            control.tick(ctx, self);
            self.control = Some(control);
        }
        self.handle_shortcuts(ctx);
        self.process_picks(ctx);
        self.handle_dropped_files(ctx);
        // Getting the window back: another program may have changed the files in the meantime.
        let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        if focused && !self.was_focused {
            self.check_disk_changes();
        }
        self.was_focused = focused;
        for command in self.viewport_commands.drain(..) {
            ctx.send_viewport_cmd(command);
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.quit_confirmed || !self.has_unsaved_changes() {
                self.release_control_file();
            } else {
                // Unsaved work: stop the close and ask, document by document.
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.request_quit(ctx);
            }
        }
        // Last, so it sees any `CancelClose` above: on macOS a close quits through `terminate:` (the Touch Bar crash).
        craft_quit::on_close_request(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let t = theme::Tokens::get(ui.ctx());
        chrome::tab_strip(self, ui);
        chrome::mode_bar(self, ui);
        // The status bar is outermost at the bottom, the messages panel sits above it.
        chrome::status_bar(self, ui);
        side_panels::show(self, ui);
        palette::show(self, ui.ctx());
        let fill = if self.session.has_document() { editor_view::editor_colors(&t).background } else { t.pasteboard };
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(fill)).show(ui, |ui| self.central(ui));
        self.show_dialog(ui.ctx());
        self.show_drop_hint(ui.ctx());
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        match serde_json::to_string(&self.prefs()) {
            Ok(json) => storage.set_string(STORAGE_KEY, json),
            Err(e) => log::warn!("could not save the preferences: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_tabs(n: usize) -> XmlCraftApp {
        let mut app = XmlCraftApp::with_prefs(Prefs::default());
        for i in 0..n {
            app.session.open(Some(format!("/x/{i}.xml").into()), b"<a/>", None).expect("opens");
        }
        app
    }

    #[test]
    fn defaults_open_the_outline_entry_helpers_and_messages() {
        let p = Prefs::default();
        assert!(p.left_open && p.bottom_open);
        assert_eq!(p.right, Some(RightPanel::EntryHelpers));
        assert_eq!(p.theme, ThemePreference::System);
    }

    #[test]
    fn preferences_round_trip() {
        let p = Prefs {
            theme: ThemePreference::Dark,
            left_open: false,
            right: None,
            side_panels: SidePanels { outline_width: 300.0, ..SidePanels::default() },
            ..Prefs::default()
        };
        let json = serde_json::to_string(&p).expect("serialises");
        assert_eq!(Prefs::load(Some(&json)), p);
    }

    #[test]
    fn unreadable_preferences_give_the_defaults() {
        for saved in [None, Some(""), Some("not json"), Some("[1,2]"), Some("{\"theme\": 7}"), Some("null")] {
            assert_eq!(Prefs::load(saved), Prefs::default(), "{saved:?}");
        }
    }

    #[test]
    fn hostile_preference_values_are_repaired() {
        let p = Prefs::load(Some(r#"{"side_panels": {"messages_height": 1e30, "outline_width": -5.0, "helpers_width": 0.0}}"#));
        assert_eq!(p.side_panels.messages_height, 600.0);
        assert_eq!(p.side_panels.outline_width, 240.0);
        assert_eq!(p.side_panels.helpers_width, 260.0);
        // Unknown fields and a missing field are tolerated.
        let p = Prefs::load(Some(r#"{"future": true, "bottom_open": false}"#));
        assert!(!p.bottom_open);
        assert!(p.left_open);
    }

    #[test]
    fn closing_a_tab_keeps_the_selection_sensible() {
        let mut app = app_with_tabs(3);
        app.session.set_active(Some(1));
        app.request_close(1);
        assert_eq!((app.tabs().len(), app.active()), (2, Some(0)), "the left neighbour becomes active");
        app.request_close(1);
        assert_eq!(app.active(), Some(0));
        app.request_close(0);
        assert_eq!((app.tabs().len(), app.active()), (0, None), "the home view when the last tab goes");
    }

    #[test]
    fn closing_before_the_active_tab_follows_it() {
        let mut app = app_with_tabs(3);
        app.session.set_active(Some(2));
        app.request_close(0);
        assert_eq!(app.active(), Some(1));
        assert_eq!(app.tabs()[1].name, "2.xml");
        app.session.set_active(None);
        app.request_close(0);
        assert_eq!(app.active(), None, "the home view stays");
    }

    #[test]
    fn closing_a_tab_that_does_not_exist_is_harmless() {
        let mut app = app_with_tabs(1);
        app.request_close(5);
        app.request_close(usize::MAX);
        assert_eq!(app.tabs().len(), 1);
        assert_eq!(XmlCraftApp::with_prefs(Prefs::default()).tabs().len(), 0);
    }

    #[test]
    fn a_tab_with_unsaved_changes_asks_before_it_closes() {
        let mut app = app_with_tabs(2);
        app.session.document_mut(1).expect("there").editor_mut().type_text("x");
        assert!(app.has_unsaved_changes());
        app.request_close(1);
        assert_eq!(app.tabs().len(), 2, "still open: the question comes first");
        assert!(matches!(app.dialog, Some(Dialog::Unsaved { .. })));
    }

    #[test]
    fn the_control_file_is_removed_once_and_only_by_the_app_that_owns_it() {
        let dir = std::env::temp_dir().join(format!("xmlcraft-control-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("control.json");
        std::fs::write(&file, "{}").expect("file");
        let mut app = XmlCraftApp::with_prefs(Prefs::default());
        app.release_control_file();
        assert!(file.exists(), "an app with no control file removes nothing");
        app.set_control_file(file.clone());
        app.release_control_file();
        assert!(!file.exists());
        app.release_control_file();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unsaved_changes_are_noticed() {
        let mut app = app_with_tabs(2);
        assert!(!app.has_unsaved_changes());
        app.session.document_mut(1).expect("there").editor_mut().type_text("x");
        assert!(app.has_unsaved_changes());
    }
}
