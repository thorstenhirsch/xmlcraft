//! Entry Helpers: what the DTD allows at the caret (the elements that may come, the attributes of the element, the entities), and a
//! double click that puts the choice in the text.

use std::sync::Arc;

use egui::{Sense, vec2};
use xmlcraft_complete::{
    AttributeChoice, Choices, Declaration, ElementChoice, EntityChoice, attribute_snippet, choices_at, element_snippet, entity_snippet,
    xsd_attribute_snippet, xsd_choices_at, xsd_element_snippet,
};
use xmlcraft_engine::schema::Schema;
use xmlcraft_engine::xsd_schemas::{XsdAnswer, XsdSchema};
use xmlcraft_grammar::Grammar;
use xmlcraft_text::Edit;

use crate::i18n::tl;
use crate::theme::{self, Tokens};
use crate::{XmlCraftApp, widgets};

/// Documents larger than this get no Entry Helpers: they are worked out from the whole text each time the caret moves.
const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
const ROW: f32 = 24.0;

/// The grammar of a document's DTD (or the XML Schema it names), and the choices worked out from it for one place in one version of the
/// text.
#[derive(Default)]
pub(crate) struct HelperState {
    /// The request the grammar answers (see `xmlcraft_engine::schema::key_of`).
    pub(crate) grammar_key: Option<u64>,
    pub(crate) grammar: Option<Arc<Grammar>>,
    /// The last request for a grammar: not asked again for the same DOCTYPE at the same place.
    pub(crate) requested_key: Option<u64>,
    /// The version of the text a request was last looked for in.
    pub(crate) checked_version: Option<u64>,
    /// The XML Schema the document names (a document with a DOCTYPE is checked against that), loaded; and why there is none, when the
    /// answer to the last request had none.
    pub(crate) xsd: Option<Arc<XsdSchema>>,
    pub(crate) xsd_error: Option<String>,
    /// The request `xsd` or `xsd_error` answers (see `xmlcraft_engine::xsd_schemas::xsd_key`), and the last request made.
    pub(crate) xsd_key: Option<u64>,
    pub(crate) requested_xsd_key: Option<u64>,
    /// How many requests for a schema were made for this document (what tests count).
    pub(crate) xsd_requests: usize,
    cache: Option<Cached>,
}

/// The choices at one place of one version of the text, from the grammar and the schema as they were.
struct Cached {
    version: u64,
    offset: usize,
    grammar_key: Option<u64>,
    xsd_key: Option<u64>,
    choices: Option<Choices>,
}

impl HelperState {
    /// Forget what was asked for, so that the grammar and the schema are asked for again (what they are made from may have changed
    /// without the text having changed).
    pub(crate) fn forget_requests(&mut self) {
        self.requested_key = None;
        self.requested_xsd_key = None;
        self.checked_version = None;
        // The same key may now answer with other content (a catalog was added, the downloads were deleted).
        self.cache = None;
    }

    /// Take the answer to a request for a grammar, if it is the answer to the last one asked: a DOCTYPE that was edited meanwhile has been
    /// asked for again, and the old answer would be the wrong grammar.
    pub(crate) fn accept(&mut self, schema: Schema) -> bool {
        if self.requested_key != Some(schema.key) {
            return false;
        }
        self.grammar_key = Some(schema.key);
        self.grammar = schema.grammar;
        true
    }

    /// Take the answer to a request for a schema, on the same terms.
    pub(crate) fn accept_xsd(&mut self, answer: XsdAnswer) -> bool {
        if self.requested_xsd_key != Some(answer.key) {
            return false;
        }
        self.xsd_key = Some(answer.key);
        match answer.schema {
            Ok(schema) => {
                self.xsd = Some(schema);
                self.xsd_error = None;
            }
            Err(why) => {
                self.xsd = None;
                self.xsd_error = Some(why);
            }
        }
        true
    }

    /// The document names no schema (any more): none is held.
    pub(crate) fn forget_xsd(&mut self) {
        self.xsd = None;
        self.xsd_error = None;
        self.xsd_key = None;
        self.requested_xsd_key = None;
    }

