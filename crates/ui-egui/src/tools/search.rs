//! Search, the tool window of Find in Files: a text or a regular expression looked for in the files under a folder
//! (`xmlcraft_platform::search`, on a thread), the hits listed by file, and a click on a hit opening the file at that line while the
//! panel stays where it is. Files are only read. The form and the last result live in the app, so they are there whenever the panel
//! is shown again.

use std::path::{Path, PathBuf};

use xmlcraft_editor::find::Query;
use xmlcraft_platform::search::{Found, Options, search_folder};

use crate::http_pane::Outcome;
use crate::i18n::{fmt, tl};
use crate::kit::{self, Tone};
use crate::{Panel, XmlCraftApp, widgets};

/// The most rows (file names and hits) the list draws.
const LISTED: usize = 1_000;
/// The height of a row of the list.
const ROW: f32 = 22.0;
/// The longest selection that is taken as what to look for (in characters).
const SELECTION_PATTERN: usize = 200;

/// The form of the Search tool window and what it found last.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SearchTool {
    folder: String,
    pattern: String,
    glob: String,
    regex: bool,
    case_sensitive: bool,
    whole_word: bool,
    /// The search being waited for.
    waiting: Option<u64>,
    outcome: Option<Result<Found, String>>,
    /// Give the keyboard to the pattern field at the next frame.
    focus: bool,
}

impl Default for SearchTool {
    fn default() -> Self {
        Self {
            folder: String::new(),
            pattern: String::new(),
            glob: String::new(),
            regex: false,
            case_sensitive: true,
            whole_word: false,
            waiting: None,
            outcome: None,
            focus: false,
        }
    }
}

impl SearchTool {
    fn options(&self) -> Options {
        let mut options = Options::new(Query {
            pattern: self.pattern.clone(),
            regex: self.regex,
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
        });
        options.glob = self.glob.clone();
        options
    }

    /// Whether a search can start: there is something to look for and somewhere to look, and none is running.
    fn can_run(&self) -> bool {
        self.waiting.is_none() && !self.pattern.is_empty() && !self.folder.trim().is_empty()
    }
}

/// What a click did: open this file at this line and column.
type Jump = (PathBuf, usize, usize);

impl XmlCraftApp {
    /// Edit ▸ Find in Files: show the Search panel, put the keyboard in its pattern field, and take a selection on one line as what
    /// to look for. The folder is the shown document's, else the working directory, unless one was typed before.
    pub(crate) fn open_search(&mut self) {
        if cfg!(target_arch = "wasm32") {
            self.status = tl!("Find in Files needs the desktop app: the browser cannot read folders.").to_owned();
            return;
        }
        if self.search.folder.is_empty() {
            self.search.folder =
                self.document_folder().or_else(|| std::env::current_dir().ok()).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        }
        if let Some(doc) = self.session.active_document() {
            let selection = doc.editor().primary();
            if !selection.is_empty()
                && let Ok(text) = doc.editor().buffer().slice(selection.start()..selection.end())
                && !text.contains(['\n', '\r'])
                && text.chars().count() <= SELECTION_PATTERN
            {
                self.search.pattern = text.to_string();
                self.search.regex = false;
            }
        }
        if let Err(e) = self.workbench.reveal(Panel::Search) {
            self.status = fmt(tl!("{panel} cannot be shown: {reason}"), &[("panel", Panel::Search.title()), ("reason", &e.to_string())]);
            return;
        }
        self.search.focus = true;
    }

    /// The folder of the document being shown, when it is a file.
    fn document_folder(&self) -> Option<PathBuf> {
        self.session.active_document().and_then(|d| d.path()).and_then(Path::parent).map(Path::to_path_buf)
    }

    fn run_search(&mut self) {
        self.generate_serial += 1;
        let serial = self.generate_serial;
        let (folder, options) = (PathBuf::from(self.search.folder.trim()), self.search.options());
        match self.spawn_http(move || Outcome::Files { serial, result: search_folder(&folder, &options).map_err(|e| e.to_string()) }) {
            Ok(()) => {
                self.search.waiting = Some(serial);
                self.search.outcome = None;
            }
            Err(why) => self.search.outcome = Some(Err(why)),
        }
    }

    /// The answer of a search, whatever else is open; a hidden Search panel gets the rail's attention dot.
    pub(crate) fn search_answered(&mut self, serial: u64, result: Result<Found, String>) {
        if self.search.waiting != Some(serial) {
            return;
        }
        self.search.waiting = None;
        self.search.outcome = Some(result);
        self.workbench.set_attention(Panel::Search);
    }
}

