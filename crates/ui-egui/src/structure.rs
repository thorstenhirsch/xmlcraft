//! Working with the structure of the document: the element the caret is in can be renamed, wrapped, unwrapped, selected and copied as a
//! path. What is found comes from the analysis of the text as it is; when that is not there yet, the commands say to wait.

use xmlcraft_engine::analysis::OutlineItem;
use xmlcraft_engine::elements::{self, RenameError};

use crate::XmlCraftApp;
use crate::dialog::Dialog;
use crate::i18n::{fmt, tl};
use crate::kit::{self, PillKind, Tone};
use crate::widgets;

/// What a new element is called when it is made by wrapping: a name to type over.
const WRAP_NAME: &str = "element";

/// The new name of an element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RenameForm {
    /// Where the name of the element's start tag begins: which element it is (the text cannot change while the dialog is up).
    target: usize,
    old: String,
    new: String,
    error: Option<String>,
    /// The keyboard goes to the field once, when the dialog opens.
    focus: bool,
}

impl XmlCraftApp {
    /// The element the caret is in, from the analysis of the text as it is. The index, the item and the text.
    fn element_at_caret(&mut self) -> Option<(usize, OutlineItem, String)> {
        let doc = self.session.active_document()?;
        let (caret, version, text) = (doc.editor().primary().start(), doc.editor().version(), doc.editor().text());
        let analysis = self.active_analysis().filter(|a| a.version == version && a.skipped.is_none());
        let Some(analysis) = analysis else {
            self.status = tl!("The document is still being read. Try again in a moment.").to_owned();
            return None;
        };
        let found = elements::innermost(&analysis.outline, caret).and_then(|i| analysis.outline.get(i).cloned().map(|item| (i, item)));
        match found {
            Some((i, item)) => Some((i, item, text)),
            None => {
                self.status = tl!("The caret is not in an element.").to_owned();
                None
            }
        }
    }

    /// Ask for the new name of the element the caret is in (Rename Element…).
    pub(crate) fn open_rename_element(&mut self) {
        let Some((_, item, _)) = self.element_at_caret() else { return };
        self.dialog = Some(Dialog::RenameElement(Box::new(RenameForm {
            target: item.name_range.start,
            old: item.name.clone(),
            new: item.name,
            error: None,
            focus: true,
        })));
    }

