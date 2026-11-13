//! Compare, the tool window of comparing and merging documents.
//!
//! **Compare**: the document being shown against another open document or a file, as trees (XML elements, JSON and YAML members) or
//! line by line (`xmlcraft_engine::compare`, on a worker thread). In a wide panel the two texts are shown side by side
//! ([`super::side_by_side`]): the rows aligned, the differences tinted, Previous and Next change, and per hunk a button that copies it
//! to the other document (one undoable edit; the comparison is then made again). The List view (and a narrow panel) lists the
//! differences as the comparison describes them. A click selects a difference in the first document, a right click in the second; the
//! panel stays open.
//!
//! **Merge**: the document being shown (ours) and another open document (theirs), both made from a third (the base), merged line by
//! line (`xmlcraft_diff::merge3`) into a new document. What both changed in the same place is a conflict, settled as asked: taken
//! from ours, theirs, both or the base, or kept as markers (`<<<<<<<`, `|||||||`, `=======`, `>>>>>>>`). The conflicts marked in the
//! result (found in its text again whenever it changes, `xmlcraft_diff::conflicts_in`) are listed with their parts and settled one
//! by one: Take ours, theirs, both or base replaces that conflict's marked lines, as one undoable edit.
//!
//! The other documents are the open ones, listed again at every frame. Only the copy and Take buttons change a document.

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use xmlcraft_diff::{ChangeKind, Choice, Family, LineOptions, MarkedConflict, Side, Whitespace, conflicts_in, merge3};
use xmlcraft_engine::compare::{CompareJob, Compared, Ignore, Mode, family_of};
use xmlcraft_engine::language::Language;

use super::side_by_side::{self, Action, Permits, Texts, ViewState};
use crate::editor_view::Colouring;
use crate::i18n::{fmt, tl};
use crate::kit::{self, Tone};
use crate::tools::{self, Bound};
use crate::{Panel, XmlCraftApp, icons, widgets};

/// The most bytes of one document the tool compares or merges.
const MAX_BYTES: usize = 16 * 1024 * 1024;
/// The most differences the list draws.
const LISTED: usize = 1000;
const ROW: f32 = 22.0;

/// Which part of the tool is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Tab {
    #[default]
    Compare,
    Merge,
}

/// How the differences of a comparison are shown (in a wide panel; a narrow one always lists them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum View {
    #[default]
    SideBySide,
    List,
}

/// What the document being shown is compared with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Second {
    Doc(u64),
    /// The file chosen with "Choose file…" ([`CompareTool::file`]).
    File,
}

/// A file on disk to compare with: read once when chosen, never written.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FileSide {
    path: PathBuf,
    name: String,
    text: Arc<str>,
    language: Language,
}

/// What a comparison came to, and the documents (and versions of their text) it was asked on: the ranges of its differences are only
/// good for those. The second side is a file when `second_file` is set (its id and version are then 0).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Answer {
    serial: u64,
    first: (u64, u64),
    second: (u64, u64),
    second_file: bool,
    names: (String, String),
    texts: Texts,
    result: Result<Compared, String>,
}

/// The request being waited for.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Waiting {
    serial: u64,
    first: (u64, u64),
    second: (u64, u64),
    second_file: bool,
    names: (String, String),
    texts: Texts,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MergeForm {
    base: Option<u64>,
    theirs: Option<u64>,
    conflicts: Choice,
    ignore_whitespace: bool,
    error: Option<String>,
    /// The document the last merge made, while it is open: its conflicts are listed to be settled.
    result: Option<u64>,
    /// The conflicts marked in it, with the line each starts on (0-based), and the version of its text they were found in.
    marked: Vec<Marked>,
    scanned: Option<(u64, u64)>,
    /// The conflict Next conflict went to.
    current: Option<usize>,
}

impl Default for MergeForm {
    fn default() -> Self {
        Self {
            base: None,
            theirs: None,
            conflicts: Choice::Markers,
            ignore_whitespace: false,
            error: None,
            result: None,
            marked: Vec::new(),
            scanned: None,
            current: None,
        }
    }
}

/// A conflict marked in the merge result: where, the line it starts on (0-based), and the first lines of its parts (ours, base, theirs).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Marked {
    conflict: MarkedConflict,
    line: usize,
    previews: [String; 3],
}

/// What a click in the conflict list asked for.
enum MergeClick {
    Settle(usize, Choice),
    Next,
    Go(usize),
    Save,
}

/// The most conflicts the list shows (the counter counts them all).
const CONFLICTS_SHOWN: usize = 200;
/// The most lines of each part of a conflict the list shows.
const PART_LINES: usize = 6;

/// The form of the Compare tool window and what it found last.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CompareTool {
    tab: Tab,
    /// The document to compare the shown one with (by id); when it is not open, the first other one.
    second: Option<u64>,
    /// The chosen file is the second side (rather than `second`).
    use_file: bool,
    file: Option<FileSide>,
    mode: Mode,
    ignore: Ignore,
    /// Side by side or the list; saved in the preferences.
    view: View,
    side: ViewState,
    waiting: Option<Waiting>,
    answer: Option<Answer>,
    /// The comparison is made again when this document has this version (after a copy).
    again: Option<(u64, u64)>,
    merge: MergeForm,
}

impl CompareTool {
    /// The tool with its saved view.
    pub(crate) fn new(side_by_side: bool) -> Self {
        Self { view: if side_by_side { View::SideBySide } else { View::List }, ..Self::default() }
    }

    /// Whether the differences are shown side by side (saved in the preferences).
    pub(crate) fn side_by_side(&self) -> bool {
        self.view == View::SideBySide
    }
}

impl Default for CompareTool {
    fn default() -> Self {
        Self {
            tab: Tab::Compare,
            second: None,
            use_file: false,
            file: None,
            mode: Mode::Auto,
            ignore: Ignore::default(),
            view: View::SideBySide,
            side: ViewState::default(),
            waiting: None,
            answer: None,
            again: None,
            merge: MergeForm::default(),
        }
    }
}

/// Where a click on a difference goes: the document and the range in it.
struct Go {
    doc: u64,
    range: Range<usize>,
}