/// The path as the list shows it: relative to the folder searched.
fn shown(path: &Path, folder: &str) -> String {
    path.strip_prefix(folder.trim()).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

/// The body of the Search panel.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    if cfg!(target_arch = "wasm32") {
        widgets::empty_state(ui, 12.0, tl!("Find in Files needs the desktop app"), tl!("The browser cannot read folders."));
        return;
    }
    let can = app.search.can_run();
    let mut run = kit::tool_footer(ui, tl!("Find"), Some("search"), can, tl!("Type what to find and a folder first."));
    let jump = kit::tool_body(ui, false, |ui| {
        run |= form(ui, &mut app.search);
        results(ui, &app.search)
    });
    if run && app.search.can_run() {
        app.run_search();
    }
    if let Some((path, line, column)) = jump
        && app.open_path(&path)
    {
        app.go_to_line(&format!("{line}:{column}"));
    }
}

/// A text field of the form, named for assistive technology. True when Enter was pressed in it.
fn field(ui: &mut egui::Ui, text: &mut String, hint: &str, name: &str, width: f32) -> (egui::Response, bool) {
    let response = ui.add(widgets::line(text).desired_width(width).hint_text(hint));
    widgets::name_for_assistive_technology(ui, &response, name);
    let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    (response, enter)
}

/// A small label above a field (narrow panels).
fn caption(ui: &mut egui::Ui, text: &str) {
    let t = crate::theme::Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(text).font(crate::theme::medium(11.5)).color(t.text_muted));
}

/// The three search options, as small toggles.
fn options(ui: &mut egui::Ui, tool: &mut SearchTool) {
    kit::mark_toggle(ui, &mut tool.regex, ".*", tl!("Regular expression"));
    kit::mark_toggle(ui, &mut tool.case_sensitive, "Aa", tl!("Match case"));
    kit::mark_toggle(ui, &mut tool.whole_word, "W", tl!("Whole word"));
}

/// A panel at least this wide has the whole form on one row.
const ONE_ROW: f32 = 760.0;
/// The width the three option toggles take, with their gaps.
const OPTIONS_WIDTH: f32 = 3.0 * 34.0;

/// The form: in a wide panel on one row (the pattern, its options, the folder and the file names), in a middling one on two (the
/// pattern and its options, then the folder and the file names); in a narrow one stacked, labels above the fields. True when Enter
/// was pressed in a field.
fn form(ui: &mut egui::Ui, tool: &mut SearchTool) -> bool {
    let narrow = kit::narrow(ui);
    let one_row = ui.available_width() >= ONE_ROW;
    let mut submit = false;
    let skipped =
        tl!("Folders that start with a dot, node_modules and target are skipped; links are not followed; files that are not text are skipped.");
    let (pattern_hint, folder_hint) = (tl!("Text or regular expression"), tl!("Folder"));
    let glob_hint = tl!("File names: *.xml;*.xsd (empty: all files)");
    let folder_and_glob = |ui: &mut egui::Ui, tool: &mut SearchTool, room: f32| {
        let (folder, enter) = field(ui, &mut tool.folder, folder_hint, tl!("The folder to search"), room * 0.6);
        folder.on_hover_text(skipped);
        let (_, enter_glob) = field(ui, &mut tool.glob, glob_hint, tl!("The names of the files to search"), room * 0.4);
        enter || enter_glob
    };
    let pattern = if narrow {
        caption(ui, tl!("Find what"));
        let (response, enter) = field(ui, &mut tool.pattern, pattern_hint, tl!("What to find"), ui.available_width());
        submit |= enter;
        ui.horizontal(|ui| options(ui, tool));
        caption(ui, tl!("Folder"));
        let (folder, enter) = field(ui, &mut tool.folder, folder_hint, tl!("The folder to search"), ui.available_width());
        folder.on_hover_text(skipped);
        submit |= enter;
        caption(ui, tl!("File names"));
        let (_, enter) = field(ui, &mut tool.glob, glob_hint, tl!("The names of the files to search"), ui.available_width());
        submit |= enter;
        response
    } else if one_row {
        ui.horizontal(|ui| {
            let gap = ui.spacing().item_spacing.x;
            let room = (ui.available_width() - OPTIONS_WIDTH - 3.0 * gap).max(0.0);
            let (response, enter) = field(ui, &mut tool.pattern, pattern_hint, tl!("What to find"), room * 0.4);
            submit |= enter;
            options(ui, tool);
            submit |= folder_and_glob(ui, tool, room * 0.6);
            response
        })
        .inner
    } else {
        let response = ui
            .horizontal(|ui| {
                let width = (ui.available_width() - OPTIONS_WIDTH).max(80.0);
                let (response, enter) = field(ui, &mut tool.pattern, pattern_hint, tl!("What to find"), width);
                submit |= enter;
                options(ui, tool);
                response
            })
            .inner;
        ui.horizontal(|ui| {
            let room = (ui.available_width() - ui.spacing().item_spacing.x).max(0.0);
            submit |= folder_and_glob(ui, tool, room);
        });
        response
    };
    if std::mem::take(&mut tool.focus) {
        pattern.request_focus();
    }
    submit
}

