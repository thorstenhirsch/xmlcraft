//! The questions the app asks: unsaved changes, a file that changed on disk, a character the encoding cannot hold, and plain messages.

use std::path::PathBuf;

use crate::XmlCraftApp;
use crate::documents::AfterClose;
use crate::i18n::{fmt, tl};
use crate::theme::{self, Tokens};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Dialog {
    /// A document with unsaved changes is being closed (or the app is quitting).
    Unsaved {
        id: u64,
        then: AfterClose,
    },
    /// The file changed on disk since it was read, and saving would replace it.
    OverwriteChanged {
        id: u64,
        path: PathBuf,
        then: Option<AfterClose>,
    },
    /// The encoding cannot hold a character of the text.
    Unmappable {
        id: u64,
        path: PathBuf,
        then: Option<AfterClose>,
        character: char,
    },
    /// The file had bytes that were not valid, and saving over it would make that permanent.
    Lossy {
        id: u64,
        then: Option<AfterClose>,
    },
    /// The file changed on disk while the document has edits.
    Reload {
        id: u64,
    },
    /// Reverting would throw edits away.
    ConfirmRevert {
        id: u64,
    },
    /// Asks for a line (and column) to go to; the answer is typed into the app's `goto_input`.
    GoToLine,
    Message {
        title: String,
        text: String,
    },
}

impl Dialog {
    /// Whether this question was asked on the way to quitting, so that answering it with anything but a yes ends the quit. (The others
    /// are about something else: the quit goes on once they are answered.)
    fn belongs_to_quit(&self) -> bool {
        match self {
            Dialog::Unsaved { then, .. } => *then == AfterClose::Quit,
            Dialog::OverwriteChanged { then, .. } | Dialog::Unmappable { then, .. } | Dialog::Lossy { then, .. } => *then == Some(AfterClose::Quit),
            Dialog::Reload { .. } | Dialog::ConfirmRevert { .. } | Dialog::Message { .. } | Dialog::GoToLine => false,
        }
    }
}

/// What a button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choice {
    Save,
    Discard,
    Cancel,
    Overwrite,
    WriteReferences,
    SaveAsUtf8,
    SaveAs,
    Reload,
    Keep,
    Revert,
    Go,
    Ok,
}

struct Describe {
    title: String,
    text: String,
    /// The buttons, the first being the default.
    buttons: Vec<(&'static str, Choice)>,
    /// A line of text to type into.
    input: bool,
}

impl XmlCraftApp {
    fn document_name(&self, id: u64) -> String {
        self.session.position_of(id).and_then(|i| self.session.document(i)).map(|d| d.name().to_owned()).unwrap_or_default()
    }

    fn describe(&self, dialog: &Dialog) -> Describe {
        match dialog {
            Dialog::Unsaved { id, .. } => Describe {
                title: fmt(tl!("Save changes to {name}?"), &[("name", &self.document_name(*id))]),
                text: tl!("Your changes will be lost if you don't save them.").to_owned(),
                buttons: vec![(tl!("Save"), Choice::Save), (tl!("Don't Save"), Choice::Discard), (tl!("Cancel"), Choice::Cancel)],
                input: false,
            },
            Dialog::OverwriteChanged { id, .. } => Describe {
                title: fmt(tl!("{name} changed on disk"), &[("name", &self.document_name(*id))]),
                text: tl!("Another program changed the file after it was opened here. Saving now replaces its changes with yours.").to_owned(),
                buttons: vec![(tl!("Cancel"), Choice::Cancel), (tl!("Overwrite"), Choice::Overwrite)],
                input: false,
            },
            Dialog::Unmappable { id, character, .. } => Describe {
                title: fmt(tl!("{name} has a character its encoding cannot hold"), &[("name", &self.document_name(*id))]),
                text: fmt(
                    tl!("The character '{character}' (U+{code}) cannot be written in this file's encoding. Write it as a character reference (possible in text and attribute values), or save the file as UTF-8."),
                    &[("character", &character.to_string()), ("code", &format!("{:04X}", u32::from(*character)))],
                ),
                buttons: vec![
                    (tl!("Save as UTF-8"), Choice::SaveAsUtf8),
                    (tl!("Write character references"), Choice::WriteReferences),
                    (tl!("Cancel"), Choice::Cancel),
                ],
                input: false,
            },
            Dialog::Lossy { id, .. } => Describe {
                title: fmt(tl!("{name} cannot be saved over"), &[("name", &self.document_name(*id))]),
                text: tl!("The file has bytes that are not valid in its encoding. They are shown as �, and saving over the file would replace them for good. Save a copy under another name instead.").to_owned(),
                buttons: vec![(tl!("Save As…"), Choice::SaveAs), (tl!("Cancel"), Choice::Cancel)],
                input: false,
            },
            Dialog::Reload { id } => Describe {
                title: fmt(tl!("{name} changed on disk"), &[("name", &self.document_name(*id))]),
                text: tl!("Another program changed the file, and you have unsaved changes here. Reloading discards your changes.").to_owned(),
                buttons: vec![(tl!("Keep my version"), Choice::Keep), (tl!("Reload"), Choice::Reload), (tl!("Cancel"), Choice::Cancel)],
                input: false,
            },
            Dialog::ConfirmRevert { id } => Describe {
                title: fmt(tl!("Revert {name}?"), &[("name", &self.document_name(*id))]),
                text: tl!("This discards your unsaved changes and reads the file again.").to_owned(),
                buttons: vec![(tl!("Cancel"), Choice::Cancel), (tl!("Revert"), Choice::Revert)],
                input: false,
            },
            Dialog::GoToLine => Describe {
                title: tl!("Go to Line").to_owned(),
                text: tl!("Type a line number, or line:column.").to_owned(),
                buttons: vec![(tl!("Go"), Choice::Go), (tl!("Cancel"), Choice::Cancel)],
                input: true,
            },
            Dialog::Message { title, text } => {
                Describe { title: title.clone(), text: text.clone(), buttons: vec![(tl!("OK"), Choice::Ok)], input: false }
            }
        }
    }

