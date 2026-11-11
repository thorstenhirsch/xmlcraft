//! Completion while typing: after `<` the elements the grammar allows, after `&` its entities, a few letters into an attribute name the
//! attributes, inside the quotes of an attribute the values of its type. The list is a popup at the caret; the arrow keys choose, Enter or
//! Tab writes the choice (one undoable step), Escape or moving away closes it. It is the Entry Helpers' answer where the caret is, so it
//! needs the DTD or the schema the helpers read; without one, element names already used in the document are offered after `<`.

use xmlcraft_complete::{Choices, attribute_snippet, entity_snippet};
use xmlcraft_engine::xml_complete::{DocumentChoices, Kind, context, document_choices};
use xmlcraft_text::Edit;

use crate::XmlCraftApp;
use crate::i18n::tl;
use crate::theme::{self, Tokens};

/// How many rows the popup shows at most.
const MAX_ROWS: usize = 8;

/// One row of the popup.
#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub label: String,
    pub detail: String,
    /// What is written in place of what was typed, and where the caret goes in it.
    pub text: String,
    pub caret: usize,
}

/// The popup that is open.
pub(crate) struct Completion {
    /// The document it is for, and the version of its text it was worked out for.
    pub doc: u64,
    pub version: u64,
    /// The offset in the document where what is written replaces, and the caret (where it ends).
    pub start: usize,
    pub caret: usize,
    /// The end of what the choice replaces: the caret, or, in a JSON string that goes on after it, the end of the string.
    pub end: usize,
    pub items: Vec<Item>,
    pub selected: usize,
}

/// Keep the items that fit what was typed: those that start with it, then those that contain it (any case); all of them when nothing
/// was typed.
fn narrowed(items: Vec<Item>, partial: &str) -> Vec<Item> {
    if partial.is_empty() {
        return items;
    }
    let p = partial.to_lowercase();
    let (mut starts, mut contains) = (Vec::new(), Vec::new());
    for item in items {
        let l = item.label.trim_start_matches(['<', '&']).to_lowercase();
        if l.starts_with(&p) {
            starts.push(item);
        } else if l.contains(&p) {
            contains.push(item);
        }
    }
    starts.extend(contains);
    starts
}