    /// The choices at `offset` of the text (version `version`), worked out again only when something they depend on changed. A DTD is
    /// what a document with a DOCTYPE is answered from; a schema, when there is none.
    fn choices(&mut self, version: u64, offset: usize, text: impl FnOnce() -> String) -> Option<&Choices> {
        let stale =
            self.cache.as_ref().is_none_or(|c| (c.version, c.offset, c.grammar_key, c.xsd_key) != (version, offset, self.grammar_key, self.xsd_key));
        if stale {
            let choices = match (&self.grammar, &self.xsd) {
                (None, None) => None,
                (grammar, xsd) => {
                    let text = text();
                    (text.len() <= MAX_TEXT_BYTES).then(|| {
                        let root = xmlcraft_syntax::parse(&text).syntax();
                        match (grammar, xsd) {
                            (Some(grammar), _) => choices_at(&root, offset, grammar),
                            (None, Some(schema)) => xsd_choices_at(&root, offset, schema),
                            (None, None) => Choices::default(),
                        }
                    })
                }
            };
            self.cache = Some(Cached { version, offset, grammar_key: self.grammar_key, xsd_key: self.xsd_key, choices });
        }
        self.cache.as_ref().and_then(|c| c.choices.as_ref())
    }
}

impl XmlCraftApp {
    /// What the DTD (or the schema) allows at the caret of the document being shown, if one was read.
    fn helper_choices(&mut self) -> Option<Choices> {
        let doc = self.session.active_document()?;
        let (id, version, caret) = (doc.id(), doc.editor().version(), doc.editor().primary().head());
        let view = self.views.get_mut(&id)?;
        view.helpers.choices(version, caret, || doc.editor().text()).cloned()
    }

    /// Put an element at the caret, with what its declaration requires when the option is on. `scope` are the namespace declarations at
    /// the caret (from the choices), for the names an XML Schema element is written with.
    pub(crate) fn insert_element(&mut self, choice: &ElementChoice, scope: &[Declaration]) {
        let snippet = match (self.active_grammar(), self.active_xsd()) {
            (Some(grammar), _) => element_snippet(&grammar, &choice.name, self.entry_mandatory),
            (None, Some(schema)) => xsd_element_snippet(&schema, scope, choice, self.entry_mandatory),
            (None, None) => return,
        };
        self.insert_at_selection(snippet.text, snippet.caret);
    }

    /// Put an entity reference at the caret.
    pub(crate) fn insert_entity(&mut self, name: &str) {
        let snippet = entity_snippet(name);
        self.insert_at_selection(snippet.text, snippet.caret);
    }

    /// Put text (a value the type of the element lists) at the caret, replacing the selection.
    pub(crate) fn insert_value(&mut self, value: &str) {
        let text = value.replace('&', "&amp;").replace('<', "&lt;");
        let caret = text.len();
        self.insert_at_selection(text, caret);
    }

    /// Add an attribute to the start tag of the element at `at` (the end of the name and the attributes already there), with the
    /// namespace declaration its name needs when it has one.
    pub(crate) fn insert_attribute(&mut self, name: &str, at: usize, first_value: Option<&str>, declare: Option<&Declaration>) {
        let snippet = match declare {
            Some(_) => xsd_attribute_snippet(name, first_value, declare),
            None => attribute_snippet(name, first_value),
        };
        let Some(doc) = self.session.active_document_mut() else { return };
        let editor = doc.editor_mut();
        if at <= editor.buffer().len() && editor.apply_edits(&[Edit::insert(at, snippet.text)]) {
            let caret = at + snippet.caret;
            editor.set_selection(caret, caret);
            self.focus_editor = true;
        }
    }

    fn active_grammar(&self) -> Option<Arc<Grammar>> {
        let id = self.session.active_document()?.id();
        self.views.get(&id)?.helpers.grammar.clone()
    }

    fn active_xsd(&self) -> Option<Arc<XsdSchema>> {
        let id = self.session.active_document()?.id();
        self.views.get(&id)?.helpers.xsd.clone()
    }