/// What a click in the results asked for.
enum Click {
    Go(Go),
    Copy { hunk: usize, target: Side },
}

fn glyph(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "+",
        ChangeKind::Removed => "−",
        ChangeKind::Changed => "≠",
    }
}

fn colouring(language: Language) -> Colouring {
    if language.is_json_like() {
        Colouring::Json
    } else if language.is_yaml() {
        Colouring::Yaml
    } else {
        Colouring::Xml
    }
}

/// The name of the merged document: that of ours with `-merged`.
fn merged_name(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}-merged.{ext}"),
        None => format!("{name}-merged"),
    }
}

/// The document chosen among `others`: `wanted` when it is one of them, else the first that is not `except`.
fn pick(wanted: Option<u64>, others: &[(u64, String)], except: Option<u64>) -> Option<u64> {
    wanted.filter(|w| others.iter().any(|(id, _)| id == w)).or_else(|| others.iter().map(|(id, _)| *id).find(|id| Some(*id) != except))
}

/// Whether a text that was changed still reads as a tree of its family (always so when it did not before the change).
fn still_well_formed(family: Family, before: &str, after: &str) -> bool {
    family.read(before).is_err() || family.read(after).is_ok()
}

impl XmlCraftApp {
    /// Compare Documents…: show the Compare panel on its Compare tab.
    pub(crate) fn open_compare(&mut self) {
        if self.show_tool(Panel::Compare) {
            self.compare.tab = Tab::Compare;
        }
    }

    /// Merge Documents…: show the Compare panel on its Merge tab.
    pub(crate) fn open_merge(&mut self) {
        if self.show_tool(Panel::Compare) {
            self.compare.tab = Tab::Merge;
        }
    }

    /// The text of the open document `id` and its version.
    fn text_and_version(&self, id: u64) -> Option<(String, u64, Language)> {
        let doc = self.session.position_of(id).and_then(|i| self.session.document(i))?;
        Some((doc.editor().text(), doc.editor().version(), doc.language()))
    }

    /// A file chosen with "Choose file…": read it (text only, at most [`MAX_BYTES`]) as the second side of the comparison.
    pub(crate) fn picked_compare_file(&mut self, path: &std::path::Path) {
        match super::query::read_decoded(path) {
            Ok((name, text)) if text.len() <= MAX_BYTES && !text.contains('\0') => {
                let language = Language::of_name(&path.to_string_lossy());
                self.compare.file = Some(FileSide { path: path.to_path_buf(), name, text: Arc::from(text), language });
                self.compare.use_file = true;
            }
            Ok(_) => {
                let limit = (MAX_BYTES / (1024 * 1024)).to_string();
                self.status = fmt(tl!("That file is not text, or larger than {limit} MB: it is not compared."), &[("limit", &limit)]);
            }
            Err(e) => self.status = fmt(tl!("The file cannot be read: {why}"), &[("why", &e)]),
        }
        self.compare.tab = Tab::Compare;
        self.show_tool(Panel::Compare);
    }

    fn run_compare(&mut self, first: &Bound, second: Second) {
        let Some((old, old_version, old_language)) = self.text_and_version(first.id) else {
            self.compare.answer = None;
            return;
        };
        let (new, new_language, second_at, second_name, second_file) = match second {
            Second::Doc(id) => {
                let Some((text, version, language)) = self.text_and_version(id) else {
                    self.compare.answer = None;
                    return;
                };
                let name = self.session.position_of(id).and_then(|i| self.session.document(i)).map(|d| d.name().to_owned()).unwrap_or_default();
                (text, language, (id, version), name, false)
            }
            Second::File => {
                let Some(file) = &self.compare.file else {
                    self.compare.answer = None;
                    return;
                };
                (file.text.to_string(), file.language, (0, 0), fmt(tl!("{name} (file)"), &[("name", &file.name)]), true)
            }
        };
        let first_at = (first.id, old_version);
        let names = (first.name.clone(), second_name);
        let texts =
            Texts { left: Arc::from(old.as_str()), right: Arc::from(new.as_str()), colouring: (colouring(old_language), colouring(new_language)) };
        self.generate_serial += 1;
        let serial = self.generate_serial;
        if old.len() > MAX_BYTES || new.len() > MAX_BYTES {
            let limit = (MAX_BYTES / (1024 * 1024)).to_string();
            let message = fmt(tl!("A document is larger than {limit} MB."), &[("limit", &limit)]);
            self.compare.answer = Some(Answer { serial, first: first_at, second: second_at, second_file, names, texts, result: Err(message) });
            return;
        }
        self.comparer.submit(CompareJob { serial, old, old_language, new, new_language, mode: self.compare.mode, ignore: self.compare.ignore });
        self.compare.waiting = Some(Waiting { serial, first: first_at, second: second_at, second_file, names, texts });
    }

    /// Collect what the worker finished, whatever else is open. Called every frame.
    pub(crate) fn pump_compare(&mut self) {
        for done in self.comparer.poll() {
            let Some(waiting) = self.compare.waiting.take_if(|w| w.serial == done.serial) else { continue };
            // The same documents again (after a copy) keep the place; others start at the top.
            let same = self.compare.answer.as_ref().is_some_and(|a| a.first.0 == waiting.first.0 && a.second.0 == waiting.second.0);
            if !same {
                self.compare.side.reset();
            }
            self.compare.answer = Some(Answer {
                serial: waiting.serial,
                first: waiting.first,
                second: waiting.second,
                second_file: waiting.second_file,
                names: waiting.names,
                texts: waiting.texts,
                result: done.result,
            });
            self.workbench.set_attention(Panel::Compare);
        }
    }

    /// Whether the documents of the last answer are still as they were compared.
    fn answer_fresh(&self) -> bool {
        self.compare.answer.as_ref().is_some_and(|a| {
            let open = |(id, version): (u64, u64)| {
                self.session.position_of(id).and_then(|i| self.session.document(i)).is_some_and(|d| d.editor().version() == version)
            };
            open(a.first) && (a.second_file || open(a.second))
        })
    }

