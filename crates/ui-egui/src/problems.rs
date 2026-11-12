//! The Problems panel and moving between problems: what is wrong with the document as XML and, once it has been validated, as a document of
//! its grammar. The panel filters by severity, groups by where a problem comes from, and the keys step from one to the next.

use std::ops::Range;

use egui::{Align, Layout, Rect, Sense, vec2};
use xmlcraft_engine::analysis::Skipped;
use xmlcraft_syntax::Severity;

use crate::editor_view::SyntaxTokens;
use crate::i18n::{fmt, tl};
use crate::kit;
use crate::theme::{self, Tokens};
use crate::{XmlCraftApp, icons, widgets};

/// Rows of the list: tall enough to click.
const ROW: f32 = 28.0;

/// Which problems the panel lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Filter {
    All,
    Errors,
    Warnings,
}

/// Where a problem comes from, which is also how the panel groups them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Group {
    /// The text is not well-formed XML.
    WellFormedness,
    /// The document does not follow its DTD or schema.
    Validity,
    /// An electronic invoice breaks a rule of its standard (EN 16931, the CIUS it claims).
    InvoiceRules,
    /// A problem in another file the document depends on (an external DTD, a schema document).
    OtherFile,
}

impl Group {
    fn title(self) -> &'static str {
        match self {
            Self::WellFormedness => tl!("Well-formedness"),
            Self::Validity => tl!("Validity"),
            Self::InvoiceRules => tl!("Invoice rules"),
            Self::OtherFile => tl!("In other files"),
        }
    }
}

/// One problem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Problem {
    pub severity: Severity,
    pub group: Group,
    pub message: String,
    /// Where it is: `Ln 3, Col 7` in the document; the name of the file for a problem in another file.
    pub place: String,
    /// What a click selects in the document; none for a problem in another file.
    pub jump: Option<Range<usize>>,
    /// The code of the diagnostic (what, with where it starts, tells it from another problem at the same place).
    pub code: &'static str,
}

impl Filter {
    fn admits(self, severity: Severity) -> bool {
        match self {
            Self::All => true,
            Self::Errors => severity == Severity::Error,
            Self::Warnings => severity != Severity::Error,
        }
    }
}

/// Where a problem of the validation comes from: the document, or another file it depends on.
fn group_of_file(file: Option<&str>) -> Group {
    if file.is_none() { Group::Validity } else { Group::OtherFile }
}

/// Whether a problem is a rule of an electronic invoice (F8 on an invoice).
fn is_invoice_rule(p: &xmlcraft_engine::validation::ValidityProblem) -> bool {
    p.diagnostic.code.key().starts_with("einvoice.rule.")
}

/// What a click on a problem selects: its span in the document; nothing for a problem in another file (its span is in that file).
fn jump_for(file: Option<&str>, span: &Range<usize>) -> Option<Range<usize>> {
    file.is_none().then(|| span.clone())
}

/// The problems of the document being shown: what is wrong with the text as XML first, then, when a validation of the same text has been
/// asked for, what is wrong with it as a document of its grammar.
pub(crate) fn collect(app: &XmlCraftApp) -> Vec<Problem> {
    let (Some(doc), Some(analysis)) = (app.session.active_document(), app.active_analysis()) else { return Vec::new() };
    let buffer = doc.editor().buffer();
    let at = |offset: usize| {
        let position = buffer.position(offset.min(buffer.len()));
        fmt(tl!("Ln {line}, Col {col}"), &[("line", &(position.line + 1).to_string()), ("col", &(position.col + 1).to_string())])
    };
    let mut rows: Vec<Problem> = analysis
        .diagnostics
        .iter()
        .map(|d| Problem {
            severity: d.severity(),
            group: Group::WellFormedness,
            message: d.message(),
            place: at(d.span.start),
            jump: Some(d.span.clone()),
            code: d.code.key(),
        })
        .collect();
    rows.extend(app.active_validation().into_iter().flat_map(|v| v.problems.iter()).map(|p| Problem {
        severity: p.diagnostic.severity(),
        group: if is_invoice_rule(p) { Group::InvoiceRules } else { group_of_file(p.file.as_deref()) },
        message: p.diagnostic.message(),
        place: match &p.file {
            // A rule of an invoice says which rule set it is from: `EN 16931 · Ln 2, Col 1`.
            None if is_invoice_rule(p) => match p.diagnostic.args.get(2) {
                Some(source) => format!("{source} · {}", at(p.diagnostic.span.start)),
                None => at(p.diagnostic.span.start),
            },
            None => at(p.diagnostic.span.start),
            Some(file) => std::path::Path::new(file).file_name().map_or_else(|| file.clone(), |n| n.to_string_lossy().into_owned()),
        },
        jump: jump_for(p.file.as_deref(), &p.diagnostic.span),
        code: p.diagnostic.code.key(),
    }));
    rows
}

