//! Completion while typing: after `<` the elements the grammar allows, after `&` its entities, a few letters into an attribute name the
//! attributes, inside the quotes of an attribute the values of its type. The list is a popup at the caret; the arrow keys choose, Enter or
//! Tab writes the choice (one undoable step), Escape or moving away closes it. It is the Entry Helpers' answer where the caret is, so it
//! needs the DTD or the schema the helpers read; without one, element names already used in the document are offered after `<`.

use xmlcraft_complete::{Choices, attribute_snippet, entity_snippet};
use xmlcraft_text::Edit;

use crate::XmlCraftApp;
use crate::i18n::tl;
use crate::theme::{self, Tokens};

/// How many rows the popup shows at most.
const MAX_ROWS: usize = 8;

/// What is being completed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Element,
    Attribute,
    /// The value of the attribute with this name.
    Value(String),
    Entity,
}

/// Where the caret is in the text, as far as completion is concerned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Context {
    pub kind: Kind,
    /// The byte offset in the document where what the choice replaces starts: the `<` or `&` of an element or an entity, the first letter
    /// of an attribute name, the first character after the quote of a value.
    pub start: usize,
    /// What is typed of the name so far.
    pub partial: String,
}

/// The most characters before the caret looked at to find the name of an attribute whose value is open.
const LOOK_BACK: usize = 4096;

/// What to complete at the caret of `editor` (a single empty caret), after the typing that put it there. Nothing for a place completion
/// has nothing to say about: text, comments, CDATA, a value that is not an attribute's, white space between attributes. Where the caret is
/// in the markup comes from the lexer, so a `<` in a comment, or a tag over several lines, is what it is.
pub(crate) fn context(editor: &mut xmlcraft_editor::Editor) -> Option<Context> {
    use xmlcraft_syntax::{Mode, is_name_char};
    let head = editor.primary().head();
    let buffer = editor.buffer();
    let before_caret = buffer.slice(buffer.line_start(buffer.line_of(head)).unwrap_or(0)..head).ok()?;
    let word_len: usize = before_caret.chars().rev().take_while(|c| is_name_char(*c)).map(char::len_utf8).sum();
    let word_start = head - word_len;
    let partial = buffer.slice(word_start..head).ok()?;
    let previous = if word_start > 0 { buffer.char_at(buffer.prev_char(word_start)) } else { None };
    match previous {
        // `<name`: an element, where the `<` is in character data.
        Some('<') => {
            let at = word_start - 1;
            (editor.markup_mode_at(at) == Mode::Content).then_some(Context { kind: Kind::Element, start: at, partial })
        }
        // `&name`: an entity, in character data or in an attribute value (where references are written too).
        Some('&') => {
            let at = word_start - 1;
            matches!(editor.markup_mode_at(at), Mode::Content | Mode::AttrValue(_)).then_some(Context { kind: Kind::Entity, start: at, partial })
        }
        _ => match editor.markup_mode_at(word_start) {
            Mode::AttrValue(quote) => value_context(editor, word_start, quote),
            Mode::Tag => {
                // An attribute name: some letters of it, after white space.
                let after_space = previous.is_some_and(char::is_whitespace);
                (!partial.is_empty() && after_space).then_some(Context { kind: Kind::Attribute, start: word_start, partial })
            }
            _ => None,
        },
    }
}

