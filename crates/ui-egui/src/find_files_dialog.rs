//! Find in Files: a text or a regular expression looked for in the files under a folder (`xmlcraft_platform::search`, on a thread), the
//! hits listed by file, and a click on a hit opening the file at that line. Files are only read. The window comes back with its last
//! query and result, so the next hit is one click away.

use std::path::{Path, PathBuf};

use xmlcraft_editor::find::Query;
use xmlcraft_platform::search::{Found, Options, search_folder};

use crate::XmlCraftApp;
use crate::dialog::Dialog;
use crate::http_pane::Outcome;
use crate::i18n::{fmt, tl};
use crate::kit::{self, Footer, Tone};
use crate::widgets;

/// The most rows (file names and hits) the list draws.
const LISTED: usize = 1_000;
/// The height of a row of the list.
const ROW: f32 = 22.0;

/// The window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FindFilesForm {
    folder: String,
    pattern: String,
    glob: String,
    regex: bool,
    case_sensitive: bool,
    whole_word: bool,
    /// The search being waited for.
    waiting: Option<u64>,
    outcome: Option<Result<Found, String>>,
    focused: bool,
}

impl FindFilesForm {
    fn new(folder: String) -> Self {
        Self {
            folder,
            pattern: String::new(),
            glob: String::new(),
            regex: false,
            case_sensitive: true,
            whole_word: false,
            waiting: None,
            outcome: None,
            focused: false,
        }
    }

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
}

/// What a click did: open this file at this line and column.
type Jump = (PathBuf, usize, usize);

impl XmlCraftApp {
    /// Open the Find in Files window (Edit ▸ Find in Files…).
    pub(crate) fn open_find_files(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        if cfg!(target_arch = "wasm32") {
            self.status = tl!("Find in Files needs the desktop app: the browser cannot read folders.").to_owned();
            return;
        }
        let mut form = self.find_files_last.take().map(|b| *b).unwrap_or_else(|| FindFilesForm::new(String::new()));
        form.waiting = None;
        form.focused = false;
        if form.folder.is_empty() {
            form.folder =
                self.document_folder().or_else(|| std::env::current_dir().ok()).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        }
        // A selection on one line is what to look for.
        if let Some(doc) = self.session.active_document() {
            let selection = doc.editor().primary();
            if !selection.is_empty()
                && let Ok(text) = doc.editor().buffer().slice(selection.start()..selection.end())
                && !text.contains(['\n', '\r'])
                && text.chars().count() <= 200
            {
                form.pattern = text.to_string();
                form.regex = false;
            }
        }
        self.dialog = Some(Dialog::FindFiles(Box::new(form)));
    }

    /// The folder of the document being shown, when it is a file.
    fn document_folder(&self) -> Option<PathBuf> {
        self.session.active_document().and_then(|d| d.path()).and_then(Path::parent).map(Path::to_path_buf)
    }

    fn run_find_files(&mut self, form: &mut FindFilesForm) {
        self.generate_serial += 1;
        let serial = self.generate_serial;
        let (folder, options) = (PathBuf::from(form.folder.trim()), form.options());
        let started = self.spawn_http(move || Outcome::Files { serial, result: search_folder(&folder, &options).map_err(|e| e.to_string()) });
        match started {
            Ok(()) => {
                form.waiting = Some(serial);
                form.outcome = None;
            }
            Err(why) => form.outcome = Some(Err(why)),
        }
    }

    /// The answer of a search, if the window is still waiting for it.
    pub(crate) fn find_files_answered(&mut self, serial: u64, result: Result<Found, String>) {
        let Some(Dialog::FindFiles(form)) = &mut self.dialog else { return };
        if form.waiting != Some(serial) {
            return;
        }
        form.waiting = None;
        form.outcome = Some(result);
    }