    /// Copy hunk `hunk` of the last answer to the `target` side: one undoable edit of that document, after which the comparison is made
    /// again. A result that no longer reads as a tree (when it did) is said in the status bar.
    fn copy_hunk(&mut self, hunk: usize, target: Side) {
        let Some(answer) = &self.compare.answer else { return };
        let Ok(done) = &answer.result else { return };
        let Some(h) = done.aligned.hunks.get(hunk) else { return };
        let (doc_id, file) = match target {
            Side::Left => (answer.first.0, false),
            Side::Right => (answer.second.0, answer.second_file),
        };
        if file {
            return;
        }
        let edits = xmlcraft_diff::take(&done.aligned, h, target, &answer.texts.left, &answer.texts.right);
        let before = match target {
            Side::Left => answer.texts.left.clone(),
            Side::Right => answer.texts.right.clone(),
        };
        let lines = match target {
            Side::Left => h.left.clone(),
            Side::Right => h.right.clone(),
        };
        let Some(doc) = self.session.position_of(doc_id).and_then(|i| self.session.document_mut(i)) else { return };
        let family = family_of(doc.language());
        let name = doc.name().to_owned();
        if edits.is_empty() {
            return;
        }
        doc.editor_mut().break_undo_group();
        if !doc.editor_mut().apply_edits(&edits) {
            self.status = fmt(tl!("{name} changed meanwhile: compare again."), &[("name", &name)]);
            return;
        }
        doc.editor_mut().break_undo_group();
        let version = doc.editor().version();
        let after = doc.editor().text();
        let first_line = (lines.start + 1).to_string();
        self.status = if still_well_formed(family, &before, &after) {
            fmt(tl!("Copied the change into {name} at line {line}"), &[("name", &name), ("line", &first_line)])
        } else {
            fmt(
                tl!("Copied the change into {name} at line {line}; it is no longer well-formed (Undo takes the change back)"),
                &[("name", &name), ("line", &first_line)],
            )
        };
        self.compare.again = Some((doc_id, version));
    }

    /// Merge with the options of the form and open the result. False: it did not merge (the form says why).
    fn run_merge(&mut self, ours: &Bound, base: Option<u64>, theirs: Option<u64>) {
        let (Some(base_id), Some(theirs_id)) = (base, theirs) else {
            self.compare.merge.error =
                Some(tl!("A merge needs three documents: this one, the base it and the other were made from, and the other.").to_owned());
            return;
        };
        if base_id == theirs_id {
            self.compare.merge.error = Some(tl!("The base and the other document are the same one: choose two different documents.").to_owned());
            return;
        }
        let (Some((ours_text, ..)), Some((base_text, ..)), Some((theirs_text, ..))) =
            (self.text_and_version(ours.id), self.text_and_version(base_id), self.text_and_version(theirs_id))
        else {
            self.compare.merge.error = Some(tl!("A document is gone.").to_owned());
            return;
        };
        if ours_text.len().max(base_text.len()).max(theirs_text.len()) > MAX_BYTES {
            let limit = (MAX_BYTES / (1024 * 1024)).to_string();
            self.compare.merge.error = Some(fmt(tl!("A document is larger than {limit} MB."), &[("limit", &limit)]));
            return;
        }
        let ignore = self.compare.merge.ignore_whitespace;
        let options = LineOptions {
            whitespace: if ignore { Whitespace::Collapse } else { Whitespace::Exact },
            ignore_blank_lines: ignore,
            ..LineOptions::default()
        };
        let merged = merge3(&base_text, &ours_text, &theirs_text, &options);
        let conflicts = self.compare.merge.conflicts;
        let (text, count) = (merged.render(conflicts), merged.conflict_count());
        let name = merged_name(&ours.name);
        self.compare.merge.error = None;
        self.open_generated(&name, &text);
        let merge = &mut self.compare.merge;
        merge.result = self.session.active_document().map(|d| d.id());
        (merge.scanned, merge.current) = (None, None);
        merge.marked.clear();
        self.status = match (count, conflicts) {
            (0, _) => fmt(tl!("Merged into {name} with no conflicts"), &[("name", &name)]),
            (n, Choice::Markers) => {
                fmt(tl!("Merged into {name}; {count} conflicts are marked in the text"), &[("name", &name), ("count", &n.to_string())])
            }
            (n, _) => fmt(tl!("Merged into {name}; {count} conflicts were settled as chosen"), &[("name", &name), ("count", &n.to_string())]),
        };
    }

    /// Find the conflicts marked in the result of the last merge again when its text changed (and forget the result when it was closed).
    fn scan_conflicts(&mut self) {
        let merge = &mut self.compare.merge;
        let Some(id) = merge.result else { return };
        let Some(doc) = self.session.position_of(id).and_then(|i| self.session.document(i)) else {
            merge.result = None;
            merge.marked.clear();
            merge.scanned = None;
            return;
        };
        let version = doc.editor().version();
        if merge.scanned == Some((id, version)) {
            return;
        }
        let text = doc.editor().text();
        let mut line = 0usize;
        let mut at = 0usize;
        merge.marked = conflicts_in(&text)
            .into_iter()
            .map(|c| {
                line += text.get(at..c.range.start).map_or(0, |t| t.matches('\n').count());
                at = c.range.start;
                let previews = [part_preview(&text, Some(&c.ours)), part_preview(&text, c.base.as_ref()), part_preview(&text, Some(&c.theirs))];
                Marked { conflict: c, line, previews }
            })
            .collect();
        merge.scanned = Some((id, version));
        merge.current = merge.current.filter(|c| *c < merge.marked.len());
    }

    /// Settle conflict `n` of the merge result as `choice` says: the marked lines are replaced by that part, as one undoable edit.
    fn settle_conflict(&mut self, n: usize, choice: Choice) {
        self.scan_conflicts();
        let merge = &self.compare.merge;
        let (Some(id), Some(marked)) = (merge.result, merge.marked.get(n)) else { return };
        let conflict = marked.conflict.clone();
        let Some(doc) = self.session.position_of(id).and_then(|i| self.session.document_mut(i)) else { return };
        let text = doc.editor().text();
        let edit = xmlcraft_text::Edit { range: conflict.range.clone(), text: conflict.settled(&text, choice) };
        doc.editor_mut().break_undo_group();
        if doc.editor_mut().apply_edits(&[edit]) {
            doc.editor_mut().break_undo_group();
        }
        self.scan_conflicts();
        let left = self.compare.merge.marked.len();
        self.status = if left == 0 {
            tl!("Every conflict is settled: save the result when it is right.").to_owned()
        } else {
            fmt(tl!("Conflict settled; {count} left"), &[("count", &left.to_string())])
        };
    }

