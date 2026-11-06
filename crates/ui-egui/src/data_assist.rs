//! Completion and hover for JSON, JSONC, JSON5, JSON Lines and YAML documents, from the JSON Schema they name: the schema is loaded in
//! the background when what the document names changes (as the XML Schema is for Entry Helpers), and the popup and the tip use it.

use std::sync::Arc;

use xmlcraft_engine::json_assist::{CompletionKind, complete_with, hover_with};
use xmlcraft_engine::json_schemas::{JsonAnswer, SchemaSet, json_key};
use xmlcraft_engine::json_validation::schema_reference;

use crate::XmlCraftApp;
use crate::completion::{Completion, Item};
use crate::i18n::tl;

/// A text longer than this is not read for the schema it names, nor completed.
const MAX_ASSIST_BYTES: usize = 2 * 1024 * 1024;

/// What a JSON or YAML document has of the schema it names.
#[derive(Default)]
pub(crate) struct DataSchema {
    /// The version of the text the name of the schema was read from, and what it was (none: the document names none).
    reference_version: Option<u64>,
    reference: Option<String>,
    /// The request that was last made, and the request the schema held answers.
    requested_key: Option<u64>,
    key: Option<u64>,
    pub(crate) set: Option<Arc<SchemaSet>>,
    /// Why the schema could not be read.
    pub(crate) error: Option<String>,
}

impl DataSchema {
    /// Take the answer to a request, if it is the answer to the last one made.
    pub(crate) fn accept(&mut self, answer: JsonAnswer) -> bool {
        if self.requested_key != Some(answer.key) {
            return false;
        }
        self.key = Some(answer.key);
        match answer.schema {
            Ok(set) => {
                self.set = Some(set);
                self.error = None;
            }
            Err(why) => {
                self.set = None;
                self.error = Some(why);
            }
        }
        true
    }

    /// What the text of version `version` names as its schema (it was read from that very text), if it names one.
    pub(crate) fn reference_if_current(&self, version: u64) -> Option<String> {
        if self.reference_version == Some(version) { self.reference.clone() } else { None }
    }

    /// Whether the text of version `version` names a schema (the name was read from that very text).
    pub(crate) fn names_a_schema_at(&self, version: u64) -> bool {
        self.reference_version == Some(version) && self.reference.is_some()
    }

    /// The document names no schema (any more): none is held.
    fn forget(&mut self) {
        self.set = None;
        self.error = None;
        self.key = None;
        self.requested_key = None;
    }

    /// What was asked for is asked for again (what the answer depends on, such as the downloads, changed).
    pub(crate) fn forget_request(&mut self) {
        self.requested_key = None;
    }
}

impl XmlCraftApp {
    /// Whether the JSON Schema the document being shown names has been loaded (completion and hover come from it).
    pub fn has_data_schema(&self) -> bool {
        let Some(id) = self.session.active_document().map(xmlcraft_engine::Document::id) else { return false };
        self.views.get(&id).is_some_and(|v| v.data_schema.set.is_some())
    }

    /// Ask for the JSON Schema the document being shown names when it is not the one that was asked for, or the place the document is
    /// at, or the other documents that are open (one may be the schema, being edited) are not as they were. Looked at every frame.
    pub(crate) fn request_json_schema(&mut self) {
        let Some(doc) = self.session.active_document() else { return };
        let language = doc.language();
        if !(language.is_json_like() || language.is_yaml()) {
            return;
        }
        let (id, version) = (doc.id(), doc.editor().version());
        let Some(view) = self.views.get_mut(&id) else { return };
        if view.data_schema.reference_version != Some(version) {
            view.data_schema.reference_version = Some(version);
            view.data_schema.reference =
                (doc.editor().buffer().len() <= MAX_ASSIST_BYTES).then(|| schema_reference(&doc.editor().text(), language)).flatten();
        }
        let Some(reference) = view.data_schema.reference.clone() else {
            view.data_schema.forget();
            return;
        };
        let versions: Vec<(String, u64)> = self
            .session
            .documents()
            .iter()
            .filter(|d| d.id() != id)
            .filter_map(|d| d.path().map(|p| (p.to_string_lossy().into_owned(), d.editor().version())))
            .collect();
        let key = json_key(&reference, doc.path(), &versions);
        if view.data_schema.requested_key == Some(key) {
            return;
        }
        view.data_schema.requested_key = Some(key);
        let open = self
            .session
            .documents()
            .iter()
            .filter_map(|d| d.path().map(|p| (p.to_string_lossy().into_owned(), d.editor().buffer().clone())))
            .collect();
        self.json_schemas.submit(id, key, reference, doc.path(), open);
    }

    /// Collect the schemas that finished loading.
    pub(crate) fn pump_json_schemas(&mut self) {
        for (id, answer) in self.json_schemas.poll() {
            if let Some(view) = self.views.get_mut(&id) {
                view.data_schema.accept(answer);
            }
        }
    }

    /// After typing in a JSON or YAML document: open the popup with what the schema allows where the caret is, or close it. Returns
    /// false when the document is not of these languages (the XML completion is for it). The popup opens only inside a name or value
    /// being typed (a string or a word), so that Enter after a bracket is a line break, not the choice of a name.
    pub(crate) fn update_data_completion(&mut self) -> bool {
        let Some(doc) = self.session.active_document() else { return false };
        let language = doc.language();
        if !(language.is_json_like() || language.is_yaml()) {
            return false;
        }
        self.completion = None;
        let editor = doc.editor();
        let caret = editor.primary();
        if editor.carets().len() != 1 || !caret.is_empty() || editor.buffer().len() > MAX_ASSIST_BYTES {
            return true;
        }
        let (id, version, head) = (doc.id(), editor.version(), caret.head());
        let Some(set) = self.views.get(&id).and_then(|v| v.data_schema.set.clone()) else { return true };
        let offered = complete_with(&set, &editor.text(), language, head);
        let Some(first) = offered.first() else { return true };
        if first.replace.is_empty() {
            return true;
        }
        let (start, end) = (first.replace.start, first.replace.end.max(head));
        let items: Vec<Item> = offered
            .iter()
            .map(|c| {
                let mut detail = c.detail.clone();
                if c.kind == CompletionKind::Property && c.required {
                    detail = if detail.is_empty() { tl!("required").to_owned() } else { format!("{detail} · {}", tl!("required")) };
                }
                Item { label: c.label.clone(), detail, text: c.insert.clone(), caret: c.cursor }
            })
            .collect();
        self.completion = Some(Completion { doc: id, version, start, caret: head, end, items, selected: 0 });
        true
    }

    /// What the schema the document being shown names says of the name or value at `offset`, for the tip where the pointer rests.
    pub(crate) fn data_hover(&self, offset: usize) -> Option<String> {
        let doc = self.session.active_document()?;
        let language = doc.language();
        if !(language.is_json_like() || language.is_yaml()) || doc.editor().buffer().len() > MAX_ASSIST_BYTES {
            return None;
        }
        let set = self.views.get(&doc.id())?.data_schema.set.clone()?;
        hover_with(&set, &doc.editor().text(), language, offset).map(|h| h.text)
    }
}
