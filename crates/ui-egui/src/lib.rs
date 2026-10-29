#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! The egui shell of XmlCraft: the window chrome, panes and theme, with the look of PdfCraft.

mod about;
mod after_analysis;
mod assign;
mod auto_validate;
mod chrome;
mod commands;
mod control;
mod dialog;
mod documents;
pub mod editor_view;
mod entry_helpers;
mod find_bar;
mod generate;
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
use xmlcraft_engine::analysis::Analyzer;
use xmlcraft_engine::schema::Schemas;
use xmlcraft_engine::validation::Validator;

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
    /// The files opened or saved lately, newest first (paths that are not valid text are not kept).
    recent: Vec<String>,
    /// Entry Helpers write the content an element requires with it.
    entry_mandatory: bool,
    /// A document that names a grammar is validated in the background when it has been left alone for a moment.
    validate_typing: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            left_open: true,
            right: Some(RightPanel::EntryHelpers),
            bottom_open: true,
            side_panels: SidePanels::default(),
            recent: Vec::new(),
            entry_mandatory: true,
            validate_typing: true,
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
    /// Works out each document's problems and outline in the background.
    analyzer: Analyzer,
    /// Checks each document against its DTD in the background, when asked (F8).
    validator: Validator,
    /// Make schemas and sample documents in the background, when the Generate dialogs ask.
    schema_generator: xmlcraft_engine::generate::SchemaGenerator,
    sample_generator: xmlcraft_engine::generate::SampleGenerator,
    /// Counts the requests the Generate dialogs make, so that an answer says which it is for.
    generate_serial: u64,
    /// Reads each document's DOCTYPE into a grammar in the background (what Entry Helpers offer comes from it).
    schemas: Schemas,
    /// Loads the XML Schema a document names, for Entry Helpers.
    xsd_schemas: xmlcraft_engine::xsd_schemas::XsdSchemas,
    /// Entry Helpers write the content an element requires with it.
    entry_mandatory: bool,
    /// Validate a document that names a grammar when it has been left alone for a moment (View: XML ▸ Validate While Typing).
    validate_typing: bool,
    /// How long a text is left alone before that.
    validate_delay: std::time::Duration,
    /// The text typed into the Outline's filter box.
    outline_filter: String,
    /// The Outline's rows for the filter and analysis they were worked out for, so a frame does not filter 50,000 names again.
    outline_rows: Option<side_panels::OutlineRows>,
    /// The find and replace bar.
    find: find_bar::FindBar,
    /// The text typed into Go to Line, and whether its field is still to be given the keyboard (once, when the dialog opens).
    goto_input: String,
    goto_focus: bool,
    /// A check (F7) is waiting for the analysis of this version of this document, to report its result.
    check_pending: Option<(u64, u64)>,
    /// A validation (F8) is waiting for the result for this version of this document, to report it.
    validate_pending: Option<(u64, u64)>,
    /// A command (the jump to the matching tag, folding) that is waiting for the analysis of the text it was asked in.
    waiting: Option<after_analysis::Waiting>,
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
        let repaint = cc.egui_ctx.clone();
        app.analyzer.on_done({
            let repaint = repaint.clone();
            move || repaint.request_repaint()
        });
        app.validator.on_done({
            let repaint = repaint.clone();
            move || repaint.request_repaint()
        });
        app.schema_generator.on_done({
            let repaint = repaint.clone();
            move || repaint.request_repaint()
        });
        app.sample_generator.on_done({
            let repaint = repaint.clone();
            move || repaint.request_repaint()
        });
        app.schemas.on_done({
            let repaint = repaint.clone();
            move || repaint.request_repaint()
        });
        app.xsd_schemas.on_done(move || repaint.request_repaint());
        app
    }

    fn with_prefs(prefs: Prefs) -> Self {
        let mut session = Session::new();
        session.set_recent(xmlcraft_engine::Recent::from_paths(prefs.recent.iter().map(std::path::PathBuf::from)));
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
            session,
            analyzer: Analyzer::new(),
            // The external parts of a DTD, and the documents of a schema, are open documents, the schemas that ship with the app and files the
            // person can read; nothing is downloaded.
            validator: Validator::new(
                std::sync::Arc::new(|| Box::new(xmlcraft_platform::subset::FileSubsetLoader::unconfined())),
                std::sync::Arc::new(xmlcraft_platform::schema_files::local_resolver),
            ),
            schema_generator: xmlcraft_engine::generate::SchemaGenerator::new(),
            sample_generator: xmlcraft_engine::generate::SampleGenerator::new(
                std::sync::Arc::new(|| Box::new(xmlcraft_platform::subset::FileSubsetLoader::unconfined())),
                std::sync::Arc::new(xmlcraft_platform::schema_files::local_resolver),
            ),
            generate_serial: 0,
            outline_filter: String::new(),
            outline_rows: None,
            find: find_bar::FindBar::default(),
            goto_input: String::new(),
            goto_focus: false,
            check_pending: None,
            validate_pending: None,
            schemas: Schemas::new(std::sync::Arc::new(|| Box::new(xmlcraft_platform::subset::FileSubsetLoader::unconfined()))),
            xsd_schemas: xmlcraft_engine::xsd_schemas::XsdSchemas::new(std::sync::Arc::new(xmlcraft_platform::schema_files::local_resolver)),
            entry_mandatory: prefs.entry_mandatory,
            validate_typing: prefs.validate_typing,
            validate_delay: auto_validate::DELAY,
            waiting: None,
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
            recent: self.session.recent().paths().iter().filter_map(|path| path.to_str().map(str::to_owned)).collect(),
            entry_mandatory: self.entry_mandatory,
            validate_typing: self.validate_typing,
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
        // After everything above that can change a text, so that this frame asks for the analysis of the text it will draw.
        self.pump_analysis();
        self.pump_generate();
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

    #[test]
    fn the_recent_files_are_saved_with_the_preferences_and_read_back() {
        let prefs = Prefs { recent: vec!["/x/a.xml".into(), "/x/b.xml".into(), "/x/a.xml".into()], ..Prefs::default() };
        let app = XmlCraftApp::with_prefs(prefs);
        // Read back without the repeat, newest first.
        assert_eq!(app.recent_files(), [std::path::PathBuf::from("/x/a.xml"), "/x/b.xml".into()]);
        let saved = serde_json::to_string(&app.prefs()).expect("serialises");
        let again = XmlCraftApp::with_prefs(Prefs::load(Some(&saved)));
        assert_eq!(again.recent_files(), app.recent_files());
        // Preferences from before there were recent files, and unreadable ones, give an empty list.
        assert!(XmlCraftApp::with_prefs(Prefs::load(Some(r#"{"left_open":false}"#))).recent_files().is_empty());
        assert!(XmlCraftApp::with_prefs(Prefs::load(Some("not json"))).recent_files().is_empty());
        assert!(XmlCraftApp::with_prefs(Prefs::load(Some(r#"{"recent":"nope"}"#))).recent_files().is_empty());
    }

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
    fn validating_while_typing_is_on_unless_it_was_turned_off_and_it_is_remembered() {
        assert!(Prefs::default().validate_typing);
        assert!(Prefs::load(Some(r#"{"left_open": false}"#)).validate_typing, "an old file that does not say is on");
        let off = Prefs { validate_typing: false, ..Prefs::default() };
        let again = Prefs::load(Some(&serde_json::to_string(&off).expect("serialises")));
        assert!(!again.validate_typing);
        assert!(!XmlCraftApp::with_prefs(again).prefs().validate_typing, "it is what the app saves, too");
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
