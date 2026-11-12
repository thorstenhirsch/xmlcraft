//! Compare, the tool window of comparing and merging documents.
//!
//! **Compare**: the document being shown against another open document, as trees (XML elements, JSON and YAML members) or line by line
//! (`xmlcraft_engine::compare`, on a worker thread). The differences are listed and fill the panel; a click selects one in the first
//! document, a right click in the second; the panel stays open.
//!
//! **Merge**: the document being shown (ours) and another open document (theirs), both made from a third (the base), merged line by
//! line (`xmlcraft_diff::merge3`) into a new document. What both changed in the same place is a conflict, settled as asked: kept as
//! markers (`<<<<<<<`, `|||||||`, `=======`, `>>>>>>>`) to be settled in the text, or taken from ours, theirs, both or the base.
//!
//! The other documents are the open ones, listed again at every frame. Nothing here changes a document.

use std::ops::Range;

use xmlcraft_diff::{ChangeKind, Choice, LineOptions, Whitespace, merge3};
use xmlcraft_engine::compare::{CompareJob, Compared, Ignore, Mode};

use crate::i18n::{fmt, tl};
use crate::kit::{self, Tone};
use crate::tools::{self, Bound};
use crate::{Panel, XmlCraftApp, widgets};

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

/// What a comparison came to, and the documents (and versions of their text) it was asked on: the ranges of its differences are only
/// good for those.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Answer {
    first: (u64, u64),
    second: (u64, u64),
    names: (String, String),
    result: Result<Compared, String>,
}

/// The request being waited for.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Waiting {
    serial: u64,
    first: (u64, u64),
    second: (u64, u64),
    names: (String, String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MergeForm {
    base: Option<u64>,
    theirs: Option<u64>,
    conflicts: Choice,
    ignore_whitespace: bool,
    error: Option<String>,
}

impl Default for MergeForm {
    fn default() -> Self {
        Self { base: None, theirs: None, conflicts: Choice::Markers, ignore_whitespace: false, error: None }
    }
}

/// The form of the Compare tool window and what it found last.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CompareTool {
    tab: Tab,
    /// The document to compare the shown one with (by id); when it is not open, the first other one.
    second: Option<u64>,
    mode: Mode,
    ignore: Ignore,
    waiting: Option<Waiting>,
    answer: Option<Answer>,
    merge: MergeForm,
}

impl Default for CompareTool {
    fn default() -> Self {
        Self {
            tab: Tab::Compare,
            second: None,
            mode: Mode::Auto,
            ignore: Ignore::default(),
            waiting: None,
            answer: None,
            merge: MergeForm::default(),
        }
    }
}

/// Where a click on a difference goes: the document and the range in it.
struct Go {
    doc: u64,
    range: Range<usize>,
}