    /// Replace the selection (or insert at the caret) as one undoable step, and put the caret `caret` bytes into the new text.
    fn insert_at_selection(&mut self, text: String, caret: usize) {
        let Some(doc) = self.session.active_document_mut() else { return };
        let editor = doc.editor_mut();
        let selection = editor.primary();
        let start = selection.start();
        if editor.apply_edits(&[Edit::replace(selection.start()..selection.end(), text)]) {
            editor.set_selection(start + caret, start + caret);
            self.focus_editor = true;
        }
    }
}

/// One row of a list: the name, with a mark for what is required, greyed when it is used already. A double click is what the row is for.
fn row(ui: &mut egui::Ui, name: &str, detail: Option<&str>, mandatory: bool, used: bool, selected: bool, scroll: bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    let label = if mandatory { format!("{name} (required)") } else { name.to_owned() };
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, !used, selected, label.clone()));
    if selected {
        ui.painter().rect_filled(rect, 0.0, t.accent_soft);
        if scroll {
            response.scroll_to_me(None);
        }
    } else if response.hovered() && !used {
        ui.painter().rect_filled(rect, 0.0, t.hover);
    }
    let color = if used { t.text_faint } else { t.text };
    if mandatory {
        ui.painter().text(rect.left_center() + vec2(14.0, 0.0), egui::Align2::CENTER_CENTER, "!", theme::semibold(13.0), t.accent);
    }
    ui.painter().text(rect.left_center() + vec2(28.0, 0.0), egui::Align2::LEFT_CENTER, name, theme::regular(12.5), color);
    if let Some(detail) = detail {
        let galley = ui.painter().layout_no_wrap(detail.to_owned(), theme::regular(11.5), t.text_faint);
        let at = egui::pos2(rect.right() - 14.0 - galley.size().x, rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(at, galley, t.text_faint);
    }
    response
}

fn hint(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(16, 0)).show(ui, |ui| {
        ui.label(egui::RichText::new(text).font(theme::regular(12.0)).color(t.text_faint));
    });
}

/// Whether the interface fonts (Inter, and the icon font) have a glyph for `c`: Basic Latin, Latin-1, Latin Extended, Greek, Cyrillic,
/// the general punctuation of running text and the euro sign. What is not here is not drawn (a box, or a refusal in the tests).
fn drawable(c: char) -> bool {
    c.is_ascii_graphic()
        || c == ' '
        || ('\u{00A1}'..='\u{024F}').contains(&c)
        || ('\u{0370}'..='\u{03FF}').contains(&c)
        || ('\u{0400}'..='\u{04FF}').contains(&c)
        || ('\u{2010}'..='\u{2027}').contains(&c)
        || c == '\u{20AC}'
}

/// The text of an entity as it is shown beside its name: its first 24 characters, or nothing when one of them could not be drawn (the
/// XHTML DTD has some two hundred entities for symbols, which would be boxes).
fn shown_value(value: &str) -> Option<String> {
    let shown: String = value.chars().take(24).collect();
    (!shown.is_empty() && shown.chars().all(drawable)).then_some(shown)
}

/// What a click on a row asks for.
#[derive(Clone)]
enum Insert {
    Element(ElementChoice),
    Attribute { name: String, at: usize, first_value: Option<String>, declare: Option<Declaration> },
    Entity(String),
    Value(String),
}

/// What the panel has to say when there is nothing to list: the document has no DTD or schema, or it is still being read.
fn waiting(ui: &mut egui::Ui, app: &XmlCraftApp, id: u64) -> bool {
    let Some(view) = app.views.get(&id) else { return true };
    let has_doctype = view.analysis.as_ref().is_some_and(|a| a.doctype.is_some());
    let has_hints = view.analysis.as_ref().is_some_and(|a| !a.schema_hints.is_empty());
    let helpers = &view.helpers;
    if has_doctype {
        if helpers.grammar.is_some() {
            return false;
        }
        // The answer to the last request is in, and there is no grammar in it: the DOCTYPE could not be read.
        if helpers.grammar_key.is_some() && helpers.grammar_key == helpers.requested_key {
            widgets::empty_state(ui, 24.0, tl!("No DTD to show"), tl!("The DOCTYPE is too large to be read."));
        } else {
            widgets::empty_state(ui, 24.0, tl!("Reading the DTD…"), "");
        }
        return true;
    }
    // A schema the document does not name but the app knows for its namespace is used, and when there is none it is the same as no schema.
    if helpers.xsd.is_some() {
        return false;
    }
    if has_hints {
        if helpers.xsd_key.is_some() && helpers.xsd_key == helpers.requested_xsd_key {
            widgets::empty_state(ui, 24.0, tl!("No schema to show"), helpers.xsd_error.as_deref().unwrap_or_default());
        } else {
            widgets::empty_state(ui, 24.0, tl!("Reading the schema…"), "");
        }
        return true;
    }
    widgets::empty_state(
        ui,
        24.0,
        tl!("No DTD or schema"),
        tl!("Add a DOCTYPE, or name a schema (Assign Schema… in the XML menu), to see what the document allows."),
    );
    true
}

