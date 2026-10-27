//! The document lifecycle in the app: open, new, save, save as, revert, close, quit, drag and drop, and noticing outside changes.
//!
//! Nothing is lost silently: a document with unsaved changes asks before it closes, a file that changed on disk asks before it is
//! overwritten, a character the encoding cannot hold asks what to do, and a file whose bytes were not valid cannot be saved over.

use std::path::{Path, PathBuf};

use xmlcraft_code_editor::{CodeEditor, ViewState};
use xmlcraft_engine::analysis::{Analysis, analyze};
use xmlcraft_engine::{Document, SaveError};
use xmlcraft_platform::files::{self, DiskChange};
use xmlcraft_text::encoding::{Encoding, Unmappable};

use crate::dialog::Dialog;
use crate::editor_view::{XmlStyler, editor_colors};
use crate::i18n::{fmt, tl};
use crate::theme::{self, ThemeKind, Tokens};
use crate::{TabInfo, XmlCraftApp};

/// What to do once a document has been saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AfterClose {
    /// Close the document that was saved.
    Close,
    /// Go on with quitting.
    Quit,
}

/// What the view keeps for one document.
pub(crate) struct DocView {
    pub view: ViewState,
    pub styler: XmlStyler,
    /// The latest analysis of the text (its problems and outline), which may be about an older version while a newer one is worked out.
    pub analysis: Option<Analysis>,
    /// The version of the text last handed to the analyzer.
    requested: Option<u64>,
}

impl DocView {
    pub fn new(kind: ThemeKind) -> Self {
        Self { view: ViewState::new(), styler: XmlStyler::new(kind), analysis: None, requested: None }
    }

    /// Keep `analysis` and underline its problems in the editor.
    fn set_analysis(&mut self, analysis: Analysis) {
        self.styler.set_problems(analysis.diagnostics.iter().map(|d| (d.span.clone(), d.severity())).collect());
        self.analysis = Some(analysis);
    }
}

impl XmlCraftApp {
    /// The tabs: one per open document.
    pub fn tabs(&self) -> Vec<TabInfo> {
        self.session.documents().iter().map(|d| TabInfo { name: d.name().to_owned(), dirty: d.is_dirty() }).collect()
    }

    /// The index of the document being shown (`None` is the home view).
    pub fn active(&self) -> Option<usize> {
        self.session.active()
    }

    /// Whether closing the window now would lose work.
    pub fn has_unsaved_changes(&self) -> bool {
        self.session.has_unsaved_changes()
    }

    pub(crate) fn message(&mut self, title: &str, text: String) {
        log::warn!("{title}: {text}");
        self.dialog = Some(Dialog::Message { title: title.to_owned(), text });
    }

    fn id_of_active(&self) -> Option<u64> {
        self.session.active_document().map(Document::id)
    }

    // --- opening ---------------------------------------------------------------------------------------------------------------

    /// Open the file at `path` and show it. A file that is already open is just shown. Returns whether it is now open.
    pub fn open_path(&mut self, path: &Path) -> bool {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let loaded = match files::read_file(&canonical) {
            Ok(loaded) => loaded,
            Err(e) => {
                self.message(
                    tl!("Couldn't open the file"),
                    fmt(tl!("{path}: {error}"), &[("path", &path.display().to_string()), ("error", &e.to_string())]),
                );
                return false;
            }
        };
        match self.session.open(Some(canonical), &loaded.bytes, Some(loaded.stamp)) {
            Ok(_) => {
                self.focus_editor = true;
                if let Some(doc) = self.session.active_document()
                    && doc.is_lossy()
                {
                    self.status = fmt(
                        tl!("{name} has bytes that are not valid {encoding}: they are shown as �, and the file cannot be saved over."),
                        &[("name", doc.name()), ("encoding", doc.encoding().encoding.name())],
                    );
                }
                true
            }
            Err(e) => {
                self.message(
                    tl!("Couldn't open the file"),
                    fmt(tl!("{path}: {error}"), &[("path", &path.display().to_string()), ("error", &e.to_string())]),
                );
                false
            }
        }
    }

    pub(crate) fn new_document(&mut self) {
        self.session.new_document();
        self.focus_editor = true;
    }