    /// Draw the window and act on what was done in it.
    pub(crate) fn show_find_files(&mut self, ctx: &egui::Context) {
        let Some(Dialog::FindFiles(mut form)) = self.dialog.take() else { return };
        let ((action, jump), closed) = kit::dialog(
            ctx,
            "xmlcraft-find-files",
            720.0,
            "search",
            tl!("Find in Files"),
            tl!("Look for text in the files under a folder. Files are only read."),
            |ui| find_files_form(ui, &mut form),
        );
        if let Some((path, line, column)) = jump {
            self.find_files_last = Some(form);
            if self.open_path(&path) {
                self.go_to_line(&format!("{line}:{column}"));
            }
            return;
        }
        match action {
            Some(Footer::Primary) => {
                self.run_find_files(&mut form);
                self.dialog = Some(Dialog::FindFiles(form));
            }
            Some(Footer::Cancel) => self.find_files_last = Some(form),
            _ if closed => self.find_files_last = Some(form),
            _ => self.dialog = Some(Dialog::FindFiles(form)),
        }
    }
}

/// The path as the list shows it: relative to the folder searched.
fn shown(path: &Path, folder: &str) -> String {
    path.strip_prefix(folder.trim()).unwrap_or(path).to_string_lossy().replace('\\', "/")
}

fn find_files_form(ui: &mut egui::Ui, form: &mut FindFilesForm) -> (Option<Footer>, Option<Jump>) {
    let mut submit = false;
    kit::section(ui, tl!("Search"), |ui| {
        let response = ui.add(widgets::line(&mut form.pattern).desired_width(f32::INFINITY).hint_text(tl!("Text or regular expression")));
        widgets::name_for_assistive_technology(ui, &response, tl!("What to find"));
        if !form.focused {
            response.request_focus();
            form.focused = true;
        }
        submit |= response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.horizontal(|ui| {
            ui.add(widgets::checkbox(&mut form.regex, tl!("Regular expression")));
            ui.add(widgets::checkbox(&mut form.case_sensitive, tl!("Match case")));
            ui.add(widgets::checkbox(&mut form.whole_word, tl!("Whole word")));
        });
        ui.add_space(kit::GAP);
        let response = ui.add(widgets::line(&mut form.folder).desired_width(f32::INFINITY).hint_text(tl!("Folder")));
        widgets::name_for_assistive_technology(ui, &response, tl!("The folder to search"));
        submit |= response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let response =
            ui.add(widgets::line(&mut form.glob).desired_width(f32::INFINITY).hint_text(tl!("File names: *.xml;*.xsd (empty: all files)")));
        widgets::name_for_assistive_technology(ui, &response, tl!("The names of the files to search"));
        submit |= response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        kit::help(
            ui,
            tl!("Folders that start with a dot, node_modules and target are skipped; links are not followed; files that are not text are skipped."),
        );
    });
    let mut jump = None;
    if form.waiting.is_some() {
        ui.add_space(kit::GAP_GROUP);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Searching…"));
        });
    }
    match &form.outcome {
        None => {}
        Some(Err(message)) => {
            ui.add_space(kit::GAP_GROUP);
            kit::banner(ui, Tone::Bad, message);
        }
        Some(Ok(found)) => {
            kit::section(ui, tl!("Result"), |ui| {
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
                let mut drawn = 0;
                egui::ScrollArea::vertical().max_height(280.0).auto_shrink([false, true]).show(ui, |ui| {
                    'files: for file in &found.files {
                        if drawn >= LISTED {
                            break;
                        }
                        let head = format!("{}  ({})", shown(&file.path, &form.folder), file.hits.len());
                        ui.add(egui::Label::new(egui::RichText::new(head).strong()).truncate());
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
                            if response.clicked() {
                                jump = Some((file.path.clone(), hit.line, hit.column));
                            }
                        }
                    }
                });
            });
        }
    }
    let can = form.waiting.is_none() && !form.pattern.is_empty() && !form.folder.trim().is_empty();
    let pressed = match kit::footer(ui, tl!("Close"), tl!("Find"), Some("search"), can, tl!("Type what to find and a folder first.")) {
        Footer::Nothing => None,
        other => Some(other),
    };
    let pressed = if submit && can { Some(Footer::Primary) } else { pressed };
    (pressed, jump)
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
        let mut form = FindFilesForm::new("/p".to_owned());
        form.pattern = "x".to_owned();
        form.glob = "*.xml".to_owned();
        form.whole_word = true;
        let options = form.options();
        assert_eq!(
            (options.query.pattern.as_str(), options.glob.as_str(), options.query.whole_word, options.query.regex),
            ("x", "*.xml", true, false)
        );
    }
}
