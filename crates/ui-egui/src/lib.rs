#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! The egui shell of XmlCraft: the window chrome, panes and theme, with the look of PdfCraft.

mod about;
mod after_analysis;
mod assign;
mod auto_validate;
mod business_chip;
mod chrome;
mod commands;
mod compare_dialog;
mod completion;
mod control;
mod convert_dialog;
mod data_assist;
mod dialog;
mod documents;
pub mod editor_view;
mod entry_helpers;
mod find_bar;
mod generate;
mod grid_view;
mod header;
mod home;
mod http_pane;
mod i18n;
mod icon_data;
mod icons;
mod kit;
mod load_schema;
mod merge_dialog;
mod motion;
mod new_from_template;
mod openapi_dialog;
mod palette;
mod pickers;
mod preferences;
mod problems;
mod proxy_dialog;
mod quick_fix;
mod recovery;
mod side_panels;
mod soap_dialog;
mod status_pickers;
mod structure;
pub mod theme;
pub use control::{ControlClient, Reply as ControlReply};
mod tools;
mod widgets;
mod workbench;
pub use workbench::{Panel, Place};
mod xpath_dialog;
mod xslt_dialog;
pub mod zoom;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use xmlcraft_engine::Session;
use xmlcraft_engine::analysis::Analyzer;
use xmlcraft_engine::schema::Schemas;
use xmlcraft_engine::validation::{ResolverFactory, Validator};
use xmlcraft_engine::xsd_definition::DefinitionFinder;
pub use xmlcraft_platform::network::Network;

use crate::dialog::Dialog;
use crate::documents::DocView;
use crate::i18n::tl;
use crate::pickers::Pickers;
use crate::theme::{ThemeKind, ThemePreference};
use crate::workbench::Workbench;

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
    /// Where the panels are, and which are shown. Read by [`Workbench::load`] (which also migrates the pane switches of 0.2.1), not by
    /// serde: a damaged layout must not cost the other preferences.
    #[serde(skip_deserializing)]
    workspace: craft_ui::docking::Workspace<workbench::Panel>,
    /// The files opened or saved lately, newest first (paths that are not valid text are not kept).
    recent: Vec<String>,
    /// Entry Helpers write the content an element requires with it.
    entry_mandatory: bool,
    /// A document that names a grammar is validated in the background when it has been left alone for a moment.
    validate_typing: bool,
    /// Nothing is downloaded (XML ▸ Work Offline): the schemas and DTDs a document names are taken from what is open, the catalogs, the
    /// bundle, files and the copies downloaded before.
    work_offline: bool,
    /// How much larger or smaller than normal the editor's text is (View ▸ Zoom In, Zoom Out, Actual Size).
    editor_zoom: f32,
    /// OASIS catalogs the person named (paths), consulted before the schemas that ship with the app.
    user_catalogs: Vec<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            workspace: Workbench::default().workspace().clone(),
            recent: Vec::new(),
            entry_mandatory: true,
            validate_typing: true,
            work_offline: false,
            editor_zoom: 1.0,
            user_catalogs: Vec::new(),
        }
    }
}

impl Prefs {
    /// Read saved preferences; anything unreadable gives the defaults rather than an error.
    fn load(saved: Option<&str>) -> Self {
        let value: serde_json::Value = saved.and_then(|s| serde_json::from_str(s).ok()).unwrap_or_default();
        let mut prefs: Self = Self::deserialize(&value).unwrap_or_default();
        prefs.workspace = Workbench::load(&value).workspace().clone();
        prefs.editor_zoom = zoom::sanitized(prefs.editor_zoom);
        prefs
    }
}