/// One thing the panel offers.
struct Entry {
    name: String,
    detail: Option<String>,
    mandatory: bool,
    used: bool,
    /// What choosing it does; none for what cannot be chosen (an attribute that is there already).
    insert: Option<Insert>,
}

/// A section of the panel.
struct Group {
    title: String,
    /// The grammar offers nothing here (whatever the filter says).
    empty: bool,
    entries: Vec<Entry>,
}

/// What the grammar offers at the caret, as sections of rows.
fn groups_of(choices: &Choices) -> Vec<Group> {
    let mut groups = vec![Group {
        title: crate::i18n::fmt(tl!("Elements in {name}"), &[("name", choices.parent.as_deref().unwrap_or("…"))]),
        empty: choices.elements.is_empty(),
        entries: choices
            .elements
            .iter()
            .map(|c| Entry {
                name: c.name.clone(),
                detail: c.empty.then(|| "EMPTY".to_owned()),
                mandatory: c.mandatory,
                used: false,
                insert: Some(Insert::Element(c.clone())),
            })
            .collect(),
    }];
    if let Some(element) = &choices.element {
        groups.push(Group {
            title: crate::i18n::fmt(tl!("Attributes of {name}"), &[("name", element)]),
            empty: choices.attributes.is_empty(),
            entries: choices
                .attributes
                .iter()
                .map(|AttributeChoice { name, mandatory, used, values, declare }| Entry {
                    name: name.clone(),
                    detail: (!values.is_empty()).then(|| values.join(" | ")),
                    mandatory: *mandatory,
                    used: *used,
                    insert: match (used, choices.attribute_at) {
                        (false, Some(at)) => {
                            Some(Insert::Attribute { name: name.clone(), at, first_value: values.first().cloned(), declare: declare.clone() })
                        }
                        _ => None,
                    },
                })
                .collect(),
        });
    }
    if !choices.values.is_empty()
        && let Some(element) = &choices.parent
    {
        groups.push(Group {
            title: crate::i18n::fmt(tl!("Values of {name}"), &[("name", element)]),
            empty: false,
            entries: choices
                .values
                .iter()
                .map(|v| Entry { name: v.clone(), detail: None, mandatory: false, used: false, insert: Some(Insert::Value(v.clone())) })
                .collect(),
        });
    }
    if !choices.entities.is_empty() {
        groups.push(Group {
            title: tl!("Entities").to_owned(),
            empty: false,
            entries: choices
                .entities
                .iter()
                .map(|EntityChoice { name, value }| Entry {
                    name: format!("&{name};"),
                    detail: value.as_deref().and_then(shown_value),
                    mandatory: false,
                    used: false,
                    insert: Some(Insert::Entity(name.clone())),
                })
                .collect(),
        });
    }
    groups
}

/// Keep what contains `filter` (which is lower case); with no filter, everything.
fn filter_groups(groups: &mut [Group], filter: &str) {
    if !filter.is_empty() {
        for group in groups {
            group.entries.retain(|e| e.name.to_lowercase().contains(filter));
        }
    }
}

/// Where in `groups` the entries that can be chosen are, in the order they are shown.
fn choosable(groups: &[Group]) -> Vec<(usize, usize)> {
    groups
        .iter()
        .enumerate()
        .flat_map(|(g, group)| group.entries.iter().enumerate().filter(|(_, e)| e.insert.is_some() && !e.used).map(move |(i, _)| (g, i)))
        .collect()
}

