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
    if has_hints {
        if helpers.xsd.is_some() {
            return false;
        }
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

pub(crate) fn body(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    let Some(doc) = app.session.active_document() else {
        hint(ui, tl!("What the DTD or the schema allows at the cursor appears here."));
        return;
    };
    let id = doc.id();
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
    let mut insert = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        section(ui, &crate::i18n::fmt(tl!("Elements in {name}"), &[("name", choices.parent.as_deref().unwrap_or("…"))]), choices.elements.is_empty());
        for choice in &choices.elements {
            let detail = choice.empty.then_some("EMPTY");
            if row(ui, &choice.name, detail, choice.mandatory, false).double_clicked() {
                insert = Some(Insert::Element(choice.clone()));
            }
        }
        if let Some(element) = &choices.element {
            section(ui, &crate::i18n::fmt(tl!("Attributes of {name}"), &[("name", element)]), choices.attributes.is_empty());
            for AttributeChoice { name, mandatory, used, values, declare } in &choices.attributes {
                let detail = (!values.is_empty()).then(|| values.join(" | "));
                if row(ui, name, detail.as_deref(), *mandatory, *used).double_clicked()
                    && !used
                    && let Some(at) = choices.attribute_at
                {
                    insert = Some(Insert::Attribute { name: name.clone(), at, first_value: values.first().cloned(), declare: declare.clone() });
                }
            }
        }
        if !choices.values.is_empty()
            && let Some(element) = &choices.parent
        {
            section(ui, &crate::i18n::fmt(tl!("Values of {name}"), &[("name", element)]), false);
            for value in &choices.values {
                if row(ui, value, None, false, false).double_clicked() {
                    insert = Some(Insert::Value(value.clone()));
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