pub struct XmlCraftApp {
    pub theme_preference: ThemePreference,
    pub mode: Mode,
    /// The editor and the panels around it: where each is, and which are shown.
    workbench: Workbench,
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
    /// Finds where an XML Schema name is defined (F12), and the versions of the open documents when it was asked.
    definition: DefinitionFinder,
    definition_versions: Vec<(String, u64)>,
    /// Expands the XIncludes of a document in the background (the resources may be downloaded).
    xinclude: xmlcraft_engine::xinclude::XIncludeExpander,
    /// Make schemas and sample documents in the background, when the Generate dialogs ask.
    schema_generator: xmlcraft_engine::generate::SchemaGenerator,
    sample_generator: xmlcraft_engine::generate::SampleGenerator,
    /// Learns a JSON Schema from a data document in the background (Generate JSON Schema…), and the request it waits for.
    json_schema_generator: xmlcraft_engine::json_infer::JsonSchemaGenerator,
    json_schema_waiting: Option<u64>,
    /// Makes a sample from a JSON Schema in the background (Generate Sample JSON…), and the request it waits for.
    json_sample_generator: xmlcraft_engine::json_sample::JsonSampleGenerator,
    json_sample_waiting: Option<u64>,
    /// Compares the documents of the Compare window.
    comparer: xmlcraft_engine::compare::Comparer,
    /// Evaluates the expressions of the XPath window.
    xpath: xmlcraft_engine::xpath::XPathRunner,
    /// Applies the stylesheets of the Transform window.
    xslt: xmlcraft_engine::xslt::XsltRunner,
    /// The expression last typed in the XPath window, put back when it is opened again.
    xpath_last: String,
    /// The Search tool window (Find in Files): its form and its last result.
    search: tools::search::SearchTool,
    /// Counts the requests the Generate dialogs make, so that an answer says which it is for.
    generate_serial: u64,
    /// Fetches the schema a document names (Load Schema, Schema Resolution), and the request being waited for.
    schema_fetcher: xmlcraft_engine::schema_fetch::SchemaFetcher,
    loading_schema: Option<u64>,
    resolution_serial: Option<u64>,
    /// The completion being offered at the caret, if any (see `completion.rs`).
    completion: Option<completion::Completion>,
    /// Completion was asked for (Edit ▸ Complete), not set off by typing: the popup opens where nothing has been typed too.
    completion_explicit: bool,
    /// Whether the editor had the keyboard at the last frame (the keys of a completion popup are the editor's only then).
    editor_has_focus: bool,
    /// Reads each document's DOCTYPE into a grammar in the background (what Entry Helpers offer comes from it).
    schemas: Schemas,
    /// What the threads of the SOAP and OpenAPI windows answer on.
    http: http_pane::HttpWorker,
    /// What the tab strip remembers: the tab last scrolled to and a tab being dragged.
    tab_strip: chrome::TabStripState,
    /// Autosave recovery: what is kept of unsaved documents, and what an earlier session left.
    recovery: recovery::Recovery,
    /// Loads the XML Schema a document names, for Entry Helpers.
    xsd_schemas: xmlcraft_engine::xsd_schemas::XsdSchemas,
    /// Loads the JSON Schema a JSON or YAML document names, for completion and hover.
    json_schemas: xmlcraft_engine::json_schemas::JsonSchemas,
    /// Entry Helpers write the content an element requires with it.
    entry_mandatory: bool,
    /// What the Entry Helpers filter field holds, the row chosen with the arrow keys (among those that can be chosen), and whether the
    /// keyboard is to go to the field at the next frame.
    entry_filter: String,
    entry_selected: usize,
    entry_focus: bool,
    /// Validate a document that names a grammar when it has been left alone for a moment (View: XML ▸ Validate While Typing).
    validate_typing: bool,
    /// The person's switch for downloading (XML ▸ Work Offline); the network says whether anything is downloaded at all.
    work_offline: bool,
    /// The editor's text size relative to normal (View ▸ Zoom In, Zoom Out, Actual Size; see [`zoom`]).
    editor_zoom: f32,
    /// The catalogs the person named, as paths (Preferences ▸ Schemas).
    user_catalogs: Vec<String>,
    /// What the grammars a document names are downloaded with.
    network: Network,
    /// How long a text is left alone before that.
    validate_delay: std::time::Duration,
    /// The text typed into the Outline's filter box.
    outline_filter: String,
    /// Which problems the Problems panel lists, and the one last gone to (where it starts in the text).
    problem_filter: problems::Filter,
    /// (the document's id, where in its text); a switch of tab leaves the other document's marker alone.
    problem_selected: Option<(u64, usize)>,
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
    /// The colours currently installed in egui, so they are applied only when they change.
    applied: Option<ThemeKind>,
}