pub(crate) const FILTER_ID: &str = "xmlcraft-entry-filter";

pub(crate) fn body(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    let Some(doc) = app.session.active_document() else {
        hint(ui, tl!("What the DTD or the schema allows at the cursor appears here."));
        return;
    };
    let id = doc.id();
    // A request for the keyboard is for this frame only: when the lists cannot be shown yet it is dropped, not kept for later.
    let focus_filter = std::mem::take(&mut app.entry_focus);
    if waiting(ui, app, id) {
        return;
    }
    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(16, 6)).show(ui, |ui| {
        ui.add(widgets::checkbox(&mut app.entry_mandatory, tl!("Insert required content")));
    });
    let Some(choices) = app.helper_choices() else {
        widgets::empty_state(ui, 12.0, tl!("Nothing to show"), tl!("The document is too large for Entry Helpers."));
        return;
    };
    let filter = app.entry_filter.trim().to_lowercase();
    let mut groups = groups_of(&choices);
    filter_groups(&mut groups, &filter);
    let order = choosable(&groups);
    app.entry_selected = app.entry_selected.min(order.len().saturating_sub(1));
    // The keys of the filter field: up and down choose, Enter inserts the one chosen, Escape clears the filter and goes back to the text.
    let field_id = egui::Id::unique(FILTER_ID);
    let had_focus = ui.memory(|m| m.has_focus(field_id));
    let mut moved = false;
    let mut escape = false;
    if had_focus {
        let (down, up) =
            ui.input_mut(|i| (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown), i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)));
        if down && !order.is_empty() {
            app.entry_selected = (app.entry_selected + 1) % order.len();
            moved = true;
        }
        if up && !order.is_empty() {
            app.entry_selected = (app.entry_selected + order.len() - 1) % order.len();
            moved = true;
        }
    }
    let mut insert = None;
    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(16, 0)).show(ui, |ui| {
        let before = app.entry_filter.clone();
        let field = ui.add(widgets::line(&mut app.entry_filter).id(field_id).hint_text(tl!("Filter")).desired_width(f32::INFINITY));
        widgets::name_for_assistive_technology(ui, &field, tl!("Filter Entry Helpers"));
        if focus_filter {
            field.request_focus();
        }
        if app.entry_filter != before {
            app.entry_selected = 0;
        }
        // A single-line field gives up the keyboard at Escape, in the frame of the key.
        escape = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape));
        if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            insert = order.get(app.entry_selected).and_then(|(g, i)| groups.get(*g)?.entries.get(*i)?.insert.clone());
            if insert.is_some() {
                app.entry_filter.clear();
                app.entry_selected = 0;
            }
        }
    });
    if escape {
        app.entry_filter.clear();
        app.entry_selected = 0;
        app.focus_editor = true;
    }
    let selected = order.get(app.entry_selected).copied();
    let mut clear_filter = false;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for (g, group) in groups.iter().enumerate() {
            if !filter.is_empty() && group.entries.is_empty() {
                continue;
            }
            section(ui, &group.title, group.empty);
            for (i, entry) in group.entries.iter().enumerate() {
                let chosen = selected == Some((g, i)) && had_focus;
                let response = row(ui, &entry.name, entry.detail.as_deref(), entry.mandatory, entry.used, chosen, moved);
                if response.double_clicked()
                    && let Some(what) = &entry.insert
                {
                    insert = Some(what.clone());
                    clear_filter = true;
                }
            }
        }
        if !filter.is_empty() && groups.iter().all(|g| g.entries.is_empty()) {
            widgets::empty_state(ui, 12.0, tl!("No match"), tl!("Nothing offered here contains that."));
        }
    });
    if clear_filter {
        app.entry_filter.clear();
        app.entry_selected = 0;
    }
    match insert {
        Some(Insert::Element(choice)) => app.insert_element(&choice, &choices.scope),
        Some(Insert::Attribute { name, at, first_value, declare }) => app.insert_attribute(&name, at, first_value.as_deref(), declare.as_ref()),
        Some(Insert::Entity(name)) => app.insert_entity(&name),
        Some(Insert::Value(value)) => app.insert_value(&value),
        None => {}
    }
}