    /// Draw the question being asked, if any, and act on the answer.
    pub(crate) fn show_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.dialog.clone() else { return };
        let t = Tokens::get(ctx);
        let described = self.describe(&dialog);
        let mut chosen = None;
        let response = egui::Modal::new(egui::Id::new("xmlcraft-dialog")).show(ctx, |ui| {
            ui.set_width(400.0);
            ui.label(egui::RichText::new(&described.title).font(theme::semibold(15.0)).color(t.text));
            ui.add_space(6.0);
            ui.label(egui::RichText::new(&described.text).font(theme::regular(13.0)).color(t.text_muted));
            if described.input {
                ui.add_space(8.0);
                let id = egui::Id::new("xmlcraft-dialog-input");
                let edit = ui
                    .add(egui::TextEdit::singleline(&mut self.goto_input).id(id).hint_text(tl!("Line or line:column")).desired_width(f32::INFINITY));
                crate::widgets::name_for_assistive_technology(ui, &edit, tl!("Line or line:column"));
                if !edit.has_focus() && !edit.lost_focus() {
                    edit.request_focus();
                }
                if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    chosen = Some(Choice::Go);
                }
            }
            ui.add_space(14.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Right to left: the last in the list is drawn first, at the right.
                for (index, (label, choice)) in described.buttons.iter().enumerate().rev() {
                    let primary = index == 0;
                    let button = if primary {
                        egui::Button::new(egui::RichText::new(*label).color(egui::Color32::WHITE)).fill(t.accent)
                    } else {
                        egui::Button::new(*label)
                    };
                    if ui.add(button).clicked() {
                        chosen = Some(*choice);
                    }
                }
            });
        });
        // Escape or a click outside is the safe answer: never one that decides something (not "Keep my version", which would let the next
        // save replace what another program wrote without asking again).
        if chosen.is_none() && response.should_close() {
            chosen = described.buttons.iter().map(|(_, c)| *c).find(|c| matches!(c, Choice::Cancel | Choice::Ok));
        }
        if let Some(choice) = chosen {
            self.dialog = None;
            self.answer(ctx, dialog, choice);
        }
    }

    fn answer(&mut self, ctx: &egui::Context, dialog: Dialog, choice: Choice) {
        match (dialog, choice) {
            (Dialog::Unsaved { id, then }, Choice::Save) => self.save_document(ctx, id, Some(then)),
            (Dialog::Unsaved { id, then }, Choice::Discard) => {
                self.close_document(id);
                if then == AfterClose::Quit {
                    self.continue_quit(ctx);
                }
            }
            (Dialog::OverwriteChanged { id, path, then }, Choice::Overwrite) => self.write_document(ctx, id, path, true, then),
            (Dialog::Unmappable { id, path, then, .. }, Choice::WriteReferences) => self.write_with_references(ctx, id, path, then),
            (Dialog::Unmappable { id, path, then, .. }, Choice::SaveAsUtf8) => self.change_encoding_and_write(ctx, id, path, then),
            (Dialog::Lossy { id, then }, Choice::SaveAs) => {
                self.pick_save_as(ctx, id, then);
            }
            (Dialog::Reload { id }, Choice::Reload) | (Dialog::ConfirmRevert { id }, Choice::Revert) => self.reload_document(id),
            (Dialog::Reload { id }, Choice::Keep) => self.keep_version(id),
            (Dialog::GoToLine, Choice::Go) => {
                let input = std::mem::take(&mut self.goto_input);
                if !self.go_to_line(&input) {
                    self.status = fmt(tl!("'{text}' is not a line number"), &[("text", input.trim())]);
                }
            }
            // Anything else is a way out. A quit that was waiting for this answer is off; one that was only waiting for another question
            // to be out of the way goes on.
            (dialog, _) => {
                if dialog.belongs_to_quit() {
                    self.quitting = false;
                } else {
                    self.continue_quit(ctx);
                }
            }
        }
    }
}