    /// Show conflict `n` of the merge result: its marked lines selected.
    fn show_conflict(&mut self, n: usize) {
        let (Some(id), Some(marked)) = (self.compare.merge.result, self.compare.merge.marked.get(n)) else { return };
        let range = marked.conflict.range.clone();
        self.compare.merge.current = Some(n);
        self.go(Go { doc: id, range });
    }

    /// Select a range in the document `id` (shown first).
    fn go(&mut self, go: Go) {
        if let Some(index) = self.session.position_of(go.doc) {
            self.session.set_active(Some(index));
            self.jump_to(go.range);
        }
    }
}

/// The body of the Compare panel.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    let bound = tools::bound(app);
    // The other open documents, from the session at this frame.
    let others: Vec<(u64, String)> = match &bound {
        Some(b) => app.session.documents().iter().filter(|d| d.id() != b.id).map(|d| (d.id(), d.name().to_owned())).collect(),
        None => Vec::new(),
    };
    let second_doc = pick(app.compare.second, &others, None);
    let second = if app.compare.use_file && app.compare.file.is_some() { Some(Second::File) } else { second_doc.map(Second::Doc) };
    let base = pick(app.compare.merge.base, &others, None);
    let theirs = pick(app.compare.merge.theirs, &others, base);
    let (primary, can, why_not) = match app.compare.tab {
        Tab::Compare => (
            tl!("Compare Documents"),
            bound.is_some() && second.is_some() && app.compare.waiting.is_none(),
            tl!("Open a second document to compare this one with."),
        ),
        Tab::Merge => {
            (tl!("Merge Documents"), bound.is_some() && others.len() >= 2, tl!("A merge needs three documents: this one, the base, and the other."))
        }
    };
    // A copy changed a document: compare again once the edit is in.
    if let Some((id, version)) = app.compare.again
        && app.compare.waiting.is_none()
        && app.text_and_version(id).is_some_and(|(_, v, _)| v == version)
    {
        app.compare.again = None;
        if let (Some(answer), Some(b)) = (app.compare.answer.clone(), &bound) {
            // The first side is the document that was first then, whichever is shown now.
            let first_name = app.session.position_of(answer.first.0).and_then(|i| app.session.document(i)).map(|d| d.name().to_owned());
            if let Some(first_name) = first_name {
                let first = Bound { id: answer.first.0, name: first_name, xml: b.xml };
                let second = if answer.second_file { Second::File } else { Second::Doc(answer.second.0) };
                app.run_compare(&first, second);
            }
        }
    }
    app.scan_conflicts();
    let result_name: Option<String> =
        app.compare.merge.result.and_then(|id| app.session.position_of(id)).and_then(|i| app.session.document(i)).map(|d| d.name().to_owned());
    let mut merge_click = None;
    let wide = !kit::narrow(ui);
    let mut run = !wide && kit::tool_footer(ui, primary, Some("git-compare"), can, why_not);
    let fresh = app.answer_fresh();
    let mut click = None;
    let mut choose_file = false;
    let shown = bound.clone();
    if wide {
        let (clicked, ran, chose, merged) =
            wide_body(ui, app, &shown, &others, (second, base, theirs), (primary, can, fresh), result_name.as_deref());
        click = clicked;
        merge_click = merged;
        run = ran;
        choose_file = chose;
    } else {
        kit::tool_body(ui, false, |ui| {
            kit::segmented(ui, &mut app.compare.tab, &[(Tab::Compare, tl!("Compare")), (Tab::Merge, tl!("Merge"))]);
            ui.add_space(kit::GAP);
            let Some(bound) = &shown else {
                tools::needs(ui, tl!("Compare needs a document"), tl!("Open two documents to compare them, or three to merge."));
                return;
            };
            tools::bound_line(ui, &bound.name);
            ui.add_space(kit::GAP);
            match app.compare.tab {
                Tab::Compare => {
                    let (chose, clicked) = tools::columns(
                        ui,
                        &mut app.compare,
                        |ui, tool| compare_form(ui, tool, &others, second),
                        |ui, tool| compare_results(ui, tool, bound.id, fresh, false),
                    );
                    click = clicked;
                    choose_file = chose;
                }
                Tab::Merge => {
                    merge_form(ui, &mut app.compare.merge, &others, base, theirs);
                    if let Some(name) = &result_name {
                        ui.add_space(kit::GAP);
                        merge_click = merge_results(ui, &app.compare.merge, name);
                    }
                }
            }
        });
    }
    if choose_file {
        app.pick_compare_file(ui.ctx());
    }
    match click {
        Some(Click::Go(go)) => app.go(go),
        Some(Click::Copy { hunk, target }) => app.copy_hunk(hunk, target),
        None => {}
    }
    match merge_click {
        Some(MergeClick::Settle(n, choice)) => app.settle_conflict(n, choice),
        Some(MergeClick::Go(n)) => app.show_conflict(n),
        Some(MergeClick::Next) => {
            let count = app.compare.merge.marked.len();
            if count > 0 {
                let next = app.compare.merge.current.map_or(0, |c| (c + 1) % count);
                app.show_conflict(next);
            }
        }
        Some(MergeClick::Save) => {
            if let Some(id) = app.compare.merge.result {
                app.save_document(ui.ctx(), id, None);
            }
        }
        None => {}
    }
    if run
        && can
        && let Some(bound) = &bound
    {
        match app.compare.tab {
            Tab::Compare => {
                if let Some(second) = second {
                    app.run_compare(bound, second);
                }
            }
            Tab::Merge => app.run_merge(bound, base, theirs),
        }
    }
}