    /// Open the files dropped on the window, and show a hint while files are over it.
    pub(crate) fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        for path in dropped {
            self.open_path(&path);
        }
    }

    // --- analysis --------------------------------------------------------------------------------------------------------------

    /// Collect finished analyses, and ask for one of the document being shown if its text has changed since. Called every frame.
    pub(crate) fn pump_analysis(&mut self) {
        let kind = self.applied.unwrap_or(ThemeKind::Light);
        for (id, analysis) in self.analyzer.poll() {
            if self.session.position_of(id).is_none() {
                continue; // closed meanwhile
            }
            let view = self.views.entry(id).or_insert_with(|| DocView::new(kind));
            if view.analysis.as_ref().is_none_or(|current| current.version <= analysis.version) {
                view.set_analysis(analysis);
            }
        }
        if let Some(doc) = self.session.active_document() {
            let (id, version) = (doc.id(), doc.editor().version());
            let view = self.views.entry(id).or_insert_with(|| DocView::new(kind));
            if view.requested != Some(version) {
                view.requested = Some(version);
                self.analyzer.submit(id, version, doc.editor().buffer());
            }
        }
    }

    /// The analysis of the document being shown, if there is one.
    pub(crate) fn active_analysis(&self) -> Option<&Analysis> {
        let id = self.session.active_document()?.id();
        self.views.get(&id)?.analysis.as_ref()
    }

    /// The problems found in the text of the document being shown, as (errors, warnings), once an analysis has arrived. It may be
    /// about a version slightly older than the text while a newer one is worked out.
    pub fn problems(&self) -> Option<(usize, usize)> {
        self.active_analysis().filter(|a| a.skipped.is_none()).map(|a| (a.error_count(), a.warning_count()))
    }

    /// Check the document being shown now and show the result in Messages (F7).
    pub(crate) fn check_now(&mut self) {
        let kind = self.applied.unwrap_or(ThemeKind::Light);
        let Some(doc) = self.session.active_document() else { return };
        let (id, version) = (doc.id(), doc.editor().version());
        let name = doc.name().to_owned();
        let analysis = analyze(&doc.editor().text(), version);
        let errors = analysis.error_count();
        let view = self.views.entry(id).or_insert_with(|| DocView::new(kind));
        view.requested = Some(version);
        view.set_analysis(analysis);
        self.bottom_open = true;
        self.status = if errors == 0 {
            fmt(tl!("{name} is well-formed"), &[("name", &name)])
        } else {
            fmt(tl!("{name}: {count} errors"), &[("name", &name), ("count", &errors.to_string())])
        };
    }

    /// Select `range` in the document being shown and give the editor the keyboard (a message or an outline entry was clicked).
    pub(crate) fn jump_to(&mut self, range: std::ops::Range<usize>) {
        if let Some(doc) = self.session.active_document_mut() {
            doc.editor_mut().break_undo_group();
            doc.editor_mut().set_selection(range.start, range.end);
            self.focus_editor = true;
        }
    }

    /// Go to a position typed as `line` or `line:column` (both counted from 1).
    pub(crate) fn go_to_line(&mut self, input: &str) -> bool {
        let Some((line, column)) = parse_position(input) else { return false };
        let Some(doc) = self.session.active_document() else { return false };
        let buffer = doc.editor().buffer();
        let line = line.saturating_sub(1).min(buffer.line_count().saturating_sub(1));
        let offset = buffer.offset_of(xmlcraft_text::Position { line, col: column.saturating_sub(1) });
        self.jump_to(offset..offset);
        if let Some(doc) = self.session.active_document() {
            let id = doc.id();
            // Put the line near the top, where it is easy to find.
            if let Some(view) = self.views.get_mut(&id) {
                view.view.scroll_to_line(line.saturating_sub(3));
            }
        }
        true
    }

    // --- showing ---------------------------------------------------------------------------------------------------------------

    /// The middle of the window: the editor of the document being shown, or the home view.
    pub(crate) fn central(&mut self, ui: &mut egui::Ui) {
        let Some(index) = self.session.active() else {
            crate::home::show(ui);
            return;
        };
        let kind = self.applied.unwrap_or(ThemeKind::Light);
        let focus = std::mem::take(&mut self.focus_editor);
        let tokens = Tokens::get(ui.ctx());
        let colors = editor_colors(&tokens);
        let Some(doc) = self.session.document_mut(index) else { return };
        let id = doc.id();
        let view = self.views.entry(id).or_insert_with(|| DocView::new(kind));
        let salt = egui::Id::new(("xmlcraft-editor", id));
        if focus {
            let widget = ui.make_persistent_id(salt);
            ui.memory_mut(|m| m.request_focus(widget));
        }
        CodeEditor::new(salt, doc.editor_mut(), &mut view.view, colors).font(theme::mono(13.0)).styler(&mut view.styler).show(ui);
    }

    /// While files are dragged over the window, say that dropping opens them.
    pub(crate) fn show_drop_hint(&self, ctx: &egui::Context) {
        if ctx.input(|i| i.raw.hovered_files.is_empty()) {
            return;
        }
        let t = Tokens::get(ctx);
        let screen = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("xmlcraft-drop-hint")));
        painter.rect_filled(screen, 0.0, t.accent.gamma_multiply(0.12));
        painter.rect_stroke(screen.shrink(8.0), 10.0, egui::Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
        painter.text(screen.center(), egui::Align2::CENTER_CENTER, tl!("Drop to open"), theme::semibold(20.0), t.accent_text);
    }

    // --- closing ---------------------------------------------------------------------------------------------------------------

    /// Close the document at `index`, asking first when it has unsaved changes.
    pub fn request_close(&mut self, index: usize) {
        let Some(doc) = self.session.document(index) else { return };
        if doc.is_dirty() {
            let id = doc.id();
            self.session.set_active(Some(index));
            self.dialog = Some(Dialog::Unsaved { id, then: AfterClose::Close });
        } else {
            self.close_index(index);
        }
    }

    /// Close without asking (the caller has).
    pub(crate) fn close_document(&mut self, id: u64) {
        if let Some(index) = self.session.position_of(id) {
            self.close_index(index);
        }
    }

    fn close_index(&mut self, index: usize) {
        if let Some(closed) = self.session.close(index) {
            self.views.remove(&closed.id());
        }
    }

    // --- saving ----------------------------------------------------------------------------------------------------------------

    /// Save the document being shown.
    pub(crate) fn save_active(&mut self, ctx: &egui::Context) {
        if let Some(id) = self.id_of_active() {
            self.save_document(ctx, id, None);
        }
    }

    pub(crate) fn save_as_active(&mut self, ctx: &egui::Context) {
        if let Some(id) = self.id_of_active() {
            self.pick_save_as(ctx, id, None);
        }
    }

    /// Save a document where it came from; one without a file yet asks where.
    pub(crate) fn save_document(&mut self, ctx: &egui::Context, id: u64, then: Option<AfterClose>) {
        let Some(doc) = self.session.position_of(id).and_then(|i| self.session.document(i)) else { return };
        match doc.path().map(Path::to_path_buf) {
            None => {
                self.pick_save_as(ctx, id, then);
            }
            Some(_) if !doc.can_overwrite_source() => self.dialog = Some(Dialog::Lossy { id }),
            Some(path) => self.write_document(ctx, id, path, Unmappable::Refuse, false, then),
        }
    }

    /// Save a document under the name the user chose. Replacing a file that is not the document's own is the panel's question
    /// (the native panel asks before replacing).
    pub(crate) fn save_document_as(&mut self, ctx: &egui::Context, id: u64, path: PathBuf, then: Option<AfterClose>) {
        self.write_document(ctx, id, path, Unmappable::Refuse, true, then);
    }

    /// Write the document to `path`. `force` skips the check that the file changed on disk since it was read (the user said to, or the
    /// file is a different one).
    pub(crate) fn write_document(&mut self, ctx: &egui::Context, id: u64, path: PathBuf, policy: Unmappable, force: bool, then: Option<AfterClose>) {
        let Some(index) = self.session.position_of(id) else { return };
        let Some(doc) = self.session.document(index) else { return };
        let same_file = doc.path() == Some(path.as_path());
        if same_file
            && !force
            && let Some(stamp) = doc.stamp()
            && files::changed_on_disk(&path, stamp) != DiskChange::Unchanged
            && path.exists()
        {
            self.dialog = Some(Dialog::OverwriteChanged { id, path, then });
            return;
        }
        let bytes = match doc.to_bytes(policy) {
            Ok(bytes) => bytes,
            Err(SaveError::Unmappable(error)) => {
                self.dialog = Some(Dialog::Unmappable { id, path, then, character: error.ch });
                return;
            }
            Err(SaveError::Lossy) => {
                self.dialog = Some(Dialog::Lossy { id });
                return;
            }
        };
        match files::write_file(&path, &bytes) {
            Ok(stamp) => {
                let canonical = std::fs::canonicalize(&path).unwrap_or(path);
                if let Some(doc) = self.session.document_mut(index) {
                    doc.mark_saved(canonical, Some(stamp));
                    self.status = fmt(tl!("Saved {name}"), &[("name", doc.name())]);
                }
                self.after_save(ctx, id, then);
            }
            Err(e) => {
                self.quitting = false;
                self.message(
                    tl!("Couldn't save the file"),
                    fmt(tl!("{path}: {error}"), &[("path", &path.display().to_string()), ("error", &e.to_string())]),
                );
            }
        }
    }

    fn after_save(&mut self, ctx: &egui::Context, id: u64, then: Option<AfterClose>) {
        match then {
            Some(AfterClose::Close) => self.close_document(id),
            Some(AfterClose::Quit) => {
                self.close_document(id);
                self.continue_quit(ctx);
            }
            None => {}
        }
    }

    /// Save as UTF-8 when the document's encoding cannot hold a character.
    pub(crate) fn change_encoding_and_write(&mut self, ctx: &egui::Context, id: u64, path: PathBuf, then: Option<AfterClose>) {
        if let Some(doc) = self.session.position_of(id).and_then(|i| self.session.document_mut(i)) {
            doc.set_encoding(Encoding::UTF_8, false);
        }
        self.write_document(ctx, id, path, Unmappable::Refuse, true, then);
    }

    // --- reverting and outside changes -----------------------------------------------------------------------------------------

    /// Throw away the changes of the document being shown and read the file again (after asking, when there are changes).
    pub(crate) fn revert_active(&mut self) {
        let Some(doc) = self.session.active_document() else { return };
        let id = doc.id();
        if doc.path().is_none() {
            return;
        }
        if doc.is_dirty() {
            self.dialog = Some(Dialog::ConfirmRevert { id });
        } else {
            self.reload_document(id);
        }
    }

    pub(crate) fn reload_document(&mut self, id: u64) {
        let Some(index) = self.session.position_of(id) else { return };
        let Some(path) = self.session.document(index).and_then(|d| d.path().map(Path::to_path_buf)) else { return };
        match files::read_file(&path) {
            Ok(loaded) => {
                if let Some(doc) = self.session.document_mut(index) {
                    match doc.reload(&loaded.bytes, Some(loaded.stamp)) {
                        Ok(()) => self.status = fmt(tl!("Reloaded {name} from disk"), &[("name", doc.name())]),
                        Err(e) => self.message(tl!("Couldn't reload the file"), e.to_string()),
                    }
                }
                if let Some(view) = self.views.get_mut(&id) {
                    view.view.reset();
                    view.styler.reset();
                }
            }
            Err(e) => self.message(
                tl!("Couldn't reload the file"),
                fmt(tl!("{path}: {error}"), &[("path", &path.display().to_string()), ("error", &e.to_string())]),
            ),
        }
    }

    /// Look at the files of the open documents: one that changed on disk is reloaded if it has no edits, and otherwise the user is asked.
    /// Called when the window gets the focus back.
    pub fn check_disk_changes(&mut self) {
        let mut changed: Vec<(u64, bool)> = Vec::new();
        for doc in self.session.documents() {
            let (Some(path), Some(stamp)) = (doc.path(), doc.stamp()) else { continue };
            match files::changed_on_disk(path, stamp) {
                DiskChange::Unchanged => {}
                DiskChange::Modified => changed.push((doc.id(), doc.is_dirty())),
                DiskChange::Missing => {
                    self.status = fmt(tl!("{name} is no longer on disk. Saving will write it again."), &[("name", doc.name())]);
                }
            }
        }
        for (id, dirty) in changed {
            if dirty {
                if self.dialog.is_none() {
                    self.dialog = Some(Dialog::Reload { id });
                }
            } else {
                self.reload_document(id);
            }
        }
    }

    /// Accept the file as it is on disk without reading it (the user chose to keep their version): ask no more about this change. The
    /// text stays as it is and stays unsaved.
    pub(crate) fn keep_version(&mut self, id: u64) {
        let Some(index) = self.session.position_of(id) else { return };
        let Some(path) = self.session.document(index).and_then(|d| d.path().map(Path::to_path_buf)) else { return };
        if let Ok(stamp) = files::stamp_of(&path)
            && let Some(doc) = self.session.document_mut(index)
        {
            doc.set_stamp(Some(stamp));
        }
    }

    // --- quitting --------------------------------------------------------------------------------------------------------------

    /// Quit, after the documents with unsaved changes have each been dealt with.
    pub(crate) fn request_quit(&mut self, ctx: &egui::Context) {
        self.quitting = true;
        self.continue_quit(ctx);
    }

    pub(crate) fn continue_quit(&mut self, ctx: &egui::Context) {
        if !self.quitting || self.dialog.is_some() {
            return;
        }
        if let Some(index) = self.session.documents().iter().position(Document::is_dirty) {
            let id = self.session.documents()[index].id();
            self.session.set_active(Some(index));
            self.dialog = Some(Dialog::Unsaved { id, then: AfterClose::Quit });
            return;
        }
        self.quit_confirmed = true;
        self.release_control_file();
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

/// `12` or `12:5`: a line, and optionally a column (counted in characters), from 1.
fn parse_position(input: &str) -> Option<(usize, usize)> {
    let (line, column) = match input.trim().split_once(':') {
        Some((line, column)) => (line.trim().parse().ok()?, column.trim().parse().ok()?),
        None => (input.trim().parse().ok()?, 1),
    };
    (line >= 1 && column >= 1).then_some((line, column))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_are_a_line_and_maybe_a_column() {
        assert_eq!(parse_position("12"), Some((12, 1)));
        assert_eq!(parse_position(" 12 : 5 "), Some((12, 5)));
        for bad in ["", "0", "0:3", "3:0", "a", "1:b", "-4", "1:2:3", ":"] {
            assert_eq!(parse_position(bad), None, "{bad:?}");
        }
    }
}
