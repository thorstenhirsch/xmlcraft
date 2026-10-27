//! The find and replace bar above the editor: what to look for, the options, how many matches there are, and next/previous/replace.
//!
//! The search itself is the editor crate's (`xmlcraft_editor::find`); this is the state of the bar and how it is drawn. All the matches
//! are shown in the editor (the styler's highlights), and the bar says which one is selected out of how many.

use std::ops::Range;
use std::sync::Arc;

use egui::{Key, vec2};
use xmlcraft_editor::find::{Direction, MAX_MATCHES, Matcher, Query, QueryError};

use crate::XmlCraftApp;
use crate::i18n::{fmt, tl};
use crate::theme::{self, Tokens};
use crate::{icons, widgets};

/// Documents larger than this are not searched for a live match count (it would read the whole text after every edit).
const COUNT_UP_TO: usize = 4 * 1024 * 1024;

/// The matches in one version of one document for one query: what the bar's counter is worked out from.
#[derive(Clone, Debug)]
pub(crate) struct FindStats {
    document: u64,
    version: u64,
    query: Query,
    /// The matches (at most [`MAX_MATCHES`]), in order, and whether there were more.
    matches: Arc<Vec<Range<usize>>>,
    more: bool,
}

#[derive(Default)]
pub(crate) struct FindBar {
    pub open: bool,
    pub show_replace: bool,
    pub query: String,
    pub replacement: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    /// Give the keyboard to the find box in the next frame (the bar was just opened or asked for again).
    pub focus: bool,
    /// The same for the replacement box (Enter in it replaces and stays there).
    focus_replacement: bool,
    /// The query as compiled; changes with the text and the options. Shared, so a frame hands it on without compiling it again.
    compiled: Option<(Query, Result<Arc<Matcher>, QueryError>)>,
    stats: Option<FindStats>,
}

impl FindBar {
    fn current_query(&self) -> Query {
        Query { pattern: self.query.clone(), regex: self.regex, case_sensitive: self.case_sensitive, whole_word: self.whole_word }
    }

    /// The compiled search for what is typed, if it is a valid one. Empty text is no search.
    pub fn matcher(&mut self) -> Option<&Arc<Matcher>> {
        let query = self.current_query();
        if self.compiled.as_ref().is_none_or(|(compiled_for, _)| *compiled_for != query) {
            let result = Matcher::new(&query).map(Arc::new);
            self.compiled = Some((query, result));
        }
        self.compiled.as_ref().and_then(|(_, result)| result.as_ref().ok())
    }

    /// The matches of the typed query in this version of this document, if the bar is open and has worked them out: what the editor
    /// highlights, the same ones the counter counts and Next goes to.
    pub fn shown_matches(&self, document: u64, version: u64) -> Option<Arc<Vec<Range<usize>>>> {
        let stats = self.stats.as_ref().filter(|s| self.open && s.document == document && s.version == version && s.query == self.current_query())?;
        Some(stats.matches.clone())
    }

    /// Why the typed query cannot be used (not for an empty one: that is just no search).
    fn error(&mut self) -> Option<String> {
        let _ = self.matcher();
        match self.compiled.as_ref().map(|(_, r)| r) {
            Some(Err(QueryError::Invalid(message))) => Some(message.clone()),
            _ => None,
        }
    }
}

impl XmlCraftApp {
    /// Show the bar (with the replace row when `replace`), put what is selected into the find box, and give it the keyboard.
    pub(crate) fn open_find(&mut self, replace: bool) {
        self.find.open = true;
        self.find.focus = true;
        self.find.show_replace |= replace;
        if let Some(doc) = self.session.active_document() {
            let selection = doc.editor().primary();
            // A selection on one line is what to look for; otherwise what was typed before stays.
            if !selection.is_empty()
                && let Ok(text) = doc.editor().buffer().slice(selection.start()..selection.end())
                && !text.contains(['\n', '\r'])
                && text.chars().count() <= 200
            {
                // In a regular-expression search the selected text is looked for as it is, not as a pattern.
                self.find.query = if self.find.regex { xmlcraft_editor::find::escape(&text) } else { text };
            }
        }
    }

    pub(crate) fn close_find(&mut self) {
        self.find.open = false;
        self.find.stats = None;
        self.focus_editor = true;
    }

    /// Select the next or previous match of the typed query (opening the bar when there is none yet).
    pub(crate) fn find_step(&mut self, direction: Direction) {
        // With a query typed before, F3 goes on with it even though the bar was closed; with none (or one that does not work) the bar opens.
        let Some(matcher) = (!self.find.query.is_empty()).then(|| self.find.matcher().cloned()).flatten() else {
            self.open_find(false);
            return;
        };
        if let Some(doc) = self.session.active_document_mut()
            && doc.editor_mut().select_match(&matcher, direction).is_none()
        {
            self.status = fmt(tl!("No matches for “{text}”"), &[("text", &self.find.query)]);
        }
    }