/// A combo box over the other documents. `value` is the one chosen.
fn document_combo(ui: &mut egui::Ui, salt: &str, name: &str, others: &[(u64, String)], value: Option<u64>, max_width: f32) -> Option<u64> {
    let mut chosen = value;
    let current = others.iter().find(|(id, _)| Some(*id) == value).map_or("", |(_, n)| n.as_str()).to_owned();
    let width = (ui.available_width() - 8.0).clamp(120.0, max_width);
    let combo = egui::ComboBox::from_id_salt(salt).selected_text(current).width(width).show_ui(ui, |ui| {
        for (id, label) in others {
            if ui.selectable_label(Some(*id) == value, label).clicked() {
                chosen = Some(*id);
            }
        }
    });
    widgets::name_for_assistive_technology(ui, &combo.response, name);
    chosen
}

/// The combo box of what to compare with: the other open documents, the file chosen before, and "Choose file…". Returns whether
/// "Choose file…" was clicked.
fn second_combo(ui: &mut egui::Ui, tool: &mut CompareTool, others: &[(u64, String)], second: Option<Second>, max_width: f32) -> bool {
    let file_label = tool.file.as_ref().map(|f| fmt(tl!("{name} (file)"), &[("name", &f.name)]));
    let current = match second {
        Some(Second::File) => file_label.clone().unwrap_or_default(),
        Some(Second::Doc(id)) => others.iter().find(|(i, _)| *i == id).map(|(_, n)| n.clone()).unwrap_or_default(),
        None => String::new(),
    };
    let width = (ui.available_width() - 8.0).clamp(120.0, max_width);
    let mut choose = false;
    let combo = egui::ComboBox::from_id_salt("xmlcraft-compare-second").selected_text(current).width(width).show_ui(ui, |ui| {
        for (id, label) in others {
            if ui.selectable_label(second == Some(Second::Doc(*id)), label).clicked() {
                tool.second = Some(*id);
                tool.use_file = false;
            }
        }
        if let Some(label) = &file_label
            && ui.selectable_label(second == Some(Second::File), label.as_str()).clicked()
        {
            tool.use_file = true;
        }
        ui.separator();
        if ui.selectable_label(false, tl!("Choose file…")).clicked() {
            choose = true;
        }
    });
    widgets::name_for_assistive_technology(ui, &combo.response, tl!("The document to compare with"));
    choose
}

fn caption(ui: &mut egui::Ui, text: &str) {
    let t = crate::theme::Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(text).font(crate::theme::medium(11.5)).color(t.text_muted));
}

/// The narrow form. Returns whether "Choose file…" was clicked.
fn compare_form(ui: &mut egui::Ui, tool: &mut CompareTool, others: &[(u64, String)], second: Option<Second>) -> bool {
    caption(ui, tl!("Compare with"));
    if others.is_empty() && tool.file.is_none() {
        kit::help(ui, tl!("Open a second document to compare this one with."));
    }
    let choose = second_combo(ui, tool, others, second, f32::INFINITY);
    ui.add_space(4.0);
    kit::segmented(ui, &mut tool.mode, &[(Mode::Auto, tl!("Automatic")), (Mode::Tree, tl!("Elements and members")), (Mode::Lines, tl!("Lines"))]);
    ui.add_space(kit::GAP);
    caption(ui, tl!("Do not count as a difference"));
    ignore_options(ui, tool);
    choose
}

/// The things that do not count as a difference, as check boxes (stacked, or inline in a wrapping row).
fn ignore_options(ui: &mut egui::Ui, tool: &mut CompareTool) {
    ui.add(widgets::checkbox(&mut tool.ignore.whitespace, tl!("White space")));
    ui.add(widgets::checkbox(&mut tool.ignore.case, tl!("Upper and lower case")));
    if tool.mode != Mode::Lines {
        ui.add(widgets::checkbox(&mut tool.ignore.comments, tl!("Comments")));
        ui.add(widgets::checkbox(&mut tool.ignore.child_order, tl!("Order of elements and items")));
        ui.add(widgets::checkbox(&mut tool.ignore.numbers_by_value, tl!("Number format (1.0 and 1)")));
        ui.add(widgets::checkbox(&mut tool.ignore.prefixes, tl!("Namespace prefixes")));
    }
}

/// The widest a document combo box is in the wide layout's toolbar, and how much of the row the rest of it takes.
const COMBO_MAX: f32 = 260.0;
const TOOLBAR_REST: f32 = 800.0;
/// A button in the toolbar that opens a small popup with options; it says how many are on (`Options (2)`).
fn options_button(ui: &mut egui::Ui, on: usize, add: impl FnOnce(&mut egui::Ui)) {
    let label = if on == 0 { tl!("Options").to_owned() } else { fmt(tl!("Options ({count})"), &[("count", &on.to_string())]) };
    let t = crate::theme::Tokens::get(ui.ctx());
    let response = ui.add(egui::Button::new((label, icons::image("chevron-down", 12.0, t.icon))));
    egui::Popup::from_toggle_button_response(&response).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_min_width(240.0);
        add(ui);
    });
}

/// How many of the things that do not count as a difference are on (those the mode reads).
fn ignored(tool: &CompareTool) -> usize {
    let i = tool.ignore;
    let lines = [i.whitespace, i.case];
    let trees = [i.comments, i.child_order, i.numbers_by_value, i.prefixes];
    lines.iter().filter(|b| **b).count() + if tool.mode == Mode::Lines { 0 } else { trees.iter().filter(|b| **b).count() }
}