/// A section heading, with a note when there is nothing in it.
fn section(ui: &mut egui::Ui, title: &str, empty: bool) {
    let t = Tokens::get(ui.ctx());
    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(16, 0)).show(ui, |ui| {
        widgets::section_title(ui, title);
        if empty {
            ui.label(egui::RichText::new(tl!("Nothing here")).font(theme::regular(12.5)).color(t.text_muted));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_value_of_an_entity_is_shown_when_it_can_be_drawn() {
        assert_eq!(shown_value("text").as_deref(), Some("text"));
        assert_eq!(shown_value("© 2026 é").as_deref(), Some("© 2026 é"));
        assert_eq!(shown_value("Ωμέγα и €").as_deref(), Some("Ωμέγα и €"));
        assert_eq!(shown_value(&"x".repeat(40)).map(|s| s.len()), Some(24), "the first 24 characters");
        // Not drawn: a symbol outside the fonts, a non-breaking space, a control character, nothing at all.
        for undrawable in ["\u{2118}", "a\u{2211}b", "\u{00A0}", "a\u{0007}", "", "\u{1F600}"] {
            assert_eq!(shown_value(undrawable), None, "{undrawable:?}");
        }
        // The cut is made before the check: a symbol past the 24th character is not looked at.
        assert_eq!(shown_value(&format!("{}\u{2118}", "x".repeat(24))).map(|s| s.len()), Some(24));
    }

    #[test]
    fn asking_again_for_a_schema_forgets_the_choices_worked_out_from_the_old_one() {
        let mut state = HelperState::default();
        let _ = state.choices(1, 0, String::new);
        assert!(state.cache.is_some(), "the choices at this place are kept");
        state.forget_requests();
        assert!(state.cache.is_none(), "the same key may now stand for other content: they are worked out again");
    }

    #[test]
    fn only_the_answer_to_the_last_request_is_taken() {
        let mut state = HelperState { requested_key: Some(2), ..HelperState::default() };
        let grammar = Some(Arc::new(Grammar::new(xmlcraft_grammar::SchemaKind::Dtd)));
        assert!(!state.accept(Schema { key: 1, grammar: grammar.clone() }), "an older request");
        assert!(state.grammar.is_none() && state.grammar_key.is_none());
        assert!(state.accept(Schema { key: 2, grammar }));
        assert!(state.grammar.is_some() && state.grammar_key == Some(2));
        // A DOCTYPE that cannot be read replaces the grammar with none.
        state.requested_key = Some(3);
        assert!(state.accept(Schema { key: 3, grammar: None }));
        assert!(state.grammar.is_none() && state.grammar_key == Some(3));
    }

    #[test]
    fn only_the_answer_to_the_last_request_for_a_schema_is_taken() {
        let mut state = HelperState { requested_xsd_key: Some(2), ..HelperState::default() };
        let schema = || {
            Ok(Arc::new(
                xmlcraft_xsd::assemble(&xmlcraft_xsd::load(
                    "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"/>",
                    "",
                    &mut xmlcraft_xsd::NoLoader,
                ))
                .schema,
            ))
        };
        assert!(!state.accept_xsd(XsdAnswer { key: 1, schema: schema() }), "an older request");
        assert!(state.xsd.is_none() && state.xsd_key.is_none());
        assert!(state.accept_xsd(XsdAnswer { key: 2, schema: schema() }));
        assert!(state.xsd.is_some() && state.xsd_key == Some(2) && state.xsd_error.is_none());
        // A schema that cannot be loaded replaces the one there was, and says why.
        state.requested_xsd_key = Some(3);
        assert!(state.accept_xsd(XsdAnswer { key: 3, schema: Err("it is gone".to_owned()) }));
        assert!(state.xsd.is_none() && state.xsd_key == Some(3) && state.xsd_error.as_deref() == Some("it is gone"));
        // The document stops naming one: nothing is held.
        state.forget_xsd();
        assert!((state.xsd.is_none(), state.xsd_key, state.requested_xsd_key, state.xsd_error.is_none()) == (true, None, None, true));
    }
}