/// The most messages one hover tip lists.
const MAX_TIP: usize = 4;

/// What is wrong at `offset` of the document being shown: the messages of the problems whose span holds it (a span with no width holds
/// its own offset), the well-formedness ones first. Only for the text the analysis is about: spans of older text would point elsewhere.
pub(crate) fn messages_at(app: &XmlCraftApp, offset: usize) -> Vec<(Severity, String)> {
    let (Some(doc), Some(analysis)) = (app.session.active_document(), app.active_analysis()) else { return Vec::new() };
    if analysis.version != doc.editor().version() {
        return Vec::new();
    }
    let holds = |span: &Range<usize>| span.start <= offset && offset < span.end.max(span.start.saturating_add(1));
    let own = analysis.diagnostics.iter().filter(|d| holds(&d.span)).map(|d| (d.severity(), d.message()));
    let validation = app
        .active_validation()
        .into_iter()
        .flat_map(|v| v.problems.iter())
        .filter(|p| p.file.is_none() && holds(&p.diagnostic.span))
        .map(|p| (p.diagnostic.severity(), p.diagnostic.message()));
    let mut found: Vec<(Severity, String)> = Vec::new();
    for (severity, message) in own.chain(validation) {
        if !found.iter().any(|(_, m)| *m == message) {
            found.push((severity, message));
        }
        if found.len() == MAX_TIP {
            break;
        }
    }
    found
}

/// The start of the problem after `caret` (`delta` > 0) or before it (`delta` < 0) among `starts`, sorted; past the last it is the first
/// and before the first it is the last. None when there is no problem.
pub(crate) fn step_from(starts: &[usize], caret: usize, delta: isize) -> Option<usize> {
    if starts.is_empty() {
        return None;
    }
    if delta >= 0 {
        starts.iter().copied().find(|s| *s > caret).or_else(|| starts.first().copied())
    } else {
        starts.iter().rev().copied().find(|s| *s < caret).or_else(|| starts.last().copied())
    }
}

impl XmlCraftApp {
    /// The messages of the problems at `offset` of the document being shown: what the hover tip says there.
    pub fn problem_messages_at(&self, offset: usize) -> Vec<String> {
        messages_at(self, offset).into_iter().map(|(_, message)| message).collect()
    }

    /// Mark the problem that starts at `start` in the document being shown as the one last gone to.
    fn select_problem(&mut self, start: usize) {
        self.problem_selected = self.session.active_document().map(|d| (d.id(), start));
    }

    /// Where the problem last gone to starts, in the document being shown (None when it was gone to in another document).
    fn selected_problem(&self) -> Option<usize> {
        let id = self.session.active_document()?.id();
        self.problem_selected.filter(|(doc, _)| *doc == id).map(|(_, start)| start)
    }

