//! The document lifecycle in the app: open, new, save, save as, revert, close, quit, drag and drop, and noticing outside changes.
//!
//! Nothing is lost silently: a document with unsaved changes asks before it closes, a file that changed on disk asks before it is
//! overwritten, a character the encoding cannot hold asks what to do, and a file whose bytes were not valid cannot be saved over.

use std::path::{Path, PathBuf};

use xmlcraft_code_editor::{CodeEditor, ViewState};
use xmlcraft_engine::analysis::{Analysis, Skipped};
use xmlcraft_engine::validation::{Validation, Verdict};
use xmlcraft_engine::{Document, SaveError};
use xmlcraft_platform::files::{self, DiskChange};
use xmlcraft_text::encoding::Encoding;

use crate::after_analysis::Then;
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
    /// The latest validation against the DTD (asked for with F8), which may be about an older version than the text.
    pub validation: Option<Validation>,
    /// The version of the text last handed to the analyzer.
    requested: Option<u64>,
    /// The grammar of the DOCTYPE, and what Entry Helpers offer from it.
    pub(crate) helpers: crate::entry_helpers::HelperState,
    /// Validating while the text is typed in.
    auto: AutoValidation,
}

/// What is known of validating a document while it is typed in.
#[derive(Default)]
struct AutoValidation {
    /// The version of the text the app saw last, and when it first saw it: how long the text has stood as it is.
    seen: Option<(u64, std::time::Instant)>,
    /// The version of the text a validation was asked for (by F8 or by this).
    requested: Option<u64>,
    /// How many validations of this document were asked for.
    count: usize,
}

impl DocView {
    pub fn new(kind: ThemeKind) -> Self {
        Self {
            view: ViewState::new(),
            styler: XmlStyler::new(kind),
            analysis: None,
            validation: None,
            requested: None,
            helpers: Default::default(),
            auto: AutoValidation::default(),
        }
    }

    /// Keep `analysis`, underline its problems in the editor and offer what it says can be folded.
    fn set_analysis(&mut self, analysis: Analysis) {
        self.styler.set_folds(analysis.folds.clone());
        self.analysis = Some(analysis);
        self.refresh_underlines();
    }

    /// Keep `validation` (a newer one replaces an older one) and underline the problems it found in the text.
    fn set_validation(&mut self, validation: Validation) {
        if self.validation.as_ref().is_none_or(|current| current.version <= validation.version) {
            self.validation = Some(validation);
            self.refresh_underlines();
        }
    }

    /// The validation, when it is about the same version of the text as the analysis (an edit makes it out of date).
    pub(crate) fn current_validation(&self) -> Option<&Validation> {
        let analysis = self.analysis.as_ref()?;
        self.validation.as_ref().filter(|v| v.version == analysis.version)
    }

    /// Underline the problems of the text: the well-formedness ones, and the validity ones while they are about the same text.
    fn refresh_underlines(&mut self) {
        let mut problems: Vec<_> = self.analysis.iter().flat_map(|a| a.diagnostics.iter()).map(|d| (d.span.clone(), d.severity())).collect();
        problems.extend(self.current_validation().into_iter().flat_map(|v| v.in_document()).map(|d| (d.span.clone(), d.severity())));
        self.styler.set_problems(problems);
    }

    /// The version of the text last handed to the analyzer.
    pub(crate) fn requested(&self) -> Option<u64> {
        self.requested
    }

    pub(crate) fn set_requested(&mut self, version: u64) {
        self.requested = Some(version);
    }
}

impl XmlCraftApp {
    /// The files opened or saved lately, newest first (what the home view lists).
    pub fn recent_files(&self) -> &[PathBuf] {
        self.session.recent().paths()
    }

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

    /// Open the file at `path` and show it. A file that is already open is just shown. Returns whether it is now open; if not, the
    /// reason is shown to the user.
    pub fn open_path(&mut self, path: &Path) -> bool {
        match self.try_open(path) {
            Ok(()) => true,
            Err(reason) => {
                self.message(tl!("Couldn't open the file"), reason);
                false
            }
        }
    }