/// The wide layout (the usual bottom dock): one toolbar, wrapping only when it does not fit, with the primary pill at its right end,
/// and under it the whole width and the rest of the height for the differences. Returns what a click asked for, whether the pill was
/// pressed and whether "Choose file…" was.
fn wide_body(
    ui: &mut egui::Ui,
    app: &mut XmlCraftApp,
    shown: &Option<Bound>,
    others: &[(u64, String)],
    (second, base, theirs): (Option<Second>, Option<u64>, Option<u64>),
    (primary, can, fresh): (&str, bool, bool),
    result_name: Option<&str>,
) -> (Option<Click>, bool, bool, Option<MergeClick>) {
    let (mut click, mut run, mut choose, mut merged) = (None, false, false, None);
    kit::tool_body(ui, false, |ui| {
        let Some(bound) = shown else {
            kit::segmented(ui, &mut app.compare.tab, &[(Tab::Compare, tl!("Compare")), (Tab::Merge, tl!("Merge"))]);
            tools::needs(ui, tl!("Compare needs a document"), tl!("Open two documents to compare them, or three to merge."));
            return;
        };
        let t = crate::theme::Tokens::get(ui.ctx());
        let pill_width = widgets::text_width(ui, primary, &crate::theme::medium(12.5)) + 64.0;
        let (top, right) = (ui.cursor().min, ui.max_rect().right());
        let pill = egui::Rect::from_min_size(egui::pos2(right - pill_width, top.y), egui::vec2(pill_width, kit::TOOL_CONTROL));
        // (A child that takes no room: the toolbar starts at the same top.)
        let mut pill_ui = ui.new_child(egui::UiBuilder::new().max_rect(pill));
        run = pill_ui
            .add_enabled_ui(can, |ui| kit::pill_sized(ui, kit::PillKind::Primary, Some("git-compare"), primary, kit::TOOL_CONTROL))
            .inner
            .clicked();
        let width = (right - top.x - pill_width - kit::GAP).max(100.0);
        // The combo boxes give way first when the row is short (two share it on the Merge tab).
        let combo_max = ((width - TOOLBAR_REST) / if app.compare.tab == Tab::Merge { 2.0 } else { 1.0 }).clamp(140.0, COMBO_MAX);
        // One row: the options are in a popup, so the differences get the height.
        ui.allocate_ui(egui::vec2(width, kit::TOOL_CONTROL), |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            ui.horizontal(|ui| {
                kit::segmented(ui, &mut app.compare.tab, &[(Tab::Compare, tl!("Compare")), (Tab::Merge, tl!("Merge"))]);
                ui.label(egui::RichText::new(format!("→ {}", bound.name)).font(crate::theme::medium(12.0)).color(t.text_muted));
                match app.compare.tab {
                    Tab::Compare => {
                        caption(ui, tl!("with"));
                        if others.is_empty() && app.compare.file.is_none() {
                            kit::help(ui, tl!("Open a second document to compare this one with."));
                        }
                        choose = second_combo(ui, &mut app.compare, others, second, combo_max);
                        let tool = &mut app.compare;
                        kit::segmented(
                            ui,
                            &mut tool.mode,
                            &[(Mode::Auto, tl!("Automatic")), (Mode::Tree, tl!("Elements and members")), (Mode::Lines, tl!("Lines"))],
                        );
                        let on = ignored(tool);
                        options_button(ui, on, |ui| {
                            caption(ui, tl!("Do not count as a difference"));
                            ignore_options(ui, tool);
                        });
                    }
                    Tab::Merge if others.len() >= 2 => {
                        let merge = &mut app.compare.merge;
                        caption(ui, tl!("Base"));
                        merge.base = document_combo(ui, "xmlcraft-merge-base", tl!("The base document"), others, base, combo_max);
                        caption(ui, tl!("Theirs"));
                        merge.theirs = document_combo(ui, "xmlcraft-merge-theirs", tl!("The other document"), others, theirs, combo_max);
                        let on = usize::from(merge.conflicts != Choice::Markers) + usize::from(merge.ignore_whitespace);
                        options_button(ui, on, |ui| {
                            caption(ui, tl!("Where both changed the same lines"));
                            kit::segmented(
                                ui,
                                &mut merge.conflicts,
                                &[
                                    (Choice::Markers, tl!("Mark")),
                                    (Choice::Ours, tl!("Ours")),
                                    (Choice::Theirs, tl!("Theirs")),
                                    (Choice::Both, tl!("Both")),
                                    (Choice::Base, tl!("Base")),
                                ],
                            );
                            ui.add_space(4.0);
                            ui.add(widgets::checkbox(&mut merge.ignore_whitespace, tl!("White space does not count as a change")));
                        });
                    }
                    Tab::Merge => {}
                }
            });
        });
        ui.add_space(4.0);
        match app.compare.tab {
            Tab::Compare => click = compare_results(ui, &mut app.compare, bound.id, fresh, true),
            Tab::Merge if others.len() >= 2 || result_name.is_some() => {
                if let Some(message) = &app.compare.merge.error {
                    kit::banner(ui, Tone::Bad, message);
                }
                if let Some(name) = result_name {
                    merged = merge_results(ui, &app.compare.merge, name);
                }
            }
            Tab::Merge => tools::needs(
                ui,
                tl!("A merge needs three documents"),
                tl!("This one, the base it and the other were made from, and the other. Open them all."),
            ),
        }
    });
    (click, run, choose, merged)
}