    /// Go to the next (`delta` 1) or the previous (`delta` -1) problem in the text, wrapping round, and say which it is.
    pub(crate) fn step_problem(&mut self, delta: isize) {
        let problems = collect(self);
        let mut jumps: Vec<(usize, &Problem)> = problems.iter().filter_map(|p| p.jump.as_ref().map(|j| (j.start, p))).collect();
        jumps.sort_by_key(|(start, _)| *start);
        let starts: Vec<usize> = {
            let mut s: Vec<usize> = jumps.iter().map(|(s, _)| *s).collect();
            s.dedup();
            s
        };
        // From the start of the selection: after a jump the selection is the problem, and the one before it is before its start.
        let Some(caret) = self.session.active_document().map(|d| d.editor().primary().start()) else { return };
        let Some(start) = step_from(&starts, caret, delta) else {
            self.status = if !problems.is_empty() {
                tl!("The problems listed are in other files.")
            } else if self.active_analysis().is_none() {
                tl!("The document has not been checked yet.")
            } else {
                tl!("There are no problems.")
            }
            .to_owned();
            return;
        };
        let Some((span, message)) =
            problems.iter().find_map(|p| p.jump.as_ref().filter(|j| j.start == start).map(|j| (j.clone(), p.message.clone())))
        else {
            return;
        };
        let number = starts.iter().position(|s| *s == start).map_or(1, |i| i + 1);
        self.status =
            fmt(tl!("Problem {n} of {total}: {message}"), &[("n", &number.to_string()), ("total", &starts.len().to_string()), ("message", &message)]);
        self.select_problem(start);
        self.jump_to(span);
    }
}

/// The rows to draw: the problems that pass the filter, with a heading before each group when there is more than one.
#[derive(Debug, PartialEq, Eq)]
enum Line {
    Heading(Group),
    Problem(usize),
}

fn lines(problems: &[Problem], filter: Filter) -> Vec<Line> {
    let shown: Vec<usize> = (0..problems.len()).filter(|&i| problems.get(i).is_some_and(|p| filter.admits(p.severity))).collect();
    let mut groups: Vec<Group> = shown.iter().filter_map(|&i| problems.get(i).map(|p| p.group)).collect();
    groups.sort();
    groups.dedup();
    let mut out = Vec::new();
    for group in &groups {
        if groups.len() > 1 {
            out.push(Line::Heading(*group));
        }
        out.extend(shown.iter().copied().filter(|&i| problems.get(i).is_some_and(|p| p.group == *group)).map(Line::Problem));
    }
    out
}