    /// Open several files (the command line, the Open panel, a drop), each in its own tab. The ones that fail are reported together, in
    /// one message, instead of each replacing the one before.
    pub fn open_paths<'p>(&mut self, paths: impl IntoIterator<Item = &'p Path>) {
        let failures: Vec<String> = paths.into_iter().filter_map(|path| self.try_open(path).err()).collect();
        match failures.as_slice() {
            [] => {}
            [only] => self.message(tl!("Couldn't open the file"), only.clone()),
            many => self.message(tl!("Couldn't open some of the files"), many.join("\n")),
        }
    }

    /// Open one file; the error is the line to show for it.
    fn try_open(&mut self, path: &Path) -> Result<(), String> {
        let failed = |e: &dyn std::fmt::Display| fmt(tl!("{path}: {error}"), &[("path", &path.display().to_string()), ("error", &e.to_string())]);
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let loaded = files::read_file(&canonical).inspect_err(|_| self.session.forget_recent(&canonical)).map_err(|e| failed(&e))?;
        self.session.open(Some(canonical), &loaded.bytes, Some(loaded.stamp)).map_err(|e| failed(&e))?;
        self.focus_editor = true;
        if let Some(doc) = self.session.active_document()
            && doc.is_lossy()
        {
            self.status = fmt(
                tl!("{name} has bytes that are not valid {encoding}: they are shown as �, and the file cannot be saved over."),
                &[("name", doc.name()), ("encoding", doc.encoding().encoding.name())],
            );
        }
        Ok(())
    }

    pub(crate) fn new_document(&mut self) {
        self.session.new_document();
        self.focus_editor = true;
    }

    /// Open the files dropped on the window, and show a hint while files are over it.
    pub(crate) fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        self.open_paths(dropped.iter().map(PathBuf::as_path));
    }

    // --- analysis --------------------------------------------------------------------------------------------------------------

    /// Collect finished analyses, ask for one of the document being shown if its text has changed since, and report a check (F7) that
    /// was waiting for its result. Called every frame, after whatever changes a text.
    pub(crate) fn pump_analysis(&mut self) {
        let kind = self.applied.unwrap_or(ThemeKind::Light);
        for (id, validation) in self.validator.poll() {
            if self.session.position_of(id).is_some() {
                self.views.entry(id).or_insert_with(|| DocView::new(kind)).set_validation(validation);
            }
        }
        for (id, schema) in self.schemas.poll() {
            if let Some(view) = self.views.get_mut(&id) {
                view.helpers.accept(schema);
            }
        }
        for (id, answer) in self.xsd_schemas.poll() {
            if let Some(view) = self.views.get_mut(&id) {
                view.helpers.accept_xsd(answer);
            }
        }
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
        self.request_schema();
        self.request_xsd();
        self.auto_validate();
        self.report_check();
        self.report_validation();
        self.finish_waiting();
    }

    /// Ask for the XML Schema the document being shown names (when it has no DOCTYPE) when what it is made of is not what the schema
    /// was asked for with: the schemas named, the place the document is at, or the other documents that are open (a schema may be one
    /// of them, as it is being edited). Looked at every frame, since an edit of another document changes the answer.
    fn request_xsd(&mut self) {
        let Some(doc) = self.session.active_document() else { return };
        let id = doc.id();
        let Some(view) = self.views.get_mut(&id) else { return };
        // The schemas the document names; when it names none, the one known for the namespace its root element is in (for what is not
        // known, the answer is that there is none, which the pane does not show as a problem).
        let hints = match view.analysis.as_ref() {
            Some(analysis) if analysis.doctype.is_none() && !analysis.schema_hints.is_empty() => analysis.schema_hints.clone(),
            Some(analysis) if analysis.doctype.is_none() => {
                analysis.root_namespace.as_deref().map(xmlcraft_engine::xsd_schemas::implicit_hints).unwrap_or_default()
            }
            _ => Vec::new(),
        };
        if hints.is_empty() {
            view.helpers.forget_xsd();
            return;
        }
        let versions: Vec<(String, u64)> = self
            .session
            .documents()
            .iter()
            .filter(|d| d.id() != id)
            .filter_map(|d| d.path().map(|p| (p.to_string_lossy().into_owned(), d.editor().version())))
            .collect();
        let key = xmlcraft_engine::xsd_schemas::xsd_key(&hints, doc.path(), &versions);
        if view.helpers.requested_xsd_key == Some(key) {
            return;
        }
        view.helpers.requested_xsd_key = Some(key);
        view.helpers.xsd_requests += 1;
        let open = self
            .session
            .documents()
            .iter()
            .filter_map(|d| d.path().map(|p| (p.to_string_lossy().into_owned(), d.editor().buffer().clone())))
            .collect();
        self.xsd_schemas.submit(id, key, hints, doc.path(), open);
    }

    /// Ask for the grammar of the DOCTYPE of the document being shown when the DOCTYPE (or the place the document is at) is not the one
    /// the grammar was asked for, once per analysis.
    fn request_schema(&mut self) {
        let Some(doc) = self.session.active_document() else { return };
        let id = doc.id();
        let Some(view) = self.views.get_mut(&id) else { return };
        let Some(analysis) = view.analysis.as_ref() else { return };
        if view.helpers.checked_version == Some(analysis.version) {
            return;
        }
        view.helpers.checked_version = Some(analysis.version);
        let Some(range) = analysis.doctype.clone() else {
            // No DOCTYPE any more: no grammar.
            view.helpers.requested_key = None;
            view.helpers.grammar_key = None;
            view.helpers.grammar = None;
            return;
        };
        let Ok(text) = doc.editor().buffer().slice(range.clone()) else { return };
        let key = xmlcraft_engine::schema::key_of(&text, doc.path());
        if view.helpers.requested_key != Some(key) {
            view.helpers.requested_key = Some(key);
            self.schemas.submit(id, key, text, range.start, doc.path());
        }
    }

    /// If a validation (F8) is waiting and its result has arrived (for the text it asked about or a later one), say what it found.
    fn report_validation(&mut self) {
        let Some((id, version)) = self.validate_pending else { return };
        let Some(doc) = self.session.position_of(id).and_then(|i| self.session.document(i)) else {
            self.validate_pending = None; // closed meanwhile
            return;
        };
        let name = doc.name().to_owned();
        let Some(validation) = self.views.get(&id).and_then(|v| v.validation.as_ref()).filter(|v| v.version >= version) else { return };
        let count = validation.error_count().to_string();
        self.status = match validation.verdict {
            Verdict::Valid => fmt(tl!("{name} is valid"), &[("name", &name)]),
            Verdict::Invalid => fmt(tl!("{name}: {count} validity errors"), &[("name", &name), ("count", &count)]),
            Verdict::NotWellFormed => fmt(tl!("{name} is not well-formed, so it cannot be validated"), &[("name", &name)]),
            Verdict::Skipped(Skipped::TooLarge) => fmt(tl!("{name} is too large to validate"), &[("name", &name)]),
            Verdict::Skipped(Skipped::Failed) => fmt(tl!("Validating {name} failed. This is a bug; the document is unchanged."), &[("name", &name)]),
        };
        self.validate_pending = None;
    }

    /// Validate the document being shown in the background when it names a grammar and has been left alone for a moment (and the person
    /// has not turned that off): what is found is underlined and listed, without a word in the status bar and without opening Messages.
    /// Called every frame; asks for a frame when the moment is still to come.
    pub(crate) fn auto_validate(&mut self) {
        let Some(doc) = self.session.active_document() else { return };
        let (id, version, size) = (doc.id(), doc.editor().version(), doc.editor().buffer().len());
        let Some(view) = self.views.get_mut(&id) else { return };
        let now = std::time::Instant::now();
        let quiet_for = match view.auto.seen {
            Some((seen, at)) if seen == version => now.saturating_duration_since(at),
            _ => {
                view.auto.seen = Some((version, now));
                std::time::Duration::ZERO
            }
        };
        let ready = view.analysis.as_ref().is_some_and(|a| crate::auto_validate::ready(a, version));
        // Both ways of asking for a validation (F8, and this) say which version they asked for.
        let done = view.auto.requested == Some(version);
        let inputs = crate::auto_validate::Inputs { enabled: self.validate_typing, ready, size, done, quiet_for, delay: self.validate_delay };
        match crate::auto_validate::decide(&inputs) {
            crate::auto_validate::Decision::Not => {}
            crate::auto_validate::Decision::Wait(left) => {
                if let Some(ctx) = &self.ctx {
                    ctx.request_repaint_after(left);
                }
            }
            crate::auto_validate::Decision::Now => {
                view.auto.requested = Some(version);
                view.auto.count += 1;
                let open = self.open_documents();
                if let Some(doc) = self.session.active_document() {
                    self.validator.submit(id, version, doc.editor().buffer(), doc.path(), open);
                }
            }
        }
    }

    /// How many times a validation of the document being shown was asked for (by F8 or while typing). What tests count.
    pub fn validations_requested(&self) -> usize {
        let Some(id) = self.session.active_document().map(Document::id) else { return 0 };
        self.views.get(&id).map_or(0, |v| v.auto.count)
    }

    /// How long a text is left alone before it is validated while it is typed in (the default is [`crate::auto_validate::DELAY`]).
    pub fn set_validate_delay(&mut self, delay: std::time::Duration) {
        self.validate_delay = delay;
    }

    /// Validate the document being shown now against its DTD and show the result in Messages (F8). The work is done in the background
    /// (the external parts of the DTD are files, read there) and reported when it arrives.
    pub(crate) fn validate_now(&mut self) {
        let kind = self.applied.unwrap_or(ThemeKind::Light);
        let Some(doc) = self.session.active_document() else { return };
        let (id, version) = (doc.id(), doc.editor().version());
        let name = doc.name().to_owned();
        let view = self.views.entry(id).or_insert_with(|| DocView::new(kind));
        // The well-formedness analysis of the same text is what Messages shows with it.
        if view.requested != Some(version) {
            view.requested = Some(version);
            self.analyzer.submit(id, version, doc.editor().buffer());
        }
        // F8 always checks again: what a document is checked against (a schema that is open and being edited, a DTD or a schema on
        // disk) can change with the text of the document the same. The answer to the last request would otherwise stand for it.
        view.validation = None;
        view.refresh_underlines();
        // The documents that are open are what a schema is read from first: one being edited is validated against as it is.
        view.auto.requested = Some(version);
        view.auto.count += 1;
        let open = self.open_documents();
        self.validator.submit(id, version, doc.editor().buffer(), doc.path(), open);
        self.validate_pending = Some((id, version));
        self.bottom_open = true;
        self.status = fmt(tl!("Validating {name}…"), &[("name", &name)]);
        self.report_validation();
    }

    /// If a check (F7) is waiting and the analysis of the text it asked about has arrived (or a later one), say what it found.
    fn report_check(&mut self) {
        let Some((id, version)) = self.check_pending else { return };
        let Some(doc) = self.session.position_of(id).and_then(|i| self.session.document(i)) else {
            self.check_pending = None; // closed meanwhile
            return;
        };
        let name = doc.name().to_owned();
        let Some(analysis) = self.views.get(&id).and_then(|v| v.analysis.as_ref()).filter(|a| a.version >= version) else { return };
        self.status = match analysis.skipped {
            Some(Skipped::TooLarge) => fmt(tl!("{name} is too large to check"), &[("name", &name)]),
            Some(Skipped::Failed) => fmt(tl!("Checking {name} failed. This is a bug; the document is unchanged."), &[("name", &name)]),
            None if analysis.error_count() == 0 => fmt(tl!("{name} is well-formed"), &[("name", &name)]),
            None => fmt(tl!("{name}: {count} errors"), &[("name", &name), ("count", &analysis.error_count().to_string())]),
        };
        self.check_pending = None;
    }

    /// The names of the two tags of the element the single caret of `doc` is in or next to, if the analysis is about the text as it is now
    /// (one about older text would put them in the wrong place).
    fn matching_tag_names(view: &DocView, editor: &xmlcraft_editor::Editor) -> Option<[std::ops::Range<usize>; 2]> {
        let analysis = view.analysis.as_ref().filter(|a| a.version == editor.version() && a.skipped.is_none())?;
        let caret = editor.primary();
        if editor.carets().len() != 1 || !caret.is_empty() {
            return None;
        }
        analysis.matching_tag(caret.head()).map(|m| [m.name, m.partner_name])
    }

    /// Link the names of the two tags of the element the caret is in the name of, so that what is typed in one is written into the other.
    /// A link follows the text through edits, so it is made from the analysis only when the caret is in no linked name, and only from an
    /// analysis of the text as it is now.
    fn link_tag_names(view: &DocView, editor: &mut xmlcraft_editor::Editor) {
        if editor.caret_in_linked_names() {
            return;
        }
        let caret = editor.primary();
        let names =
            view.analysis.as_ref().filter(|a| a.version == editor.version() && a.skipped.is_none()).and_then(|a| a.linked_names(caret.head()));
        match names {
            Some([a, b]) if editor.carets().len() == 1 && caret.is_empty() => drop(editor.link_names(a, b)),
            _ => editor.unlink_names(),
        }
    }

    /// Where things were on the screen in the last frame of the editor of the document being shown (for tests and the control channel).
    pub fn editor_geometry(&self) -> Option<xmlcraft_code_editor::Geometry> {
        let id = self.session.active_document()?.id();
        self.views.get(&id)?.view.geometry()
    }

    /// The names of the two tags the editor of the document being shown highlights now (the caret is in or next to one of them).
    pub fn highlighted_tag_names(&self) -> Option<[std::ops::Range<usize>; 2]> {
        let id = self.session.active_document()?.id();
        self.views.get(&id)?.styler.tag_match().cloned()
    }

    /// The analysis of the document being shown, if there is one.
    pub(crate) fn active_analysis(&self) -> Option<&Analysis> {
        let id = self.session.active_document()?.id();
        self.views.get(&id)?.analysis.as_ref()
    }

    /// The validation of the document being shown, if one was asked for (F8) and it is about the text as the analysis last saw it.
    pub(crate) fn active_validation(&self) -> Option<&Validation> {
        let id = self.session.active_document()?.id();
        self.views.get(&id)?.current_validation()
    }

    /// The problems found in the text of the document being shown, as (errors, warnings), once an analysis has arrived. It may be
    /// about a version slightly older than the text while a newer one is worked out.
    pub fn problems(&self) -> Option<(usize, usize)> {
        self.active_analysis().filter(|a| a.skipped.is_none()).map(|a| (a.error_count(), a.warning_count()))
    }

    /// How many times the XML Schema the document being shown names was asked for (it is asked for again when the schemas named, the
    /// place of the document or another open document change, not when the document itself is edited).
    pub fn schema_requests(&self) -> usize {
        let Some(id) = self.session.active_document().map(Document::id) else { return 0 };
        self.views.get(&id).map_or(0, |v| v.helpers.xsd_requests)
    }

    /// Whether the grammar of the DOCTYPE of the document being shown has been read, or the XML Schema it names has been loaded (what
    /// Entry Helpers offer comes from it).
    pub fn has_grammar(&self) -> bool {
        let Some(id) = self.session.active_document().map(Document::id) else { return false };
        self.views.get(&id).is_some_and(|v| v.helpers.grammar.is_some() || v.helpers.xsd.is_some())
    }

    /// The places in the text of the document being shown that are underlined as wrong: the ranges and how serious each is.
    pub fn underlined(&self) -> Vec<(std::ops::Range<usize>, xmlcraft_syntax::Severity)> {
        let Some(id) = self.session.active_document().map(Document::id) else { return Vec::new() };
        self.views.get(&id).map(|v| v.styler.problems().to_vec()).unwrap_or_default()
    }

    /// The validity problems found in the text of the document being shown, as (errors, warnings), once a validation of the text as it is
    /// now has arrived (an edit makes it out of date, and it is not shown again until F8 is pressed).
    pub fn validity(&self) -> Option<(usize, usize)> {
        self.active_validation().map(|v| (v.error_count(), v.warning_count()))
    }

    /// Comment out the lines the carets are on, or bring them back (⌘/); say why when that cannot be done.
    pub(crate) fn toggle_comment(&mut self) {
        let Some(doc) = self.session.active_document_mut() else { return };
        if let Err(refused) = doc.editor_mut().toggle_comment() {
            self.status = fmt(tl!("Can't comment that out: {why}"), &[("why", &refused.to_string())]);
        }
    }

    /// Pretty-print (or, with `minify`, strip the white space of) the document being shown, as one undoable edit. A document that is not
    /// well-formed is left as it is and the reason is shown.
    pub(crate) fn reformat(&mut self, minify: bool) {
        let Some(doc) = self.session.active_document_mut() else { return };
        let name = doc.name().to_owned();
        let result = if minify { doc.minify() } else { doc.pretty_print() };
        match result {
            Ok(true) => {
                self.status = fmt(if minify { tl!("Stripped the white space of {name}") } else { tl!("Pretty-printed {name}") }, &[("name", &name)])
            }
            Ok(false) => self.status = fmt(tl!("{name} needs no changes"), &[("name", &name)]),
            Err(xmlcraft_format::FormatError::NotWellFormed { message, offset }) => {
                let line = doc.editor().buffer().line_of(offset.min(doc.editor().buffer().len())) + 1;
                self.message(
                    tl!("The document is not well-formed"),
                    fmt(
                        tl!("{name} is left as it is: formatting does not guess what broken markup was meant to be. The first problem is on line {line}: {message}"),
                        &[("name", &name), ("line", &line.to_string()), ("message", &message)],
                    ),
                );
            }
        }
    }

    /// Check the document being shown now and show the result in Messages (F7). The check is the background analysis of the text as it
    /// is now (asked for at once if it has not been yet), reported when it arrives: the window never waits for a parse.
    pub(crate) fn check_now(&mut self) {
        let kind = self.applied.unwrap_or(ThemeKind::Light);
        let Some(doc) = self.session.active_document() else { return };
        let (id, version) = (doc.id(), doc.editor().version());
        let view = self.views.entry(id).or_insert_with(|| DocView::new(kind));
        if view.requested != Some(version) {
            view.requested = Some(version);
            self.analyzer.submit(id, version, doc.editor().buffer());
        }
        self.check_pending = Some((id, version));
        self.bottom_open = true;
        self.report_check();
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
            if let Some(path) = crate::home::show(ui, self.session.recent().paths()) {
                self.open_path(&path);
            }
            return;
        };
        let kind = self.applied.unwrap_or(ThemeKind::Light);
        // Not in the frame of a click: egui takes the focus back from the editor at a click that is not on it (a clicked message or
        // outline entry asks for the editor in that very frame), so the request waits one frame.
        let focus = self.focus_editor && !ui.input(|i| i.pointer.any_click());
        if focus {
            self.focus_editor = false;
        } else if self.focus_editor {
            ui.ctx().request_repaint();
        }
        let tokens = Tokens::get(ui.ctx());
        let colors = editor_colors(&tokens);
        // The find bar is above the editor; what it looks for is shown in the text.
        self.find_bar(ui);
        let Some(doc) = self.session.document_mut(index) else { return };
        let (id, version) = (doc.id(), doc.editor().version());
        let matches = self.find.shown_matches(id, version);
        let view = self.views.entry(id).or_insert_with(|| DocView::new(kind));
        view.styler.set_matches(matches);
        view.styler.set_tag_match(Self::matching_tag_names(view, doc.editor()));
        Self::link_tag_names(view, doc.editor_mut());
        // What can be folded is right for the lines above the first one that changed since the analysis (and for none before there is one).
        let valid_before = view.analysis.as_ref().map_or(0, |a| doc.editor().first_changed_line_since(a.version).unwrap_or(usize::MAX));
        view.styler.set_folds_valid_before(valid_before);
        let salt = egui::Id::unique(("xmlcraft-editor", id));
        if focus {
            let widget = ui.make_persistent_id(salt);
            ui.memory_mut(|m| m.request_focus(widget));
        }
        let output = CodeEditor::new(salt, doc.editor_mut(), &mut view.view, colors).font(theme::mono(13.0)).styler(&mut view.styler).show(ui);
        // The mark of something that can be folded was clicked: fold it, knowing how far it goes from the analysis of the text as it is.
        if let Some(line) = output.fold_clicked {
            self.when_analysed(Then::FoldLine(line));
        }
    }

    /// While files are dragged over the window, say that dropping opens them.
    pub(crate) fn show_drop_hint(&self, ctx: &egui::Context) {
        if ctx.input(|i| i.raw.hovered_files.is_empty()) {
            return;
        }
        let t = Tokens::get(ctx);
        let screen = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("xmlcraft-drop-hint")));
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
            Some(_) if !doc.can_overwrite_source() => self.dialog = Some(Dialog::Lossy { id, then }),
            Some(path) => self.write_document(ctx, id, path, false, then),
        }
    }

    /// Save a document under the name the user chose. Replacing a file that is not the document's own is the panel's question (the
    /// native panel asks before replacing); when the name is the document's own file, the check for outside changes still applies.
    pub(crate) fn save_document_as(&mut self, ctx: &egui::Context, id: u64, path: PathBuf, then: Option<AfterClose>) {
        self.write_document(ctx, id, path, false, then);
    }

    /// Write the document to `path`. Unless `force` (the user said to overwrite), a file that is the document's own and changed on disk
    /// since it was read is not replaced without asking.
    pub(crate) fn write_document(&mut self, ctx: &egui::Context, id: u64, path: PathBuf, force: bool, then: Option<AfterClose>) {
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
        let bytes = match doc.to_bytes() {
            Ok(bytes) => bytes,
            Err(SaveError::Unmappable(error)) => {
                self.dialog = Some(Dialog::Unmappable { id, path, then, character: error.ch });
                return;
            }
            Err(SaveError::Lossy) => {
                self.dialog = Some(Dialog::Lossy { id, then });
                return;
            }
        };
        match files::write_file(&path, &bytes) {
            Ok(stamp) => {
                let canonical = std::fs::canonicalize(&path).unwrap_or(path);
                if let Some(doc) = self.session.document_mut(index) {
                    doc.mark_saved(canonical.clone(), Some(stamp));
                    self.status = fmt(tl!("Saved {name}"), &[("name", doc.name())]);
                }
                self.session.remember_recent(canonical);
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

    /// Save as UTF-8 when the document's encoding cannot hold a character. (The XML declaration is brought in line with an edit.) The
    /// check for outside changes still applies: the user answered the encoding question, not that one.
    pub(crate) fn change_encoding_and_write(&mut self, ctx: &egui::Context, id: u64, path: PathBuf, then: Option<AfterClose>) {
        if let Some(doc) = self.session.position_of(id).and_then(|i| self.session.document_mut(i)) {
            doc.set_encoding(Encoding::UTF_8, false);
        }
        self.write_document(ctx, id, path, false, then);
    }

    /// Replace the characters the encoding cannot hold by character references in the text, then write. Where a reference would not
    /// mean the same (a comment, a CDATA section, a processing instruction, a name) nothing is changed and the user is told which
    /// character it is.
    pub(crate) fn write_with_references(&mut self, ctx: &egui::Context, id: u64, path: PathBuf, then: Option<AfterClose>) {
        let Some(doc) = self.session.position_of(id).and_then(|i| self.session.document_mut(i)) else { return };
        match doc.apply_character_references() {
            Ok(_) => self.write_document(ctx, id, path, false, then),
            Err(error) => {
                let (character, line) = match &error {
                    SaveError::Unmappable(e) => (e.ch, doc.editor().buffer().line_of(e.offset) + 1),
                    SaveError::Lossy => ('\u{FFFD}', 0),
                };
                self.quitting = false;
                self.message(
                    tl!("A character reference won't do here"),
                    fmt(
                        tl!("The character '{character}' (U+{code}) on line {line} cannot be written in this file's encoding, and it is in a comment, a CDATA section, a processing instruction or a name, where a character reference would not mean the same. Save the file as UTF-8 instead."),
                        &[("character", &character.to_string()), ("code", &format!("{:04X}", u32::from(character))), ("line", &line.to_string())],
                    ),
                );
            }
        }
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
                    // What was worked out about the old text (its outline, its problems) is not about this one.
                    view.analysis = None;
                    view.requested = None;
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
                    self.status = fmt(tl!("{name} can't be found or read on disk any more. Saving will write it again."), &[("name", doc.name())]);
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

    #[test]
    fn a_validation_that_comes_late_does_not_replace_a_newer_one() {
        let validation = |version| Validation { version, verdict: Verdict::Valid, problems: Vec::new(), truncated: false };
        let mut view = DocView::new(ThemeKind::Light);
        view.set_validation(validation(5));
        view.set_validation(validation(3));
        assert_eq!(view.validation.as_ref().map(|v| v.version), Some(5));
        view.set_validation(validation(6));
        assert_eq!(view.validation.as_ref().map(|v| v.version), Some(6));
    }
}