/// An open attribute value: everything typed after the quote is what a value replaces; the attribute is named before the `=`.
fn value_context(editor: &mut xmlcraft_editor::Editor, caret_word_start: usize, quote: xmlcraft_syntax::Quote) -> Option<Context> {
    use xmlcraft_syntax::{Quote, is_name_char};
    let q = if quote == Quote::Double { '"' } else { '\'' };
    let head = editor.primary().head();
    let buffer = editor.buffer();
    // The quote that opened the value: the nearest one of its kind before the caret (a value cannot hold its own quote).
    let from = caret_word_start.saturating_sub(LOOK_BACK);
    let from = (from..=caret_word_start).find(|o| buffer.is_boundary(*o)).unwrap_or(caret_word_start);
    let window = buffer.slice(from..head).ok()?;
    let quote_at = from + window.rfind(q)?;
    let value_start = quote_at + q.len_utf8();
    let before_quote = buffer.slice(from..quote_at).ok()?;
    let name_part = before_quote.trim_end().strip_suffix('=')?.trim_end();
    let name_len: usize = name_part.chars().rev().take_while(|c| is_name_char(*c)).map(char::len_utf8).sum();
    let (rest, name) = name_part.split_at(name_part.len() - name_len);
    // An attribute follows white space; a name straight after `<` is the element's.
    if name.is_empty() || !rest.ends_with(char::is_whitespace) {
        return None;
    }
    let name = name.to_owned();
    let partial = buffer.slice(value_start..head).ok()?;
    // A value cannot hold `<`: what comes after one is not a value any more.
    if partial.contains('<') {
        return None;
    }
    Some(Context { kind: Kind::Value(name), start: value_start, partial })
}

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
    fn completion_items(&mut self, kind: &Kind, choices: Option<&Choices>) -> Vec<Item> {
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
            (Kind::Element, None) => self.names_in_the_document(),
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
            (Kind::Value(name), Some(choices)) => choices
                .attributes
                .iter()
                .filter(|a| &a.name == name)
                .flat_map(|a| a.values.iter())
                .map(|v| Item { label: v.clone(), detail: String::new(), text: v.clone(), caret: v.len() })
                .collect(),
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
            _ => Vec::new(),
        }
    }

    /// Without a grammar: the element names this document uses, each written as a start tag and an end tag.
    fn names_in_the_document(&self) -> Vec<Item> {
        let Some(analysis) = self.active_analysis() else { return Vec::new() };
        let mut names: Vec<&str> = analysis.outline.iter().map(|i| i.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        names
            .into_iter()
            .take(500)
            .map(|n| Item { label: n.to_owned(), detail: tl!("used in this document").to_owned(), text: format!("<{n}></{n}>"), caret: n.len() + 2 })
            .collect()
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
        if self.update_data_completion() || self.update_html_completion() {
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
        let Some(ctx) = context(editor) else {
            self.completion = None;
            return;
        };
        // An element goes where the `<` is (what is typed is not a tag yet); the rest is about the place of the caret.
        let choices = if ctx.kind == Kind::Element { self.helper_choices_at(ctx.start) } else { self.helper_choices() };
        let mut items = narrowed(self.completion_items(&ctx.kind, choices.as_ref()), &ctx.partial);
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
    use xmlcraft_editor::Editor;

    /// The context at the `¦` of `marked` (the caret).
    fn at(marked: &str) -> Option<(Kind, String, usize)> {
        let caret = marked.find('¦').expect("a marker");
        let mut editor = Editor::new(&marked.replacen('¦', "", 1));
        editor.set_selection(caret, caret);
        context(&mut editor).map(|c| (c.kind, c.partial, c.start))
    }

    #[test]
    fn a_name_after_an_open_bracket_is_an_element_and_after_an_ampersand_an_entity() {
        assert_eq!(at("<r><¦</r>"), Some((Kind::Element, String::new(), 3)));
        assert_eq!(at("<r>  <bo¦</r>"), Some((Kind::Element, "bo".to_owned(), 5)), "the replacement starts at the bracket");
        assert_eq!(at("<r>a &¦</r>"), Some((Kind::Entity, String::new(), 5)));
        assert_eq!(at("<r>x &am¦</r>"), Some((Kind::Entity, "am".to_owned(), 5)));
        assert_eq!(at("<r><é́x¦</r>").map(|c| c.1), Some("é́x".to_owned()), "combining marks are name characters");
    }

    #[test]
    fn letters_after_white_space_in_a_start_tag_are_an_attribute_even_over_several_lines() {
        assert_eq!(at("<a i¦").map(|c| (c.0, c.1)), Some((Kind::Attribute, "i".to_owned())));
        assert_eq!(at("<a x=\"1\" ki¦").map(|c| c.1), Some("ki".to_owned()));
        assert_eq!(at("<a x=\"1>2\" ki¦").map(|c| c.0), Some(Kind::Attribute), "a > inside a value does not end the tag");
        assert_eq!(at("<a\n   x=\"1\"\n   ki¦").map(|c| c.0), Some(Kind::Attribute), "the tag goes on over lines");
        // Not at the white space itself, not in text, not after the tag has ended.
        assert_eq!(at("<a ¦"), None);
        assert_eq!(at("<a x=\"1\"> te¦"), None);
        assert_eq!(at("text te¦"), None);
    }

    #[test]
    fn markup_that_is_not_a_tag_offers_nothing() {
        for text in ["<!-- <¦", "<!-- &am¦", "<r><![CDATA[ <¦", "<r><![CDATA[ &am¦", "<?pi <¦", "<!-- <a i¦"] {
            assert_eq!(at(text), None, "{text}");
        }
        assert!(at("<r><!-- a --><¦").is_some(), "after the comment it is a tag again");
    }

    #[test]
    fn inside_the_quotes_of_an_attribute_its_values_are_offered_whole() {
        assert_eq!(at("<a kind=\"¦"), Some((Kind::Value("kind".to_owned()), String::new(), 9)));
        assert_eq!(at("<a kind=\"p¦").map(|c| c.1), Some("p".to_owned()));
        assert_eq!(at("<a kind='q¦").map(|c| (c.0, c.1)), Some((Kind::Value("kind".to_owned()), "q".to_owned())));
        assert_eq!(at("<a id=\"1\" kind = \"¦").map(|c| c.0), Some(Kind::Value("kind".to_owned())));
        // A value with spaces in it is replaced whole, from the quote.
        let (kind, partial, start) = at("<a state=\"not ap¦").expect("a value");
        assert_eq!((kind, partial.as_str(), start), (Kind::Value("state".to_owned()), "not ap", 10));
        // A closed value is not open; a quote of the other kind inside one is a character of it.
        assert_eq!(at("<a kind=\"p\" x¦").map(|c| c.0), Some(Kind::Attribute));
        assert_eq!(at("<a kind=\"it's¦").map(|c| c.1), Some("it's".to_owned()));
        // A value with no attribute name before it (or in an end tag's place) is nothing.
        assert_eq!(at("<a =\"¦"), None);
    }

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