impl XmlCraftApp {
    /// An app that downloads nothing: what tests make. The program makes it with [`XmlCraftApp::with_network`].
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        Self::with_network(cc, Network::offline())
    }

    /// An app whose schemas and DTDs are downloaded with `network` (unless the person works offline).
    pub fn with_network(cc: &eframe::CreationContext<'_>, network: Network) -> Self {
        let saved = cc.storage.and_then(|s| s.get_string(STORAGE_KEY));
        Self::with_saved_preferences(cc, network, saved.as_deref())
    }

    /// An app made from preferences saved as [`eframe::App::save`] writes them (what a restart reads back), instead of eframe's storage.
    pub fn with_saved_preferences(cc: &eframe::CreationContext<'_>, network: Network, saved: Option<&str>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        theme::install_fonts(&cc.egui_ctx);
        let prefs = Prefs::load(saved);
        let mut app = Self::with_prefs_and_network(prefs, network);
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
        app.definition.on_done({
            let repaint = repaint.clone();
            move || repaint.request_repaint()
        });
        app.xinclude.on_done({
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
        app.json_schema_generator.on_done({
            let repaint = repaint.clone();
            move || repaint.request_repaint()
        });
        app.json_sample_generator.on_done({
            let repaint = repaint.clone();
            move || repaint.request_repaint()
        });
        app.comparer.on_done({
            let repaint = repaint.clone();
            move || repaint.request_repaint()
        });
        app.xpath.on_done({
            let repaint = repaint.clone();
            move || repaint.request_repaint()
        });
        app.xslt.on_done({
            let repaint = repaint.clone();
            move || repaint.request_repaint()
        });
        app.schema_fetcher.on_done({
            let repaint = repaint.clone();
            move || repaint.request_repaint()
        });
        app.schemas.on_done({
            let repaint = repaint.clone();
            move || repaint.request_repaint()
        });
        app.xsd_schemas.on_done({
            let repaint = repaint.clone();
            move || repaint.request_repaint()
        });
        app.json_schemas.on_done(move || repaint.request_repaint());
        app
    }

    #[cfg(test)]
    fn with_prefs(prefs: Prefs) -> Self {
        Self::with_prefs_and_network(prefs, Network::offline())
    }

    fn with_prefs_and_network(prefs: Prefs, network: Network) -> Self {
        network.set_offline(prefs.work_offline);
        network.set_user_catalogs(prefs.user_catalogs.clone());
        // The resolvers of the background workers are made when a request is: they see the switch as it is then.
        let resolvers = || -> ResolverFactory {
            let network = network.clone();
            std::sync::Arc::new(move |buffers| network.resolver(buffers))
        };
        let mut session = Session::new();
        session.set_recent(xmlcraft_engine::Recent::from_paths(prefs.recent.iter().map(std::path::PathBuf::from)));
        Self {
            theme_preference: prefs.theme,
            mode: Mode::default(),
            workbench: Workbench::from_workspace(prefs.workspace),
            palette_open: false,
            palette_query: String::new(),
            palette_last_query: String::new(),
            palette_selected: 0,
            session,
            analyzer: Analyzer::new(),
            // The external parts of a DTD, and the documents of a schema, are open documents, the DTDs and schemas that ship with the app
            // (found by their public identifiers and namespaces), files the person can read and, unless they work offline, what the
            // web has (kept on disk).
            validator: Validator::new(resolvers()),
            definition: DefinitionFinder::new(resolvers()),
            definition_versions: Vec::new(),
            xinclude: xmlcraft_engine::xinclude::XIncludeExpander::new(resolvers()),
            schema_generator: xmlcraft_engine::generate::SchemaGenerator::new(),
            sample_generator: xmlcraft_engine::generate::SampleGenerator::new(resolvers()),
            json_schema_generator: xmlcraft_engine::json_infer::JsonSchemaGenerator::new(),
            json_schema_waiting: None,
            json_sample_generator: xmlcraft_engine::json_sample::JsonSampleGenerator::new(resolvers()),
            json_sample_waiting: None,
            comparer: xmlcraft_engine::compare::Comparer::new(),
            xpath: xmlcraft_engine::xpath::XPathRunner::new(),
            xslt: xmlcraft_engine::xslt::XsltRunner::new(),
            xpath_last: String::new(),
            search: tools::search::SearchTool::default(),
            generate_serial: 0,
            schema_fetcher: xmlcraft_engine::schema_fetch::SchemaFetcher::new(resolvers()),
            loading_schema: None,
            resolution_serial: None,
            completion: None,
            completion_explicit: false,
            editor_has_focus: false,
            outline_filter: String::new(),
            problem_filter: problems::Filter::All,
            problem_selected: None,
            outline_rows: None,
            find: find_bar::FindBar::default(),
            goto_input: String::new(),
            goto_focus: false,
            check_pending: None,
            validate_pending: None,
            schemas: Schemas::new(resolvers()),
            http: http_pane::HttpWorker::default(),
            recovery: recovery::Recovery::default(),
            tab_strip: chrome::TabStripState::default(),
            xsd_schemas: xmlcraft_engine::xsd_schemas::XsdSchemas::new(resolvers()),
            json_schemas: xmlcraft_engine::json_schemas::JsonSchemas::new(resolvers()),
            entry_mandatory: prefs.entry_mandatory,
            entry_filter: String::new(),
            entry_selected: 0,
            entry_focus: false,
            validate_typing: prefs.validate_typing,
            work_offline: prefs.work_offline,
            editor_zoom: zoom::sanitized(prefs.editor_zoom),
            user_catalogs: prefs.user_catalogs.clone(),
            network,
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
            applied: None,
        }
    }

    fn prefs(&self) -> Prefs {
        Prefs {
            theme: self.theme_preference,
            workspace: self.workbench.workspace().clone(),
            recent: self.session.recent().paths().iter().filter_map(|path| path.to_str().map(str::to_owned)).collect(),
            entry_mandatory: self.entry_mandatory,
            validate_typing: self.validate_typing,
            work_offline: self.work_offline,
            editor_zoom: self.editor_zoom,
            user_catalogs: self.user_catalogs.clone(),
        }
    }

    /// The key under which [`eframe::App::save`] keeps the preferences.
    pub const STORAGE_KEY: &'static str = STORAGE_KEY;

    /// Whether a panel is shown.
    pub fn panel_visible(&self, panel: Panel) -> bool {
        self.workbench.is_visible(panel)
    }

    /// Show or hide a panel (the editor stays); what went wrong goes to the status bar.
    pub fn set_panel_visible(&mut self, panel: Panel, visible: bool) {
        if let Err(e) = self.workbench.set_visible(panel, visible) {
            self.status =
                crate::i18n::fmt(tl!("{panel} cannot be shown or hidden: {reason}"), &[("panel", panel.title()), ("reason", &e.to_string())]);
        }
    }

    /// Where a panel is, `None` while it is hidden.
    pub fn panel_place(&self, panel: Panel) -> Option<Place> {
        self.workbench.place(panel)
    }

    /// Whether a hidden tool window has news (the rail's dot): a search that finished while Search was hidden.
    pub fn panel_attention(&self, panel: Panel) -> bool {
        self.workbench.attention(panel)
    }

    /// Change the arrangement of the panels, all or none, as a drag or a menu would.
    pub fn apply_layout(&mut self, actions: Vec<craft_ui::docking::Action<Panel>>) -> Result<(), String> {
        self.workbench.apply(actions).map(drop).map_err(|e| e.to_string())
    }

    /// Whether nothing is downloaded now: the person works offline, `XMLCRAFT_OFFLINE` is on, or this build has no client.
    pub(crate) fn network_is_offline(&self) -> bool {
        self.work_offline || self.network.is_offline()
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
        self.pump_compare();
        self.pump_xpath();
        self.pump_transform();
        self.pump_http();
        self.recovery_tick(ctx);
        self.pump_schema_fetch();
        for command in self.viewport_commands.drain(..) {
            ctx.send_viewport_cmd(command);
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.quit_confirmed || !self.has_unsaved_changes() {
                self.recovery_finish();
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
        chrome::command_bar(self, ui);
        // The status bar is outermost at the bottom, the problems panel sits above it; the rail is outermost on the left.
        chrome::status_bar(self, ui);
        // The palette before the editor: its keys are its own while it is open.
        palette::show(self, ui.ctx());
        // The rail is always there (on the home view with the document's tool windows disabled).
        chrome::activity_rail(self, ui);
        // The editor (with the document header, or the home view) and the panels around it, in the docking engine.
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.panel)).show(ui, |ui| workbench::show(self, ui));
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
        let app = XmlCraftApp::with_prefs(p.clone());
        assert!(Panel::ALL.iter().all(|panel| app.panel_visible(*panel) == panel.spec().default_visible));
        assert!(!app.panel_visible(Panel::Search), "Search waits for a search");
        assert_eq!(p.theme, ThemePreference::System);
    }

    #[test]
    fn preferences_round_trip() {
        let p = Prefs { theme: ThemePreference::Dark, ..Prefs::default() };
        let mut app = XmlCraftApp::with_prefs(p);
        app.set_panel_visible(Panel::Outline, false);
        app.apply_layout(vec![craft_ui::docking::Action::MovePanel {
            panel: Panel::EntryHelpers,
            to: craft_ui::docking::Target::NewDock { side: craft_ui::docking::Side::End },
        }])
        .expect("moves");
        let p = app.prefs();
        let json = serde_json::to_string(&p).expect("serialises");
        assert_eq!(Prefs::load(Some(&json)), p);
        let again = XmlCraftApp::with_prefs(Prefs::load(Some(&json)));
        assert!(!again.panel_visible(Panel::Outline));
        assert_eq!(again.panel_place(Panel::EntryHelpers), Some(Place::Right));
    }

    #[test]
    fn the_catalogs_the_person_named_are_remembered_and_given_to_the_network() {
        assert!(Prefs::default().user_catalogs.is_empty());
        assert!(Prefs::load(Some(r#"{"left_open": false}"#)).user_catalogs.is_empty(), "an old file that does not say has none");
        let named = Prefs { user_catalogs: vec!["/x/a.xml".into(), "/x/b.xml".into()], ..Prefs::default() };
        let again = Prefs::load(Some(&serde_json::to_string(&named).expect("serialises")));
        assert_eq!(again.user_catalogs, ["/x/a.xml", "/x/b.xml"]);
        let app = XmlCraftApp::with_prefs(again);
        assert_eq!(app.prefs().user_catalogs, ["/x/a.xml", "/x/b.xml"], "it is what the app saves, too");
        assert_eq!(app.network.user_catalogs().len(), 2, "and what its resolvers use");
        let mut app = app;
        app.add_user_catalog("/x/c.xml");
        assert_eq!(app.network.user_catalogs().len(), 3, "a catalog added later is used at once");
        app.remove_user_catalog(0);
        assert_eq!(app.network.user_catalogs().len(), 2, "and one removed is not");
    }

    #[test]
    fn the_zoom_is_remembered_and_one_in_the_file_is_kept_in_range() {
        assert_eq!(Prefs::default().editor_zoom, 1.0);
        assert_eq!(Prefs::load(Some(r#"{"left_open": false}"#)).editor_zoom, 1.0, "an old file that does not say");
        let zoomed = Prefs { editor_zoom: 1.5, ..Prefs::default() };
        let again = Prefs::load(Some(&serde_json::to_string(&zoomed).expect("serialises")));
        assert!((again.editor_zoom - 1.5).abs() < 1e-4);
        assert!((XmlCraftApp::with_prefs(again).prefs().editor_zoom - 1.5).abs() < 1e-4, "it is what the app saves, too");
        assert_eq!(Prefs::load(Some(r#"{"editor_zoom": 400.0}"#)).editor_zoom, zoom::MAX);
        assert_eq!(Prefs::load(Some(r#"{"editor_zoom": -1.0}"#)).editor_zoom, zoom::MIN);
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
    fn working_offline_is_saved_with_the_preferences_and_is_off_unless_it_was_turned_on() {
        assert!(!Prefs::default().work_offline);
        assert!(!Prefs::load(Some(r#"{"left_open": false}"#)).work_offline, "an old file that does not say is online");
        assert!(!Prefs::load(Some("not json")).work_offline);
        let on = Prefs { work_offline: true, ..Prefs::default() };
        let again = Prefs::load(Some(&serde_json::to_string(&on).expect("serializes")));
        assert!(again.work_offline);
        let app = XmlCraftApp::with_prefs(again);
        assert_eq!(app.command_checked("xml.work_offline"), Some(true));
        assert!(app.prefs().work_offline, "it is what the app saves, too");
        assert_eq!(XmlCraftApp::with_prefs(Prefs::default()).command_checked("xml.work_offline"), Some(false));
    }

    #[test]
    fn unreadable_preferences_give_the_defaults() {
        for saved in [None, Some(""), Some("not json"), Some("[1,2]"), Some("{\"theme\": 7}"), Some("null")] {
            assert_eq!(Prefs::load(saved), Prefs::default(), "{saved:?}");
        }
    }

    #[test]
    fn hostile_preference_values_are_repaired() {
        // Sizes of 0.2.1 out of range are clamped to what its panes allowed.
        let p = Prefs::load(Some(r#"{"side_panels": {"messages_height": 1e30, "outline_width": -5.0, "helpers_width": 0.0}}"#));
        assert!(p.workspace.validate().is_ok());
        // Unknown fields and a missing field are tolerated.
        let app = XmlCraftApp::with_prefs(Prefs::load(Some(r#"{"future": true, "bottom_open": false}"#)));
        assert!(!app.panel_visible(Panel::Problems));
        assert!(app.panel_visible(Panel::Outline));
        // A damaged layout costs only the layout.
        let p = Prefs::load(Some(r#"{"theme": "Dark", "workspace": {"layout": 5}}"#));
        assert_eq!(p.theme, ThemePreference::Dark);
        assert_eq!(p.workspace, Prefs::default().workspace);
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
