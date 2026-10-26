#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! The egui shell of XmlCraft: the window chrome, panes and theme, with the look of PdfCraft.

mod chrome;
mod commands;
mod home;
mod i18n;
mod icon_data;
mod icons;
mod motion;
mod palette;
mod side_panels;
pub mod theme;
mod widgets;

use serde::{Deserialize, Serialize};

use xmlcraft_engine::Session;

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

/// A document tab as the strip shows it. The engine's documents replace this when they exist (milestone M1.12).
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
    /// Open documents, as tabs. `None` in `active` is the home view.
    pub tabs: Vec<TabInfo>,
    pub active: Option<usize>,
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
    /// Commands for the window that a command asked for; sent on the next frame.
    viewport_commands: Vec<egui::ViewportCommand>,
    /// What the status bar says instead of "Ready": the last command's trouble, if any.
    pub status: String,
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
        Self::with_prefs(prefs)
    }

    fn with_prefs(prefs: Prefs) -> Self {
        Self {
            theme_preference: prefs.theme,
            tabs: Vec::new(),
            active: None,
            mode: Mode::default(),
            left_open: prefs.left_open,
            right: prefs.right,
            bottom_open: prefs.bottom_open,
            palette_open: false,
            palette_query: String::new(),
            palette_last_query: String::new(),
            palette_selected: 0,
            session: Session::new(),
            viewport_commands: Vec::new(),
            status: String::new(),
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

    /// Close the tab at `index`; the neighbour to its left (or the home view) becomes active.
    pub fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        self.tabs.remove(index);
        self.active = match self.active {
            Some(a) if a == index => index.checked_sub(1).filter(|_| !self.tabs.is_empty()).or((!self.tabs.is_empty()).then_some(0)),
            Some(a) if a > index => Some(a - 1),
            other => other,
        };
        if self.tabs.is_empty() {
            self.active = None;
        }
    }

    /// Install the colours for the current preference and system setting, when they changed.
    fn sync_theme(&mut self, ctx: &egui::Context) {
        let system = ctx.system_theme().map(|t| t == egui::Theme::Dark);
        let kind = self.theme_preference.resolve(system, ThemeKind::Light);
        if self.applied != Some(kind) {
            theme::apply(ctx, kind);
            self.applied = Some(kind);
        }
    }
}

impl eframe::App for XmlCraftApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sync_theme(ctx);
        self.handle_shortcuts(ctx);
        for command in self.viewport_commands.drain(..) {
            ctx.send_viewport_cmd(command);
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
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.pasteboard)).show(ui, |ui| {
            if self.active.is_none() {
                home::show(ui);
            }
        });
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
        app.tabs = (0..n).map(|i| TabInfo { name: format!("{i}.xml"), dirty: false }).collect();
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
        app.active = Some(1);
        app.close_tab(1);
        assert_eq!((app.tabs.len(), app.active), (2, Some(0)), "the left neighbour becomes active");
        app.close_tab(1);
        assert_eq!(app.active, Some(0));
        app.close_tab(0);
        assert_eq!((app.tabs.len(), app.active), (0, None), "the home view when the last tab goes");
    }

    #[test]
    fn closing_before_the_active_tab_follows_it() {
        let mut app = app_with_tabs(3);
        app.active = Some(2);
        app.close_tab(0);
        assert_eq!(app.active, Some(1));
        assert_eq!(app.tabs[1].name, "2.xml");
        app.active = None;
        app.close_tab(0);
        assert_eq!(app.active, None, "the home view stays");
    }

    #[test]
    fn closing_a_tab_that_does_not_exist_is_harmless() {
        let mut app = app_with_tabs(1);
        app.close_tab(5);
        app.close_tab(usize::MAX);
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(XmlCraftApp::with_prefs(Prefs::default()).tabs.len(), 0);
    }
}