    pub(crate) fn show_rename_element(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let mut answer = None;
        let title = tl!("Rename Element").to_owned();
        let old = match &dialog {
            Dialog::RenameElement(form) => form.old.clone(),
            _ => String::new(),
        };
        let text = fmt(tl!("Both its tags are renamed, as one step you can undo."), &[]);
        let (_, escaped) = kit::question(ctx, "xmlcraft-rename-element", 440.0, "text-cursor-input", Tone::Accent, &title, &text, |ui| {
            let Dialog::RenameElement(form) = &mut dialog else { return };
            ui.add_space(kit::GAP_GROUP);
            let field = ui.add(widgets::line(&mut form.new).hint_text(old.as_str()).desired_width(f32::INFINITY));
            widgets::name_for_assistive_technology(ui, &field, tl!("The new name of the element"));
            if std::mem::take(&mut form.focus) {
                field.request_focus();
                // The old name is selected, so that typing replaces it.
                if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), field.id) {
                    state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(form.new.chars().count()),
                    )));
                    state.store(ui.ctx(), field.id);
                }
            }
            if field.changed() {
                form.error = None;
            }
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                answer = Some(true);
            }
            if let Some(message) = &form.error {
                ui.add_space(kit::GAP);
                kit::banner(ui, Tone::Bad, message);
            }
            match kit::actions(ui, &[(tl!("Rename"), PillKind::Primary), (tl!("Cancel"), PillKind::Secondary)]) {
                Some(0) => answer = Some(true),
                Some(_) => answer = Some(false),
                None => {}
            }
        });
        match answer {
            Some(false) => {}
            _ if answer.is_none() && escaped => {}
            Some(true) => {
                if let Dialog::RenameElement(form) = &mut dialog {
                    match self.rename_now(form) {
                        Ok(message) => {
                            self.status = message;
                            return;
                        }
                        Err(why) => form.error = Some(why),
                    }
                }
                self.dialog = Some(dialog);
            }
            _ => self.dialog = Some(dialog),
        }
    }

    /// Rename the element `form` is about; the message to show, or why not.
    fn rename_now(&mut self, form: &RenameForm) -> Result<String, String> {
        let new = form.new.trim().to_owned();
        let (item, text) = {
            let doc = self.session.active_document().ok_or_else(|| tl!("The document is gone.").to_owned())?;
            let text = doc.editor().text();
            let version = doc.editor().version();
            let analysis = self
                .active_analysis()
                .filter(|a| a.version == version)
                .ok_or_else(|| tl!("The document is still being read. Try again in a moment.").to_owned())?;
            let item =
                analysis.outline.iter().find(|i| i.name_range.start == form.target).cloned().ok_or_else(|| tl!("The element is gone.").to_owned())?;
            (item, text)
        };
        let edits = elements::rename_edits(&text, &item, &new).map_err(|e| match e {
            RenameError::NotAName => {
                tl!("That is not an element name. Start with a letter, an underscore or a colon; go on with letters, digits, . - _ and :.").to_owned()
            }
            RenameError::Unchanged => tl!("That is the name it has.").to_owned(),
        })?;
        let doc = self.session.active_document_mut().ok_or_else(|| tl!("The document is gone.").to_owned())?;
        if !doc.editor_mut().apply_edits(&edits) {
            return Err(tl!("The text could not be changed.").to_owned());
        }
        self.dialog = None;
        Ok(fmt(tl!("Renamed {old} to {new}"), &[("old", &form.old), ("new", &new)]))
    }

    /// Wrap what is selected in a new element, with its name selected and both tags linked: typing the name renames both.
    pub(crate) fn wrap_in_tag(&mut self) {
        let Some(doc) = self.session.active_document_mut() else { return };
        if doc.editor_mut().wrap_selection_in_tag(WRAP_NAME) {
            self.focus_editor = true;
        } else {
            self.status = tl!("Wrapping needs one caret or one selection.").to_owned();
        }
    }

    /// Take the tags of the element the caret is in away, keeping what is inside it.
    pub(crate) fn unwrap_element(&mut self) {
        let Some((_, item, _)) = self.element_at_caret() else { return };
        let edits = elements::unwrap_edits(&item);
        let Some(doc) = self.session.active_document_mut() else { return };
        if doc.editor_mut().apply_edits(&edits) {
            self.status = fmt(tl!("Removed the tags of {name}"), &[("name", &item.name)]);
            self.focus_editor = true;
        }
    }

    /// Select the element the caret is in; when it is selected already, the one around it.
    pub(crate) fn select_element(&mut self) {
        let Some((index, item, _)) = self.element_at_caret() else { return };
        let Some(doc) = self.session.active_document() else { return };
        let current = doc.editor().primary();
        let range = elements::element_range(&item);
        let target = if (current.start(), current.end()) == (range.start, range.end) {
            // Already selected: the element around it, when there is one.
            let outline = self.active_analysis().map(|a| a.outline.clone()).unwrap_or_default();
            elements::parent(&outline, index).and_then(|p| outline.get(p)).map_or(range, elements::element_range)
        } else {
            range
        };
        self.jump_to(target);
    }

    /// Copy the path of the element the caret is in.
    pub(crate) fn copy_xpath(&mut self) {
        let Some((index, _, _)) = self.element_at_caret() else { return };
        let path = self.active_analysis().and_then(|a| elements::xpath(&a.outline, index));
        let (Some(path), Some(ctx)) = (path, self.ctx.clone()) else { return };
        ctx.copy_text(path.clone());
        self.status = fmt(tl!("Copied {path}"), &[("path", &path)]);
    }
}