    /// Replace the selected match and select the next one (the Replace button).
    pub(crate) fn replace_one(&mut self) {
        let Some(matcher) = self.find.matcher().cloned() else { return };
        let replacement = self.find.replacement.clone();
        if let Some(doc) = self.session.active_document_mut() {
            doc.editor_mut().replace_next(&matcher, &replacement);
        }
    }

    /// Replace every match (the Replace All button), and say how many.
    pub(crate) fn replace_everything(&mut self) {
        let Some(matcher) = self.find.matcher().cloned() else { return };
        let replacement = self.find.replacement.clone();
        let Some(doc) = self.session.active_document_mut() else { return };
        self.status = match doc.editor_mut().replace_all(&matcher, &replacement, false) {
            Some(0) => fmt(tl!("No matches for “{text}”"), &[("text", &self.find.query)]),
            Some(n) => fmt(tl!("Replaced {n} matches"), &[("n", &n.to_string())]),
            None => fmt(tl!("More than {n} matches: nothing was replaced"), &[("n", &MAX_MATCHES.to_string())]),
        };
    }

    /// ⌘⇧L: a caret on every match of the typed query, or of the selected text (or word) when nothing is typed.
    pub(crate) fn select_all_occurrences(&mut self) {
        let typed = self.find.open && !self.find.query.is_empty();
        // A query that is typed but does not work selects nothing: the occurrences of something else would be a surprise.
        let matcher = if typed {
            let Some(matcher) = self.find.matcher().cloned() else { return };
            Some(matcher)
        } else {
            None
        };
        let Some(doc) = self.session.active_document_mut() else { return };
        let matcher = matcher.or_else(|| {
            let editor = doc.editor_mut();
            if editor.primary().is_empty() {
                editor.select_next_occurrence();
            }
            let primary = editor.primary();
            let text = editor.buffer().slice(primary.start()..primary.end()).ok()?;
            Matcher::new(&Query::text(&text)).ok().map(Arc::new)
        });
        if let Some(matcher) = matcher
            && doc.editor_mut().select_all_matches(&matcher).is_none()
        {
            self.status = fmt(tl!("More than {n} matches: no carets were added"), &[("n", &MAX_MATCHES.to_string())]);
        }
    }

    /// How many matches the typed query has in the document being shown (and whether there are more), and which one is selected
    /// (counted from 1). The matches are found again only when the text or the query changed.
    fn find_summary(&mut self) -> Option<(usize, bool, Option<usize>)> {
        let query = self.find.current_query();
        let matcher = self.find.matcher().cloned()?;
        let doc = self.session.active_document()?;
        let (document, version) = (doc.id(), doc.editor().version());
        let fresh = self.find.stats.as_ref().is_some_and(|s| s.document == document && s.version == version && s.query == query);
        if !fresh {
            let len = doc.editor().buffer().len();
            let (matches, more) = if len <= COUNT_UP_TO { matcher.find_all(&doc.editor().text(), 0..len, MAX_MATCHES) } else { (Vec::new(), true) };
            self.find.stats = Some(FindStats { document, version, query, matches: Arc::new(matches), more });
        }
        let stats = self.find.stats.as_ref()?;
        let selection = doc.editor().primary();
        let current = stats
            .matches
            .binary_search_by_key(&selection.start(), |m| m.start)
            .ok()
            .filter(|i| stats.matches.get(*i).is_some_and(|m| m.end == selection.end()))
            .map(|i| i + 1);
        Some((stats.matches.len(), stats.more, current))
    }