/// What the last search found, filling the rest of the panel. Returns the hit clicked.
fn results(ui: &mut egui::Ui, tool: &SearchTool) -> Option<Jump> {
    ui.add_space(kit::GAP);
    if tool.waiting.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Searching…"));
        });
    }
    let found = match &tool.outcome {
        None => {
            if tool.waiting.is_none() {
                kit::help(ui, tl!("Type what to find and press Enter. The hits are listed here; a click opens the file at the line."));
            }
            return None;
        }
        Some(Err(message)) => {
            kit::banner(ui, Tone::Bad, message);
            return None;
        }
        Some(Ok(found)) => found,
    };
    let hits: usize = found.files.iter().map(|f| f.hits.len()).sum();
    let mut summary = fmt(
        tl!("{hits} hits in {files} files; {searched} files searched."),
        &[("hits", &hits.to_string()), ("files", &found.files.len().to_string()), ("searched", &found.searched.to_string())],
    );
    if found.skipped > 0 {
        summary.push(' ');
        summary.push_str(&fmt(tl!("{skipped} were skipped (not text, or not readable)."), &[("skipped", &found.skipped.to_string())]));
    }
    if found.truncated {
        summary.push(' ');
        summary.push_str(tl!("There is more: a limit cut the search short. Narrow it down."));
    }
    kit::help(ui, &summary);
    let t = crate::theme::Tokens::get(ui.ctx());
    let mut jump = None;
    let mut drawn = 0;
    egui::ScrollArea::vertical().id_salt("search-results").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        'files: for file in &found.files {
            if drawn >= LISTED {
                break;
            }
            let head = format!("{}  ({})", shown(&file.path, &tool.folder), file.hits.len());
            ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), ROW), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(egui::RichText::new(head).strong()).truncate());
            });
            drawn += 1;
            for hit in &file.hits {
                if drawn >= LISTED {
                    break 'files;
                }
                drawn += 1;
                let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), egui::Sense::click());
                let text = format!("{}:{}   {}", hit.line, hit.column, hit.text.trim());
                response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, text.clone()));
                if response.hovered() {
                    ui.painter().rect_filled(rect, 4.0, t.hover);
                }
                ui.painter().with_clip_rect(rect).text(
                    rect.left_center() + egui::vec2(14.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    &text,
                    egui::FontId::monospace(12.5),
                    t.text,
                );
                if crate::theme::hand(response).clicked() {
                    jump = Some((file.path.clone(), hit.line, hit.column));
                }
            }
        }
    });
    jump
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_shown_relative_to_the_folder_searched() {
        assert_eq!(shown(Path::new("/p/sub/a.xml"), "/p"), "sub/a.xml");
        assert_eq!(shown(Path::new("/q/a.xml"), "/p"), "/q/a.xml");
    }

    #[test]
    fn the_form_asks_for_what_was_typed() {
        let mut tool = SearchTool { folder: "/p".to_owned(), ..SearchTool::default() };
        assert!(!tool.can_run(), "nothing to look for yet");
        tool.pattern = "x".to_owned();
        tool.glob = "*.xml".to_owned();
        tool.whole_word = true;
        assert!(tool.can_run());
        let options = tool.options();
        assert_eq!(
            (options.query.pattern.as_str(), options.glob.as_str(), options.query.whole_word, options.query.regex),
            ("x", "*.xml", true, false)
        );
        tool.waiting = Some(1);
        assert!(!tool.can_run(), "one search at a time");
    }
}