fn glyph(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Added => "+",
        ChangeKind::Removed => "−",
        ChangeKind::Changed => "≠",
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
    fn text_and_version(&self, id: u64) -> Option<(String, u64, xmlcraft_engine::language::Language)> {
        let doc = self.session.position_of(id).and_then(|i| self.session.document(i))?;
        Some((doc.editor().text(), doc.editor().version(), doc.language()))
    }

    fn run_compare(&mut self, first: &Bound, second: u64) {
        let (Some((old, old_version, old_language)), Some((new, new_version, new_language))) =
            (self.text_and_version(first.id), self.text_and_version(second))
        else {
            self.compare.answer = None;
            return;
        };
        let second_name = self.session.position_of(second).and_then(|i| self.session.document(i)).map(|d| d.name().to_owned()).unwrap_or_default();
        let (first_at, second_at) = ((first.id, old_version), (second, new_version));
        let names = (first.name.clone(), second_name);
        if old.len() > MAX_BYTES || new.len() > MAX_BYTES {
            let limit = (MAX_BYTES / (1024 * 1024)).to_string();
            let message = fmt(tl!("A document is larger than {limit} MB."), &[("limit", &limit)]);
            self.compare.answer = Some(Answer { first: first_at, second: second_at, names, result: Err(message) });
            return;
        }
        self.generate_serial += 1;
        let serial = self.generate_serial;
        self.comparer.submit(CompareJob { serial, old, old_language, new, new_language, mode: self.compare.mode, ignore: self.compare.ignore });
        self.compare.waiting = Some(Waiting { serial, first: first_at, second: second_at, names });
    }

    /// Collect what the worker finished, whatever else is open. Called every frame.
    pub(crate) fn pump_compare(&mut self) {
        for done in self.comparer.poll() {
            let Some(waiting) = self.compare.waiting.take_if(|w| w.serial == done.serial) else { continue };
            self.compare.answer = Some(Answer { first: waiting.first, second: waiting.second, names: waiting.names, result: done.result });
            self.workbench.set_attention(Panel::Compare);
        }
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
        self.status = match (count, conflicts) {
            (0, _) => fmt(tl!("Merged into {name} with no conflicts"), &[("name", &name)]),
            (n, Choice::Markers) => {
                fmt(tl!("Merged into {name}; {count} conflicts are marked in the text"), &[("name", &name), ("count", &n.to_string())])
            }
            (n, _) => fmt(tl!("Merged into {name}; {count} conflicts were settled as chosen"), &[("name", &name), ("count", &n.to_string())]),
        };
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
    let second = pick(app.compare.second, &others, None);
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
    let run = kit::tool_footer(ui, primary, Some("git-compare"), can, why_not);
    let fresh = app.compare.answer.as_ref().is_some_and(|a| {
        [a.first, a.second]
            .iter()
            .all(|(id, version)| app.session.position_of(*id).and_then(|i| app.session.document(i)).is_some_and(|d| d.editor().version() == *version))
    });
    let mut go = None;
    let shown = bound.clone();
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
                let ((), clicked) = tools::columns(
                    ui,
                    &mut app.compare,
                    |ui, tool| compare_form(ui, tool, &others, second),
                    |ui, tool| compare_results(ui, tool, bound.id, fresh),
                );
                go = clicked;
            }
            Tab::Merge => merge_form(ui, &mut app.compare.merge, &others, base, theirs),
        }
    });
    if let Some(go) = go {
        app.go(go);
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
fn document_combo(ui: &mut egui::Ui, salt: &str, name: &str, others: &[(u64, String)], value: Option<u64>) -> Option<u64> {
    let mut chosen = value;
    let current = others.iter().find(|(id, _)| Some(*id) == value).map_or("", |(_, n)| n.as_str()).to_owned();
    let width = (ui.available_width() - 8.0).max(120.0);
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

fn caption(ui: &mut egui::Ui, text: &str) {
    let t = crate::theme::Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(text).font(crate::theme::medium(11.5)).color(t.text_muted));
}

fn compare_form(ui: &mut egui::Ui, tool: &mut CompareTool, others: &[(u64, String)], second: Option<u64>) {
    caption(ui, tl!("Compare with"));
    if others.is_empty() {
        kit::help(ui, tl!("Open a second document to compare this one with."));
    } else {
        tool.second = document_combo(ui, "xmlcraft-compare-second", tl!("The document to compare with"), others, second);
    }
    ui.add_space(4.0);
    kit::segmented(ui, &mut tool.mode, &[(Mode::Auto, tl!("Automatic")), (Mode::Tree, tl!("Elements and members")), (Mode::Lines, tl!("Lines"))]);
    ui.add_space(kit::GAP);
    caption(ui, tl!("Do not count as a difference"));
    ui.add(widgets::checkbox(&mut tool.ignore.whitespace, tl!("White space")));
    ui.add(widgets::checkbox(&mut tool.ignore.case, tl!("Upper and lower case")));
    if tool.mode != Mode::Lines {
        ui.add(widgets::checkbox(&mut tool.ignore.comments, tl!("Comments")));
        ui.add(widgets::checkbox(&mut tool.ignore.child_order, tl!("The order of elements and list items")));
        ui.add(widgets::checkbox(&mut tool.ignore.numbers_by_value, tl!("How a number is written (1.0 and 1)")));
        ui.add(widgets::checkbox(&mut tool.ignore.prefixes, tl!("Namespace prefixes")));
    }
}

/// The differences of the last comparison, filling the rest of the panel. Returns where a click went. Shown while the document in
/// front is one of the two that were compared.
fn compare_results(ui: &mut egui::Ui, tool: &mut CompareTool, shown: u64, fresh: bool) -> Option<Go> {
    ui.add_space(kit::GAP);
    if tool.waiting.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Comparing…"));
        });
    }
    let Some(answer) = tool.answer.as_ref().filter(|a| a.first.0 == shown || a.second.0 == shown) else {
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
    kit::help(ui, &fmt(tl!("{first} against {second}"), &[("first", &answer.names.0), ("second", &answer.names.1)]));
    if let Some(note) = &done.note {
        kit::help(ui, note);
    }
    let count = done.count.to_string();
    kit::help(
        ui,
        &if done.count == 0 {
            tl!("The documents are the same.").to_owned()
        } else if done.count > done.differences.len().min(LISTED) {
            fmt(
                tl!("{count} differences; the first {shown} are listed. A click selects one in the first document, a right click in the second."),
                &[("count", &count), ("shown", &done.differences.len().min(LISTED).to_string())],
            )
        } else {
            fmt(tl!("{count} differences. A click selects one in the first document, a right click in the second."), &[("count", &count)])
        },
    );
    // The ranges are good for the text that was compared: after an edit the list stays but does not point.
    if !fresh {
        kit::help(ui, tl!("A document changed since: compare again."));
    }
    let (first, second) = (answer.first, answer.second);
    let t = crate::theme::Tokens::get(ui.ctx());
    let mut go = None;
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
                go = Some(Go { doc: first.0, range: difference.old.clone() });
            } else if response.secondary_clicked() {
                go = Some(Go { doc: second.0, range: difference.new.clone() });
            }
        }
    });
    go
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
    form.base = document_combo(ui, "xmlcraft-merge-base", tl!("The base document"), others, base);
    ui.add_space(4.0);
    caption(ui, tl!("Theirs"));
    form.theirs = document_combo(ui, "xmlcraft-merge-theirs", tl!("The other document"), others, theirs);
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
}