impl XmlCraftApp {
    /// The items for `kind` from what the grammar allows at the caret.
    fn completion_items(&mut self, kind: &Kind, choices: Option<&Choices>, partial: &str) -> Vec<Item> {
        match (kind, choices) {
            (Kind::Element, Some(choices)) => choices
                .elements
                .iter()
                .filter_map(|c| {
                    let snippet = self.element_snippet_for(c, &choices.scope)?;
                    Some(Item {
                        label: c.name.clone(),
                        detail: if c.mandatory {
                            tl!("required").to_owned()
                        } else if c.empty {
                            "EMPTY".to_owned()
                        } else {
                            String::new()
                        },
                        text: snippet.text,
                        caret: snippet.caret,
                    })
                })
                .collect(),
            (Kind::Element, None) => self.names_in_the_document(false),
            (Kind::EndTag, _) => self.names_in_the_document(true),
            (Kind::Attribute, Some(choices)) => choices
                .attributes
                .iter()
                .filter(|a| !a.used)
                .map(|a| {
                    // (An attribute of another namespace brings the declaration of its prefix.)
                    let snippet = match &a.declare {
                        Some(_) => xmlcraft_complete::xsd_attribute_snippet(&a.name, a.values.first().map(String::as_str), a.declare.as_ref()),
                        None => attribute_snippet(&a.name, a.values.first().map(String::as_str)),
                    };
                    // The text starts with the space that separates attributes: there is one before the word already.
                    Item {
                        label: a.name.clone(),
                        detail: if a.mandatory { tl!("required").to_owned() } else { String::new() },
                        text: snippet.text.trim_start().to_owned(),
                        caret: snippet.caret.saturating_sub(1),
                    }
                })
                .collect(),
            (Kind::Attribute, None) => {
                let choices = self.document_choices();
                choices
                    .attributes
                    .iter()
                    .filter(|name| name.as_str() != partial)
                    .map(|name| {
                        let snippet = attribute_snippet(name, None);
                        Item {
                            label: name.clone(),
                            detail: tl!("used in this document").to_owned(),
                            text: snippet.text.trim_start().to_owned(),
                            caret: snippet.caret.saturating_sub(1),
                        }
                    })
                    .collect()
            }
            (Kind::Value(name), Some(choices)) => {
                let Some(attribute) = choices.attributes.iter().find(|a| &a.name == name) else { return Vec::new() };
                if !attribute.list {
                    return attribute
                        .values
                        .iter()
                        .map(|v| Item { label: v.clone(), detail: String::new(), text: v.clone(), caret: v.len() })
                        .collect();
                }
                // A list (IDREFS): what is typed is one of its values, and a choice adds to the ones before it.
                let token_start = partial
                    .rfind(char::is_whitespace)
                    .map_or(0, |i| i.saturating_add(partial.get(i..).and_then(|rest| rest.chars().next()).map_or(0, char::len_utf8)));
                let before = partial.get(..token_start).unwrap_or_default();
                let used: Vec<&str> = before.split_whitespace().collect();
                attribute
                    .values
                    .iter()
                    .filter(|v| !used.contains(&v.as_str()))
                    .map(|v| {
                        let text = format!("{before}{v}");
                        Item { label: text.clone(), detail: String::new(), caret: text.len(), text }
                    })
                    .collect()
            }
            (Kind::Value(name), None) => {
                let choices = self.document_choices();
                choices
                    .values
                    .get(name)
                    .into_iter()
                    .flatten()
                    .map(|value| Item {
                        label: value.clone(),
                        detail: tl!("used in this document").to_owned(),
                        text: value.clone(),
                        caret: value.len(),
                    })
                    .collect()
            }
            (Kind::Entity, Some(choices)) => {
                // Inside the start tag (an attribute value) the choices list none: the grammar's own entities are what a value may refer to.
                let entities = match (choices.entities.is_empty(), self.active_grammar()) {
                    // (An attribute value cannot refer to an external entity or to one whose text holds `<`.)
                    (true, Some(grammar)) => {
                        xmlcraft_complete::entities(&grammar).into_iter().filter(|e| e.value.as_deref().is_some_and(|v| !v.contains('<'))).collect()
                    }
                    _ => choices.entities.clone(),
                };
                entities
                    .iter()
                    .map(|e| {
                        let snippet = entity_snippet(&e.name);
                        Item {
                            label: format!("&{};", e.name),
                            detail: e.value.as_deref().and_then(crate::entry_helpers::shown_value).unwrap_or_default(),
                            text: snippet.text,
                            caret: snippet.caret,
                        }
                    })
                    .collect()
            }
            (Kind::Entity, None) => self
                .document_choices()
                .entities
                .iter()
                .map(|name| {
                    let snippet = entity_snippet(name);
                    Item { label: format!("&{name};"), detail: tl!("used in this document").to_owned(), text: snippet.text, caret: snippet.caret }
                })
                .collect(),
        }
    }