/// The differences of the last comparison, filling the rest of the panel: side by side (`wide`, unless the list is chosen) or listed.
/// Returns what a click asked for. Shown while the document in front is one of the two that were compared.
fn compare_results(ui: &mut egui::Ui, tool: &mut CompareTool, shown: u64, fresh: bool, wide: bool) -> Option<Click> {
    if !wide {
        ui.add_space(kit::GAP);
    }
    if tool.waiting.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Comparing…"));
        });
    }
    let Some(answer) = tool.answer.as_ref().filter(|a| a.first.0 == shown || (!a.second_file && a.second.0 == shown)) else {
        if tool.waiting.is_none() {
            kit::help(ui, tl!("Choose the other document and press Compare Documents. The differences are listed here."));
        }
        return None;
    };
    let done = match &answer.result {
        Err(message) => {
            kit::banner(ui, Tone::Bad, message);
            return None;
        }
        Ok(done) => done,
    };
    let side_by_side = wide && tool.view == View::SideBySide && !done.aligned.too_large;
    let count = done.count.to_string();
    let summary = if done.count == 0 {
        tl!("The documents are the same.").to_owned()
    } else if side_by_side {
        fmt(tl!("{count} differences. A click on a row selects it in that document, a right click in the second."), &[("count", &count)])
    } else if done.count > done.differences.len().min(LISTED) {
        fmt(
            tl!("{count} differences; the first {shown} are listed. A click selects one in the first document, a right click in the second."),
            &[("count", &count), ("shown", &done.differences.len().min(LISTED).to_string())],
        )
    } else {
        fmt(tl!("{count} differences. A click selects one in the first document, a right click in the second."), &[("count", &count)])
    };
    let mut click = None;
    let against = fmt(tl!("{first} against {second}"), &[("first", &answer.names.0), ("second", &answer.names.1)]);
    // One row in a wide panel (the summary at its right end), wrapping in a narrow one.
    let row = |ui: &mut egui::Ui| {
        if wide {
            kit::segmented(ui, &mut tool.view, &[(View::SideBySide, tl!("Side by side")), (View::List, tl!("List"))]);
        }
        if side_by_side {
            let (at, total) = tool.side.counter(&done.aligned);
            let enabled = total > 0;
            let up = ui.add_enabled_ui(enabled, |ui| icons::button(ui, "chevron-up", 24.0, false, tl!("Previous change"))).inner;
            let down = ui.add_enabled_ui(enabled, |ui| icons::button(ui, "chevron-down", 24.0, false, tl!("Next change"))).inner;
            if up.clicked() {
                tool.side.step(&done.aligned, true);
            }
            if down.clicked() {
                tool.side.step(&done.aligned, false);
            }
            let (at, total) = match tool.side.counter(&done.aligned) {
                (Some(a), t) => (Some(a), t),
                _ => (at, total),
            };
            let counter = match at {
                Some(a) => fmt(tl!("{at} of {total}"), &[("at", &a.to_string()), ("total", &total.to_string())]),
                None => fmt(tl!("{total} changes"), &[("total", &total.to_string())]),
            };
            ui.label(egui::RichText::new(counter).font(crate::theme::medium(12.0)));
        }
        if wide {
            // The summary at the right end of the row (what a click does is its tooltip), so the differences start under it.
            let (short, hint) = match summary.split_once(". ") {
                Some((count, hint)) => (format!("{count}. {against}"), hint.to_owned()),
                None => (format!("{summary} {against}"), String::new()),
            };
            let t = crate::theme::Tokens::get(ui.ctx());
            let text = egui::RichText::new(short.as_str()).size(11.5).color(t.text_muted);
            let width = widgets::text_width(ui, &short, &egui::FontId::proportional(11.5));
            // At the right end when it fits (a narrow row wraps it under the rest).
            let room = ui.available_width() - width - 2.0 * ui.spacing().item_spacing.x - 6.0;
            if room > 0.0 {
                ui.add_space(room);
            }
            let label = ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Extend));
            if !hint.is_empty() {
                label.on_hover_text(hint);
            }
        } else {
            kit::help(ui, &against);
        }
    };
    if wide {
        ui.horizontal(row);
    } else {
        ui.horizontal_wrapped(row);
    }
    if let Some(note) = &done.note {
        kit::help(ui, note);
    }
    if !wide {
        kit::help(ui, &summary);
    }
    if done.aligned.too_large && wide && tool.view == View::SideBySide {
        let limit = xmlcraft_diff::MAX_LINES.to_string();
        kit::help(
            ui,
            &fmt(tl!("A document has more than {limit} lines: too many to show side by side, so the differences are listed."), &[("limit", &limit)]),
        );
    }
    // The ranges are good for the text that was compared: after an edit the list stays but does not point.
    if !fresh {
        kit::help(ui, tl!("A document changed since: compare again."));
    }
    let (first, second) = (answer.first, answer.second);
    if side_by_side {
        let file_why = tl!("The second side is a file on disk: open it to change it.");
        let permits = Permits { to_left: Ok(()), to_right: if answer.second_file { Err(file_why) } else { Ok(()) } };
        match side_by_side::show(ui, &mut tool.side, &done.aligned, &answer.texts, answer.serial, &permits, fresh) {
            Some(Action::Copy { hunk, target }) => click = Some(Click::Copy { hunk, target }),
            Some(Action::Go(side, line)) => {
                let (doc, lines, text) = match side {
                    Side::Left => (first.0, &done.aligned.left_lines, &answer.texts.left),
                    Side::Right => (second.0, &done.aligned.right_lines, &answer.texts.right),
                };
                if !(side == Side::Right && answer.second_file) {
                    let range = lines.get(line).cloned().unwrap_or(text.len()..text.len());
                    click = Some(Click::Go(Go { doc, range }));
                }
            }
            None => {}
        }
        return click;
    }
    let t = crate::theme::Tokens::get(ui.ctx());
    egui::ScrollArea::vertical().id_salt("xmlcraft-compare-differences").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for difference in done.differences.iter().take(LISTED) {
            let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), egui::Sense::click());
            let text = format!("{}  {}", glyph(difference.kind), difference.label.replace(['\n', '\r'], " "));
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, text.clone()));
            if response.hovered() {
                ui.painter().rect_filled(rect, 4.0, t.hover);
            }
            ui.painter().with_clip_rect(rect).text(
                rect.left_center() + egui::vec2(6.0, 0.0),
                egui::Align2::LEFT_CENTER,
                &text,
                egui::FontId::monospace(12.5),
                t.text,
            );
            if !fresh {
                continue;
            }
            if response.clicked() {
                click = Some(Click::Go(Go { doc: first.0, range: difference.old.clone() }));
            } else if response.secondary_clicked() && !answer.second_file {
                click = Some(Click::Go(Go { doc: second.0, range: difference.new.clone() }));
            }
        }
    });
    click
}

/// The first lines of a part of a conflict, for the list (`(none)` for an empty part).
fn part_preview(text: &str, range: Option<&Range<usize>>) -> String {
    let part = range.and_then(|r| text.get(r.clone())).unwrap_or_default();
    if part.is_empty() {
        return tl!("(nothing)").to_owned();
    }
    let lines: Vec<&str> = part.lines().collect();
    let mut shown: String = lines.iter().take(PART_LINES).map(|l| l.chars().take(200).collect::<String>()).collect::<Vec<_>>().join("\n");
    if lines.len() > PART_LINES {
        shown.push_str(&fmt(tl!("\n… {count} more lines"), &[("count", &(lines.len() - PART_LINES).to_string())]));
    }
    shown
}

