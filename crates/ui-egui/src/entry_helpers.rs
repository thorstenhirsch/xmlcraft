//! Entry Helpers: what the DTD allows at the caret (the elements that may come, the attributes of the element, the entities), and a
//! double click that puts the choice in the text.

use std::sync::Arc;

use egui::{Sense, vec2};
use xmlcraft_complete::{AttributeChoice, Choices, ElementChoice, EntityChoice, attribute_snippet, choices_at, element_snippet, entity_snippet};
use xmlcraft_engine::schema::Schema;
use xmlcraft_grammar::Grammar;
use xmlcraft_text::Edit;

use crate::i18n::tl;
use crate::theme::{self, Tokens};
use crate::{XmlCraftApp, widgets};

/// Documents larger than this get no Entry Helpers: they are worked out from the whole text each time the caret moves.
const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
const ROW: f32 = 24.0;

/// The grammar of a document's DTD, and the choices worked out from it for one place in one version of the text.
#[derive(Default)]
pub(crate) struct HelperState {
    /// The request the grammar answers (see `xmlcraft_engine::schema::key_of`).
    pub(crate) grammar_key: Option<u64>,
    pub(crate) grammar: Option<Arc<Grammar>>,
    /// The last request for a grammar: not asked again for the same DOCTYPE at the same place.
    pub(crate) requested_key: Option<u64>,
    /// The version of the text a request was last looked for in.
    pub(crate) checked_version: Option<u64>,
    cache: Option<(u64, usize, Option<u64>, Option<Choices>)>,
}

impl HelperState {
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

    /// The choices at `offset` of the text (version `version`), worked out again only when something they depend on changed.
    fn choices(&mut self, version: u64, offset: usize, text: impl FnOnce() -> String) -> Option<&Choices> {
        let key = (version, offset, self.grammar_key);
        if self.cache.as_ref().is_none_or(|(v, o, g, _)| (*v, *o, *g) != key) {
            let value = self.grammar.as_ref().and_then(|grammar| {
                let text = text();
                (text.len() <= MAX_TEXT_BYTES).then(|| choices_at(&xmlcraft_syntax::parse(&text).syntax(), offset, grammar))
            });
            self.cache = Some((key.0, key.1, key.2, value));
        }
        self.cache.as_ref().and_then(|(_, _, _, value)| value.as_ref())
    }
}

impl XmlCraftApp {
    /// What the DTD allows at the caret of the document being shown, if it has a DTD that was read.
    fn helper_choices(&mut self) -> Option<Choices> {
        let doc = self.session.active_document()?;
        let (id, version, caret) = (doc.id(), doc.editor().version(), doc.editor().primary().head());
        let view = self.views.get_mut(&id)?;
        view.helpers.choices(version, caret, || doc.editor().text()).cloned()
    }

    /// Put an element at the caret, with what its declaration requires when the option is on.
    pub(crate) fn insert_element(&mut self, name: &str) {
        let Some(grammar) = self.active_grammar() else { return };
        let snippet = element_snippet(&grammar, name, self.entry_mandatory);
        self.insert_at_selection(snippet.text, snippet.caret);
    }

    /// Put an entity reference at the caret.
    pub(crate) fn insert_entity(&mut self, name: &str) {
        let snippet = entity_snippet(name);
        self.insert_at_selection(snippet.text, snippet.caret);
    }

    /// Add an attribute to the start tag of the element at `at` (the end of the name and the attributes already there).
    pub(crate) fn insert_attribute(&mut self, name: &str, at: usize, first_value: Option<&str>) {
        let snippet = attribute_snippet(name, first_value);
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
fn row(ui: &mut egui::Ui, name: &str, detail: Option<&str>, mandatory: bool, used: bool) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    let label = if mandatory { format!("{name} (required)") } else { name.to_owned() };
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, !used, label.clone()));
    if response.hovered() && !used {
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

/// What a click on a row asks for.
enum Insert {
    Element(String),
    Attribute { name: String, at: usize, first_value: Option<String> },
    Entity(String),
}

pub(crate) fn body(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    let Some(doc) = app.session.active_document() else {
        hint(ui, tl!("What the DTD allows at the cursor appears here."));
        return;
    };
    let id = doc.id();
    let has_doctype = app.views.get(&id).and_then(|v| v.analysis.as_ref()).is_some_and(|a| a.doctype.is_some());
    if !has_doctype {
        widgets::empty_state(ui, 24.0, tl!("No DTD"), tl!("Add a DOCTYPE to the document to see what it allows."));
        return;
    }
    if let Some(helpers) = app.views.get(&id).map(|v| &v.helpers).filter(|h| h.grammar.is_none()) {
        // The answer to the last request is in, and there is no grammar in it: the DOCTYPE could not be read.
        if helpers.grammar_key.is_some() && helpers.grammar_key == helpers.requested_key {
            widgets::empty_state(ui, 24.0, tl!("No DTD to show"), tl!("The DOCTYPE is too large to be read."));
        } else {
            widgets::empty_state(ui, 24.0, tl!("Reading the DTD…"), "");
        }
        return;
    }
    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(16, 6)).show(ui, |ui| {
        ui.add(widgets::checkbox(&mut app.entry_mandatory, tl!("Insert required content")));
    });
    let Some(choices) = app.helper_choices() else {
        widgets::empty_state(ui, 12.0, tl!("Nothing to show"), tl!("The document is too large for Entry Helpers."));
        return;
    };
    let mut insert = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        section(ui, &crate::i18n::fmt(tl!("Elements in {name}"), &[("name", choices.parent.as_deref().unwrap_or("…"))]), choices.elements.is_empty());
        for ElementChoice { name, mandatory, empty } in &choices.elements {
            let detail = empty.then_some("EMPTY");
            if row(ui, name, detail, *mandatory, false).double_clicked() {
                insert = Some(Insert::Element(name.clone()));
            }
        }
        if let Some(element) = &choices.element {
            section(ui, &crate::i18n::fmt(tl!("Attributes of {name}"), &[("name", element)]), choices.attributes.is_empty());
            for AttributeChoice { name, mandatory, used, values } in &choices.attributes {
                let detail = (!values.is_empty()).then(|| values.join(" | "));
                if row(ui, name, detail.as_deref(), *mandatory, *used).double_clicked()
                    && !used
                    && let Some(at) = choices.attribute_at
                {
                    insert = Some(Insert::Attribute { name: name.clone(), at, first_value: values.first().cloned() });
                }
            }
        }
        if !choices.entities.is_empty() {
            section(ui, tl!("Entities"), false);
            for EntityChoice { name, value } in &choices.entities {
                let shown = value.as_deref().map(|v| v.chars().take(24).collect::<String>());
                if row(ui, &format!("&{name};"), shown.as_deref(), false, false).double_clicked() {
                    insert = Some(Insert::Entity(name.clone()));
                }
            }
        }
    });
    match insert {
        Some(Insert::Element(name)) => app.insert_element(&name),
        Some(Insert::Attribute { name, at, first_value }) => app.insert_attribute(&name, at, first_value.as_deref()),
        Some(Insert::Entity(name)) => app.insert_entity(&name),
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
}
