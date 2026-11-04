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
    /// The byte offset (in the line's prefix) where what the choice replaces starts: the `<` or `&` of an element or an entity, the first
    /// letter of an attribute name or of a value.
    pub start: usize,
    /// What is typed of the name so far.
    pub partial: String,
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':')
}

/// What to complete at the end of `prefix` (the text of the caret's line up to the caret), after `typed` was typed. Nothing for a place
/// completion has nothing to say about (text, a comment, white space between attributes).
pub(crate) fn context(prefix: &str, typed: char) -> Option<Context> {
    let word_len: usize = prefix.chars().rev().take_while(|c| is_name_char(*c)).map(char::len_utf8).sum();
    let word_start = prefix.len() - word_len;
    let (before, partial) = (prefix.get(..word_start)?, prefix.get(word_start..)?);
    if before.ends_with('<') {
        return Some(Context { kind: Kind::Element, start: word_start - 1, partial: partial.to_owned() });
    }
    if before.ends_with('&') {
        return Some(Context { kind: Kind::Entity, start: word_start - 1, partial: partial.to_owned() });
    }
    // Inside a start tag (a `<` and a name after it, and no `>` since)?
    let open = prefix.rfind('<')?;
    let tag = prefix.get(open + 1..)?;
    if tag.contains('>') || !tag.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_' || c == ':') {
        return None;
    }
    // Which quote is open, if one is: the state of the tag so far. A quote of the other kind inside a value is a character of it.
    let mut open_quote: Option<(char, usize)> = None;
    for (i, c) in tag.char_indices() {
        match open_quote {
            None if c == '"' || c == '\'' => open_quote = Some((c, i)),
            Some((q, _)) if c == q => open_quote = None,
            _ => {}
        }
    }
    if let Some((_, at)) = open_quote {
        // The name of the attribute whose value is open: before `=` (and white space) before the quote.
        let name_part = tag.get(..at)?.trim_end().strip_suffix('=')?.trim_end();
        let name: String = name_part.chars().rev().take_while(|c| is_name_char(*c)).collect::<Vec<_>>().into_iter().rev().collect();
        return (!name.is_empty()).then_some(Context { kind: Kind::Value(name), start: word_start, partial: partial.to_owned() });
    }
    // An attribute name: some letters of it have been typed, after white space.
    if !partial.is_empty() && before.ends_with(char::is_whitespace) && is_name_char(typed) {
        return Some(Context { kind: Kind::Attribute, start: word_start, partial: partial.to_owned() });
    }
    None
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
                .filter(|a| !a.used && a.declare.is_none())
                .map(|a| {
                    let snippet = attribute_snippet(&a.name, a.values.first().map(String::as_str));
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
            (Kind::Entity, Some(choices)) => choices
                .entities
                .iter()
                .map(|e| {
                    let snippet = entity_snippet(&e.name);
                    Item { label: format!("&{};", e.name), detail: e.value.clone().unwrap_or_default(), text: snippet.text, caret: snippet.caret }
                })
                .collect(),
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

    /// Work out what to offer at the caret after the frame's typing, and open or close the popup.
    pub(crate) fn update_completion(&mut self) {
        let Some(doc) = self.session.active_document() else {
            self.completion = None;
            return;
        };
        let editor = doc.editor();
        let (id, version) = (doc.id(), editor.version());
        let caret = editor.primary();
        if editor.carets().len() != 1 || !caret.is_empty() {
            self.completion = None;
            return;
        }
        let head = caret.head();
        let buffer = editor.buffer();
        let line_start = buffer.line_start(buffer.line_of(head)).unwrap_or(0);
        let Ok(prefix) = buffer.slice(line_start..head) else {
            self.completion = None;
            return;
        };
        let Some(typed) = prefix.chars().next_back() else {
            self.completion = None;
            return;
        };
        let Some(ctx) = context(&prefix, typed) else {
            self.completion = None;
            return;
        };
        // An element goes where the `<` is; the rest is about the place of the caret.
        let choices = if ctx.kind == Kind::Element { self.helper_choices_at(line_start + ctx.start) } else { self.helper_choices() };
        let items = narrowed(self.completion_items(&ctx.kind, choices.as_ref()), &ctx.partial);
        if items.is_empty() {
            self.completion = None;
            return;
        }
        let mut items = items;
        items.truncate(200);
        self.completion = Some(Completion { doc: id, version, start: line_start + ctx.start, caret: head, items, selected: 0 });
    }

    /// Whether the popup that is open is about the text as it is in the document being shown.
    fn completion_is_current(&self) -> bool {
        let Some(c) = &self.completion else { return false };
        self.session.active_document().is_some_and(|d| d.id() == c.doc && d.editor().version() == c.version && d.editor().primary().head() == c.caret)
    }

    /// Before the editor is drawn: the keys of the popup. Arrow keys choose, Enter and Tab write the choice, Escape closes it; they are
    /// taken from the frame's input so that the editor does not also act on them.
    pub(crate) fn completion_keys(&mut self, ui: &mut egui::Ui) {
        if self.completion.is_some() && !self.completion_is_current() {
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
        if c.caret > editor.buffer().len() || c.start > c.caret {
            return;
        }
        if editor.apply_edits(&[Edit::replace(c.start..c.caret, item.text.clone())]) {
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

    fn kind_of(prefix: &str, typed: char) -> Option<(Kind, String)> {
        context(prefix, typed).map(|c| (c.kind, c.partial))
    }

    #[test]
    fn a_name_after_an_open_bracket_is_an_element_and_after_an_ampersand_an_entity() {
        assert_eq!(kind_of("<", '<'), Some((Kind::Element, String::new())));
        assert_eq!(kind_of("  <bo", 'o'), Some((Kind::Element, "bo".to_owned())));
        assert_eq!(context("  <bo", 'o').map(|c| c.start), Some(2), "the replacement starts at the bracket");
        assert_eq!(kind_of("a &", '&'), Some((Kind::Entity, String::new())));
        assert_eq!(kind_of("a &am", 'm'), Some((Kind::Entity, "am".to_owned())));
        assert_eq!(context("x &am", 'm').map(|c| c.start), Some(2));
    }

    #[test]
    fn letters_after_white_space_in_a_start_tag_are_an_attribute() {
        assert_eq!(kind_of("<a i", 'i'), Some((Kind::Attribute, "i".to_owned())));
        assert_eq!(kind_of("<a x=\"1\" ki", 'i'), Some((Kind::Attribute, "ki".to_owned())));
        // Not at the white space itself, not in text, not after the tag has ended.
        assert_eq!(kind_of("<a ", ' '), None);
        assert_eq!(kind_of("<a x=\"1\"> te", 'e'), None);
        assert_eq!(kind_of("text te", 'e'), None);
        assert_eq!(kind_of("<!-- a b", 'b'), None);
        assert_eq!(kind_of("</a i", 'i'), None, "an end tag has no attributes");
    }

    #[test]
    fn inside_the_quotes_of_an_attribute_its_values_are_offered() {
        assert_eq!(kind_of("<a kind=\"", '"'), Some((Kind::Value("kind".to_owned()), String::new())));
        assert_eq!(kind_of("<a kind=\"p", 'p'), Some((Kind::Value("kind".to_owned()), "p".to_owned())));
        assert_eq!(kind_of("<a kind='q", 'q'), Some((Kind::Value("kind".to_owned()), "q".to_owned())));
        assert_eq!(kind_of("<a id=\"1\" kind = \"", '"'), Some((Kind::Value("kind".to_owned()), String::new())));
        // A closed value is not open any more; a quote of the other kind inside one does not open another.
        assert_eq!(kind_of("<a kind=\"p\" x", 'x'), Some((Kind::Attribute, "x".to_owned())));
        assert_eq!(kind_of("<a kind=\"it's", 's'), Some((Kind::Value("kind".to_owned()), "s".to_owned())));
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