/// The conflicts of the last merge's result, each with its parts and the buttons that settle it, under a counter, Next conflict and
/// Save result. Returns what was clicked.
fn merge_results(ui: &mut egui::Ui, form: &MergeForm, name: &str) -> Option<MergeClick> {
    let mut click = None;
    let left = form.marked.len();
    ui.horizontal_wrapped(|ui| {
        let counter =
            if left == 0 { tl!("No conflicts left").to_owned() } else { fmt(tl!("{count} conflicts left"), &[("count", &left.to_string())]) };
        ui.label(egui::RichText::new(counter).font(crate::theme::medium(12.5)));
        if ui.add_enabled_ui(left > 0, |ui| icons::button(ui, "chevron-down", 24.0, false, tl!("Next conflict"))).inner.clicked() {
            click = Some(MergeClick::Next);
        }
        if kit::pill_sized(ui, kit::PillKind::Secondary, Some("save"), tl!("Save result"), kit::TOOL_CONTROL).clicked() {
            click = Some(MergeClick::Save);
        }
        kit::help(ui, &fmt(tl!("in {name}: each button replaces the marked lines of its conflict (Undo takes it back)."), &[("name", name)]));
    });
    if left == 0 {
        return click;
    }
    let t = crate::theme::Tokens::get(ui.ctx());
    egui::ScrollArea::vertical().id_salt("xmlcraft-merge-conflicts").auto_shrink([false, false]).show(ui, |ui| {
        for (n, marked) in form.marked.iter().enumerate().take(CONFLICTS_SHOWN) {
            kit::card(ui, form.current == Some(n), |ui| {
                ui.horizontal(|ui| {
                    let title = fmt(tl!("Conflict {n} · line {line}"), &[("n", &(n + 1).to_string()), ("line", &(marked.line + 1).to_string())]);
                    if ui.link(egui::RichText::new(title).font(crate::theme::medium(12.5))).clicked() {
                        click = Some(MergeClick::Go(n));
                    }
                    for (choice, label) in [
                        (Choice::Ours, tl!("Take ours")),
                        (Choice::Theirs, tl!("Take theirs")),
                        (Choice::Both, tl!("Take both")),
                        (Choice::Base, tl!("Take base")),
                    ] {
                        let enabled = choice != Choice::Base || marked.conflict.base.is_some();
                        if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                            click = Some(MergeClick::Settle(n, choice));
                        }
                    }
                });
                ui.columns(3, |columns| {
                    for (column, (title, preview)) in
                        columns.iter_mut().zip([tl!("Ours"), tl!("Base"), tl!("Theirs")].into_iter().zip(&marked.previews))
                    {
                        column.label(egui::RichText::new(title).font(crate::theme::medium(11.5)).color(t.text_muted));
                        column.spacing_mut().item_spacing.y = 0.0;
                        for line in preview.lines() {
                            column.add(
                                egui::Label::new(egui::RichText::new(line).font(egui::FontId::monospace(12.0)).color(t.text))
                                    .wrap_mode(egui::TextWrapMode::Truncate),
                            );
                        }
                    }
                });
            });
            ui.add_space(4.0);
        }
        if left > CONFLICTS_SHOWN {
            kit::help(ui, &fmt(tl!("The first {shown} are listed; settle them to see the rest."), &[("shown", &CONFLICTS_SHOWN.to_string())]));
        }
    });
    click
}

fn merge_form(ui: &mut egui::Ui, form: &mut MergeForm, others: &[(u64, String)], base: Option<u64>, theirs: Option<u64>) {
    if others.len() < 2 {
        tools::needs(
            ui,
            tl!("A merge needs three documents"),
            tl!("This one, the base it and the other were made from, and the other. Open them all."),
        );
        return;
    }
    caption(ui, tl!("Base"));
    form.base = document_combo(ui, "xmlcraft-merge-base", tl!("The base document"), others, base, f32::INFINITY);
    ui.add_space(4.0);
    caption(ui, tl!("Theirs"));
    form.theirs = document_combo(ui, "xmlcraft-merge-theirs", tl!("The other document"), others, theirs, f32::INFINITY);
    ui.add_space(kit::GAP);
    caption(ui, tl!("Where both changed the same lines"));
    kit::segmented(
        ui,
        &mut form.conflicts,
        &[
            (Choice::Markers, tl!("Mark")),
            (Choice::Ours, tl!("Ours")),
            (Choice::Theirs, tl!("Theirs")),
            (Choice::Both, tl!("Both")),
            (Choice::Base, tl!("Base")),
        ],
    );
    ui.add_space(4.0);
    ui.add(widgets::checkbox(&mut form.ignore_whitespace, tl!("White space does not count as a change")));
    if let Some(message) = &form.error {
        ui.add_space(kit::GAP);
        kit::banner(ui, Tone::Bad, message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_of_difference_has_its_own_mark() {
        let marks = [glyph(ChangeKind::Added), glyph(ChangeKind::Removed), glyph(ChangeKind::Changed)];
        assert_eq!(marks, ["+", "−", "≠"]);
    }

    #[test]
    fn the_merged_document_is_named_after_ours() {
        assert_eq!(merged_name("order.xml"), "order-merged.xml");
        assert_eq!(merged_name("a.b.json"), "a.b-merged.json");
        assert_eq!(merged_name("Untitled-1"), "Untitled-1-merged");
    }

    #[test]
    fn a_chosen_document_that_is_gone_gives_way_to_the_first_other_one() {
        let others = vec![(1, "a".to_owned()), (2, "b".to_owned()), (3, "c".to_owned())];
        assert_eq!(pick(Some(2), &others, None), Some(2));
        assert_eq!(pick(Some(9), &others, None), Some(1), "closed meanwhile");
        assert_eq!(pick(None, &others, Some(1)), Some(2), "the default skips the base");
        assert_eq!(pick(Some(1), &others, Some(1)), Some(1), "a choice is kept even when it repeats the base: the merge says so");
        assert_eq!(pick(None, &[], None), None);
    }

    #[test]
    fn a_copy_that_breaks_a_well_formed_text_is_noticed_and_one_that_was_broken_is_not() {
        assert!(still_well_formed(Family::Xml, "<a/>", "<b/>"));
        assert!(!still_well_formed(Family::Xml, "<a/>", "<a>"));
        assert!(still_well_formed(Family::Xml, "<a>", "<a><b>"), "it was not well-formed before");
        assert!(!still_well_formed(Family::Json, "{}", "{"));
    }
}