    /// Draw the bar above the editor. Returns nothing: what the user does is carried out at once.
    pub(crate) fn find_bar(&mut self, ui: &mut egui::Ui) {
        if !self.find.open {
            return;
        }
        let t = Tokens::get(ui.ctx());
        // The field takes what the controls beside it leave (they must all stay in view in a narrow window), up to a comfortable width.
        let field_width = (ui.available_width() - 360.0).clamp(90.0, 260.0);
        let before = self.find.current_query();
        let stats = self.find_summary();
        let error = self.find.error();
        let mut action: Option<Action> = None;
        egui::Frame::NONE.fill(t.panel).stroke(egui::Stroke::new(1.0, t.divider)).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            // Wraps in a narrow window rather than losing the controls at the right.
            ui.horizontal_wrapped(|ui| {
                let toggle = |ui: &mut egui::Ui, on: &mut bool, text: &str, name: &str| {
                    let response =
                        ui.add(egui::Button::new(egui::RichText::new(text).font(theme::mono(12.0))).selected(*on).min_size(vec2(26.0, 24.0)));
                    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, *on, name));
                    let response = response.on_hover_text(name);
                    if response.clicked() {
                        *on = !*on;
                    }
                };
                let field = ui.add(
                    egui::TextEdit::singleline(&mut self.find.query)
                        .id(egui::Id::new("xmlcraft-find-query"))
                        .hint_text(tl!("Find"))
                        .desired_width(field_width),
                );
                widgets::name_for_assistive_technology(ui, &field, tl!("Find"));
                if std::mem::take(&mut self.find.focus) {
                    field.request_focus();
                }
                // A single-line field gives up the keyboard at Enter and at Escape, in the frame of the key.
                if field.lost_focus() || field.has_focus() {
                    let (enter, escape, shift) = ui.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape), i.modifiers.shift));
                    if escape {
                        action = Some(Action::Close);
                    } else if enter && field.lost_focus() {
                        action = Some(Action::Step(if shift { Direction::PREVIOUS } else { Direction::NEXT }));
                    }
                }
                toggle(ui, &mut self.find.case_sensitive, "Aa", tl!("Match case"));
                toggle(ui, &mut self.find.whole_word, "W", tl!("Whole word"));
                toggle(ui, &mut self.find.regex, ".*", tl!("Regular expression"));
                let summary = match (&error, stats, self.find.query.is_empty()) {
                    (_, _, true) => String::new(),
                    (Some(message), _, _) => message.clone(),
                    (None, Some((0, false, _)), _) => tl!("No results").to_owned(),
                    // A text too large to count (it is not read after every edit): matches are still found and highlighted.
                    (None, Some((0, true, _)), _) => tl!("Too large to count").to_owned(),
                    (None, Some((count, more, current)), _) => {
                        let total = if more { format!("{count}+") } else { count.to_string() };
                        match current {
                            Some(i) => fmt(tl!("{i} of {total}"), &[("i", &i.to_string()), ("total", &total)]),
                            None => fmt(tl!("{total} matches"), &[("total", &total)]),
                        }
                    }
                    (None, None, _) => String::new(),
                };
                let color = if error.is_some() { crate::editor_view::SyntaxTokens::for_kind(t.kind).error } else { t.text_muted };
                ui.label(egui::RichText::new(summary).font(theme::regular(12.0)).color(color));
                for (icon, name, step) in
                    [("chevron-up", tl!("Previous match"), Direction::PREVIOUS), ("chevron-down", tl!("Next match"), Direction::NEXT)]
                {
                    if icons::button(ui, icon, 24.0, false, name).clicked() {
                        action = Some(Action::Step(step));
                    }
                }
                let (swap_icon, swap) =
                    if self.find.show_replace { ("chevron-down", tl!("Hide replace")) } else { ("chevron-right", tl!("Show replace")) };
                if icons::button(ui, swap_icon, 24.0, self.find.show_replace, swap).clicked() {
                    self.find.show_replace = !self.find.show_replace;
                }
                if icons::button(ui, "x", 24.0, false, tl!("Close")).clicked() {
                    action = Some(Action::Close);
                }
            });
            if self.find.show_replace {
                ui.horizontal_wrapped(|ui| {
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut self.find.replacement)
                            .id(egui::Id::new("xmlcraft-find-replacement"))
                            .hint_text(tl!("Replace with"))
                            .desired_width(field_width),
                    );
                    widgets::name_for_assistive_technology(ui, &field, tl!("Replace with"));
                    if std::mem::take(&mut self.find.focus_replacement) {
                        field.request_focus();
                    }
                    if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        action = Some(Action::Replace);
                        self.find.focus_replacement = true; // Enter replaces and stays in the box, as Enter in the find box stays there
                    }
                    if ui.button(tl!("Replace")).clicked() {
                        action = Some(Action::Replace);
                    }
                    if ui.button(tl!("Replace All")).clicked() {
                        action = Some(Action::ReplaceAll);
                    }
                });
            }
        });
        match action {
            Some(Action::Close) => self.close_find(),
            Some(Action::Step(direction)) => {
                self.find.focus = true; // the keyboard stays in the bar while stepping through the matches
                self.find_step(direction);
            }
            Some(Action::Replace) => self.replace_one(),
            Some(Action::ReplaceAll) => self.replace_everything(),
            None => {
                // What was typed (or an option) changed: go to the first match at or after the caret, so the search follows the typing.
                if self.find.current_query() != before
                    && let Some(matcher) = self.find.matcher().cloned()
                    && let Some(doc) = self.session.active_document_mut()
                {
                    doc.editor_mut().select_match_at_or_after_caret(&matcher);
                }
            }
        }
    }
}

enum Action {
    Close,
    Step(Direction),
    Replace,
    ReplaceAll,
}