pub(crate) fn body(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    let t = Tokens::get(ui.ctx());
    if app.session.active_document().is_none() {
        widgets::empty_state(ui, 6.0, tl!("No problems"), tl!("Well-formedness and validation results appear here."));
        return;
    }
    let Some(analysis) = app.active_analysis() else {
        widgets::empty_state(ui, 6.0, tl!("Checking…"), "");
        return;
    };
    match analysis.skipped {
        Some(Skipped::TooLarge) => return widgets::empty_state(ui, 6.0, tl!("Not checked"), tl!("The document is too large to check while typing.")),
        Some(Skipped::Failed) => return widgets::empty_state(ui, 6.0, tl!("The check failed"), tl!("This is a bug. The document is unchanged.")),
        None => {}
    }
    let problems = collect(app);
    if problems.is_empty() {
        let valid = app.active_validation().is_some_and(|v| v.is_valid());
        widgets::empty_state(
            ui,
            6.0,
            tl!("No problems"),
            if valid { tl!("The document is well-formed and valid.") } else { tl!("The document is well-formed.") },
        );
        return;
    }
    let errors = problems.iter().filter(|p| p.severity == Severity::Error).count();
    let others = problems.len() - errors;
    // The filters, and the way from one problem to the next.
    let mut step = 0isize;
    egui::Frame::NONE.inner_margin(egui::Margin { left: 12, right: 10, top: 0, bottom: 6 }).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for (filter, label, count) in
                [(Filter::All, tl!("All"), problems.len()), (Filter::Errors, tl!("Errors"), errors), (Filter::Warnings, tl!("Warnings"), others)]
            {
                if kit::toggle_chip(ui, app.problem_filter == filter, &format!("{label} {count}")).clicked() {
                    app.problem_filter = filter;
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if icons::button(ui, "chevron-down", 26.0, false, &tip(ui, "xml.next_problem", tl!("Next problem"))).clicked() {
                    step = 1;
                }
                if icons::button(ui, "chevron-up", 26.0, false, &tip(ui, "xml.previous_problem", tl!("Previous problem"))).clicked() {
                    step = -1;
                }
            });
        });
    });
    app.ensure_fixes();
    let lines = lines(&problems, app.problem_filter);
    if lines.is_empty() {
        widgets::empty_state(ui, 12.0, tl!("Nothing in this list"), tl!("Choose All to see every problem."));
    }
    let syntax = SyntaxTokens::for_kind(t.kind);
    let mut clicked = None;
    let mut fix_clicked: Option<xmlcraft_engine::quickfix::QuickFix> = None;
    let selected = app.selected_problem();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW, lines.len(), |ui, range| {
        for line in lines.get(range).unwrap_or_default() {
            let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
            match line {
                Line::Heading(group) => {
                    ui.painter().text(
                        rect.left_center() + vec2(14.0, 3.0),
                        egui::Align2::LEFT_CENTER,
                        group.title().to_uppercase(),
                        theme::semibold(10.5),
                        t.text_faint,
                    );
                }
                Line::Problem(index) => {
                    let Some(problem) = problems.get(*index) else { continue };
                    let (message, place) = (problem.message.as_str(), problem.place.clone());
                    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{message} ({place})")));
                    let row = rect.shrink2(vec2(6.0, 1.0));
                    let is_selected = problem.jump.as_ref().is_some_and(|j| Some(j.start) == selected);
                    if is_selected {
                        ui.painter().rect_filled(row, 6.0, t.accent_soft);
                    } else if response.hovered() && problem.jump.is_some() {
                        ui.painter().rect_filled(row, 6.0, t.hover);
                    }
                    let (icon, color) = match problem.severity {
                        Severity::Error => ("circle-x", syntax.error),
                        Severity::Warning => ("triangle-alert", syntax.warning),
                        Severity::Info => ("info", syntax.info),
                    };
                    icons::paint(ui, Rect::from_min_size(rect.left_center() + vec2(14.0, -8.0), vec2(16.0, 16.0)), icon, 15.0, color);
                    let place_galley = ui.painter().layout_no_wrap(place, theme::regular(11.5), t.text_faint);
                    let right = rect.right() - 16.0;
                    ui.painter().galley(
                        egui::pos2(right - place_galley.size().x, rect.center().y - place_galley.size().y / 2.0),
                        place_galley.clone(),
                        t.text_faint,
                    );
                    // A problem that can be put right has a Fix button before its place.
                    let fix = problem.jump.as_ref().and_then(|j| app.fixes_of_problem(j.start, problem.code).first());
                    let mut text_right = right - place_galley.size().x - 12.0;
                    if let Some(fix) = fix {
                        let pill = Rect::from_min_size(egui::pos2(text_right - 40.0, rect.center().y - 9.0), vec2(40.0, 18.0));
                        let button = ui.interact(pill, response.id.with("fix"), Sense::click());
                        let label = fmt(tl!("Fix: {what}"), &[("what", &fix.title)]);
                        button.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label.clone()));
                        let wash = if button.hovered() { t.accent } else { t.accent_soft };
                        let ink = if button.hovered() { egui::Color32::WHITE } else { t.accent_text };
                        ui.painter().rect_filled(pill, 9.0, wash);
                        ui.painter().text(pill.center(), egui::Align2::CENTER_CENTER, tl!("Fix"), theme::semibold(11.0), ink);
                        if theme::hand(button).on_hover_text(&fix.title).clicked() {
                            fix_clicked = Some(fix.clone());
                        }
                        text_right = pill.left() - 8.0;
                    }
                    let text_area = Rect::from_min_max(rect.min, egui::pos2(text_right, rect.max.y));
                    ui.painter().with_clip_rect(text_area).text(
                        rect.left_center() + vec2(38.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        message,
                        theme::regular(12.5),
                        t.text,
                    );
                    // A problem in another file cannot be gone to: no pointer for it.
                    let response = if problem.jump.is_some() { theme::hand(response) } else { response };
                    if response.clicked() {
                        clicked = Some((problem.jump.clone(), problem.message.clone()));
                    }
                }
            }
        }
    });
    if step != 0 {
        app.step_problem(step);
    }
    if let Some(fix) = fix_clicked {
        app.apply_quick_fix(&fix);
        return;
    }
    if let Some((Some(span), _)) = clicked {
        app.select_problem(span.start);
        app.jump_to(span);
    }
}