    /// Without a grammar: the element names this document uses, written as a start tag and an end tag, or as the name an end tag needs.
    fn names_in_the_document(&self, end_tag: bool) -> Vec<Item> {
        let Some(analysis) = self.active_analysis() else { return Vec::new() };
        let mut names: Vec<&str> = analysis.outline.iter().map(|i| i.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        names
            .into_iter()
            .take(xmlcraft_engine::xml_complete::MAX_NAMES)
            .map(|n| {
                let (text, caret) = if end_tag { (format!("{n}>"), n.len() + 1) } else { (format!("<{n}></{n}>"), n.len() + 2) };
                Item { label: n.to_owned(), detail: tl!("used in this document").to_owned(), text, caret }
            })
            .collect()
    }

    /// The names the document being shown uses.
    fn document_choices(&self) -> DocumentChoices {
        self.session.active_document().map(|doc| document_choices(&doc.editor().text())).unwrap_or_default()
    }

    /// After typing in an HTML document: open the popup with the elements, attributes or values that fit where the caret is, or close it.
    /// Returns false when the document is not HTML.
    fn update_html_completion(&mut self) -> bool {
        let Some(doc) = self.session.active_document() else { return false };
        if !doc.language().is_html() {
            return false;
        }
        self.completion = None;
        let editor = doc.editor();
        let caret = editor.primary();
        if editor.carets().len() != 1 || !caret.is_empty() || editor.buffer().len() > 2 * 1024 * 1024 {
            return true;
        }
        let (id, version, head) = (doc.id(), editor.version(), caret.head());
        let Some(found) = xmlcraft_engine::html_complete::complete(&editor.text(), head) else { return true };
        if found.items.is_empty() {
            return true;
        }
        let items: Vec<Item> = found.items.into_iter().map(|i| Item { label: i.label, detail: i.detail, text: i.insert, caret: i.cursor }).collect();
        self.completion =
            Some(Completion { doc: id, version, start: found.replace.start, caret: head, end: found.replace.end.max(head), items, selected: 0 });
        true
    }

    /// After typing in an XSLT stylesheet: open the popup with the `xsl:` elements, attributes, values or XPath functions that fit where the
    /// caret is. Returns true when it did; otherwise the other kinds of completion go on (a literal result element has names of its own).
    fn update_xslt_completion(&mut self) -> bool {
        let Some(doc) = self.session.active_document() else { return false };
        if !doc.language().is_xml() {
            return false;
        }
        let editor = doc.editor();
        let caret = editor.primary();
        if editor.carets().len() != 1 || !caret.is_empty() || editor.buffer().len() > 2 * 1024 * 1024 {
            return false;
        }
        let (id, version, head) = (doc.id(), editor.version(), caret.head());
        let Some(found) = xmlcraft_engine::xslt_complete::complete(&editor.text(), head, self.completion_explicit) else { return false };
        let items: Vec<Item> = found.items.into_iter().map(|i| Item { label: i.label, detail: i.detail, text: i.insert, caret: i.cursor }).collect();
        self.completion =
            Some(Completion { doc: id, version, start: found.replace.start, caret: head, end: found.replace.end.max(head), items, selected: 0 });
        true
    }

    /// Edit ▸ Complete: open the popup at the caret whether or not anything has been typed (between names, after a colon). Without
    /// anything to offer there it says so.
    pub(crate) fn complete_at_caret(&mut self) {
        self.completion_explicit = true;
        self.update_completion();
        self.completion_explicit = false;
        if self.completion.is_none() {
            self.status = tl!("Nothing to complete here.").to_owned();
        } else {
            self.focus_editor = true;
        }
    }

    /// Work out what to offer at the caret after the frame's typing, and open or close the popup.
    pub(crate) fn update_completion(&mut self) {
        // JSON and YAML documents are completed from their JSON Schema.
        if self.update_data_completion() || self.update_html_completion() || self.update_xslt_completion() {
            return;
        }
        let Some(doc) = self.session.active_document_mut() else {
            self.completion = None;
            return;
        };
        let id = doc.id();
        let editor = doc.editor_mut();
        let version = editor.version();
        let caret = editor.primary();
        if editor.carets().len() != 1 || !caret.is_empty() {
            self.completion = None;
            return;
        }
        let head = caret.head();
        let explicit = self.completion_explicit;
        let text = editor.text();
        let Some(ctx) = context(&text, head, explicit) else {
            self.completion = None;
            return;
        };
        // An element goes where the `<` is (what is typed is not a tag yet); the rest is about the place of the caret.
        let choices = if ctx.kind == Kind::Element { self.helper_choices_at(ctx.start) } else { self.helper_choices() };
        let mut items = narrowed(self.completion_items(&ctx.kind, choices.as_ref(), &ctx.partial), &ctx.partial);
        if items.is_empty() {
            self.completion = None;
            return;
        }
        items.truncate(200);
        self.completion = Some(Completion { doc: id, version, start: ctx.start, caret: head, end: head, items, selected: 0 });
    }

    /// Whether the popup that is open is about the text as it is in the document being shown.
    fn completion_is_current(&self) -> bool {
        let Some(c) = &self.completion else { return false };
        self.session.active_document().is_some_and(|d| d.id() == c.doc && d.editor().version() == c.version && d.editor().primary().head() == c.caret)
    }

    /// Before the editor is drawn: the keys of the popup. Arrow keys choose, Enter and Tab write the choice, Escape closes it; they are
    /// taken from the frame's input so that the editor does not also act on them.
    pub(crate) fn completion_keys(&mut self, ui: &mut egui::Ui) {
        // The keys are the editor's: with the keyboard in the find bar, the palette or a dialog the popup is not theirs to take them.
        if self.completion.is_some() && (!self.completion_is_current() || !self.editor_has_focus) {
            self.completion = None;
        }
        let Some(c) = &mut self.completion else { return };
        use egui::{Key, Modifiers};
        let (down, up, accept, tab, escape) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Tab),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        let n = c.items.len().max(1);
        if down {
            c.selected = (c.selected + 1) % n;
        }
        if up {
            c.selected = (c.selected + n - 1) % n;
        }
        if escape {
            self.completion = None;
        } else if accept || tab {
            self.accept_completion();
        }
    }