/// A button's tip: what it does, and its shortcut when the registry has one.
fn tip(ui: &egui::Ui, command: &str, what: &str) -> String {
    match crate::commands::command_shortcut_label(ui.ctx(), command) {
        s if s.is_empty() => what.to_owned(),
        s => format!("{what} ({s})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem(severity: Severity, group: Group, start: usize) -> Problem {
        Problem { severity, group, message: format!("m{start}"), place: String::new(), jump: Some(start..start + 1), code: "test" }
    }

    #[test]
    fn stepping_goes_to_the_next_problem_after_the_caret_and_wraps_round() {
        let starts = [3, 10, 20];
        assert_eq!(step_from(&starts, 0, 1), Some(3));
        assert_eq!(step_from(&starts, 3, 1), Some(10), "from a problem to the next, not to itself");
        assert_eq!(step_from(&starts, 12, 1), Some(20));
        assert_eq!(step_from(&starts, 20, 1), Some(3), "past the last it is the first");
        assert_eq!(step_from(&starts, 99, 1), Some(3));
        assert_eq!(step_from(&starts, 12, -1), Some(10));
        assert_eq!(step_from(&starts, 10, -1), Some(3));
        assert_eq!(step_from(&starts, 3, -1), Some(20), "before the first it is the last");
        assert_eq!(step_from(&starts, 0, -1), Some(20));
        assert_eq!(step_from(&[], 5, 1), None);
        assert_eq!(step_from(&[], 5, -1), None);
        assert_eq!(step_from(&[7], 7, 1), Some(7), "the only problem, from itself, wraps to itself");
    }

    #[test]
    fn the_filters_pass_the_severities_they_name() {
        for severity in [Severity::Error, Severity::Warning, Severity::Info] {
            assert!(Filter::All.admits(severity));
        }
        assert!(Filter::Errors.admits(Severity::Error));
        assert!(!Filter::Errors.admits(Severity::Warning) && !Filter::Errors.admits(Severity::Info));
        // What is not an error is a warning's kind: a note is listed with the warnings.
        assert!(Filter::Warnings.admits(Severity::Warning) && Filter::Warnings.admits(Severity::Info));
        assert!(!Filter::Warnings.admits(Severity::Error));
    }

    #[test]
    fn a_problem_in_another_file_is_grouped_so_and_cannot_be_jumped_to() {
        assert_eq!(group_of_file(None), Group::Validity);
        assert_eq!(group_of_file(Some("/s/types.xsd")), Group::OtherFile);
        assert_eq!(jump_for(None, &(4..9)), Some(4..9));
        assert_eq!(jump_for(Some("/s/types.xsd"), &(4..9)), None, "its span is in that file, not in the document");
    }

    #[test]
    fn the_list_has_headings_only_when_there_is_more_than_one_group() {
        let problems = vec![
            problem(Severity::Error, Group::Validity, 5),
            problem(Severity::Warning, Group::WellFormedness, 1),
            problem(Severity::Error, Group::Validity, 9),
        ];
        assert_eq!(
            lines(&problems, Filter::All),
            [Line::Heading(Group::WellFormedness), Line::Problem(1), Line::Heading(Group::Validity), Line::Problem(0), Line::Problem(2)]
        );
        // One group left after filtering: no heading.
        assert_eq!(lines(&problems, Filter::Errors), [Line::Problem(0), Line::Problem(2)]);
        assert_eq!(lines(&problems, Filter::Warnings), [Line::Problem(1)]);
        assert!(lines(&[], Filter::All).is_empty());
    }
}