    /// Write the chosen item in place of what was typed.
    pub(crate) fn accept_completion(&mut self) {
        let Some(c) = self.completion.take() else { return };
        let Some(item) = c.items.get(c.selected) else { return };
        let Some(doc) = self.session.active_document_mut() else { return };
        let editor = doc.editor_mut();
        if c.end > editor.buffer().len() || c.start > c.caret || c.caret > c.end {
            return;
        }
        if editor.apply_edits(&[Edit::replace(c.start..c.end, item.text.clone())]) {
            let at = (c.start + item.caret).min(editor.buffer().len());
            editor.set_selection(at, at);
            self.focus_editor = true;
        }
    }

    /// Draw the popup under the caret.
    pub(crate) fn show_completion(&mut self, ctx: &egui::Context) {
        if !self.completion_is_current() {
            self.completion = None;
            return;
        }
        let (Some(c), Some(geometry), Some(doc)) = (&self.completion, self.editor_geometry(), self.session.active_document()) else { return };
        let editor = doc.editor();
        let buffer = editor.buffer();
        let line = buffer.line_of(c.caret);
        let column = xmlcraft_editor::motion::display_column(buffer, c.caret, editor.settings().tab_width);
        let at = geometry.point(editor.row_of_line(line) + 1, column);
        let t = Tokens::get(ctx);
        let shown = c.items.len().min(MAX_ROWS);
        let first = c.selected.saturating_sub(MAX_ROWS - 1).min(c.items.len().saturating_sub(shown));
        let mut picked: Option<usize> = None;
        egui::Area::new(egui::Id::unique("xmlcraft-completion")).order(egui::Order::Foreground).fixed_pos(at + egui::vec2(0.0, 2.0)).show(
            ctx,
            |ui| {
                egui::Frame::NONE
                    .fill(t.card)
                    .stroke(egui::Stroke::new(1.0, t.border))
                    .corner_radius(egui::CornerRadius::same(8))
                    .shadow(ctx.global_style().visuals.popup_shadow)
                    .inner_margin(egui::Margin::symmetric(4, 4))
                    .show(ui, |ui| {
                        ui.set_width(280.0);
                        for (i, item) in c.items.iter().enumerate().skip(first).take(shown) {
                            let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), egui::Sense::click());
                            let chosen = i == c.selected;
                            if response.clicked() {
                                picked = Some(i);
                            }
                            response.widget_info(|| {
                                egui::WidgetInfo::selected(egui::Role::ListBoxOption, true, chosen, format!("Complete {}", item.label))
                            });
                            if chosen {
                                ui.painter().rect_filled(rect, 5.0, t.accent_soft);
                            }
                            ui.painter().text(
                                rect.left_center() + egui::vec2(8.0, 0.0),
                                egui::Align2::LEFT_CENTER,
                                &item.label,
                                theme::mono(12.5),
                                if chosen { t.accent_text } else { t.text },
                            );
                            if !item.detail.is_empty() {
                                ui.painter().text(
                                    rect.right_center() - egui::vec2(8.0, 0.0),
                                    egui::Align2::RIGHT_CENTER,
                                    &item.detail,
                                    theme::regular(11.0),
                                    t.text_faint,
                                );
                            }
                        }
                    });
            },
        );
        // A click on a row writes it.
        if let Some(i) = picked {
            if let Some(c) = &mut self.completion {
                c.selected = i;
            }
            self.accept_completion();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_was_typed_narrows_the_items_prefix_first() {
        let item = |l: &str| Item { label: l.to_owned(), detail: String::new(), text: l.to_owned(), caret: l.len() };
        let items = vec![item("book"), item("notebook"), item("Boat"), item("title")];
        let labels = |v: Vec<Item>| v.into_iter().map(|i| i.label).collect::<Vec<_>>();
        assert_eq!(labels(narrowed(items.clone(), "")), ["book", "notebook", "Boat", "title"]);
        assert_eq!(labels(narrowed(items.clone(), "bo")), ["book", "Boat", "notebook"], "prefixes first, then what contains it; any case");
        assert!(narrowed(items, "zz").is_empty());
    }
}
