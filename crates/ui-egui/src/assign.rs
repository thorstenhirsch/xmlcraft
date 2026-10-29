//! The Assign Schema dialog: says which DTD or XML Schema the document being shown is to be checked against, written as a minimal edit
//! (a DOCTYPE, or `xsi:` attributes on the root element) that can be undone.

use std::path::{Path, PathBuf};

use xmlcraft_engine::assign::{Assignment, assign, is_dtd_location, relative_location, schema_namespace};

use crate::XmlCraftApp;
use crate::dialog::Dialog;
use crate::i18n::{fmt, tl};
use crate::theme::{self, Tokens};
use crate::widgets;

/// What the location is: told by the end of its name, or said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Detect,
    Schema,
    Dtd,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AssignForm {
    source: u64,
    name: String,
    /// Where the document is, to write locations from its folder.
    document: Option<PathBuf>,
    location: String,
    kind: Kind,
    error: Option<String>,
    /// The keyboard goes to the field once, when the dialog opens.
    focus: bool,
}

enum Action {
    Assign,
    Browse,
    Cancel,
}

impl XmlCraftApp {
    /// Ask which schema or DTD to assign to the document being shown (Assign Schema…).
    pub(crate) fn open_assign_schema(&mut self) {
        let Some(doc) = self.session.active_document() else { return };
        self.dialog = Some(Dialog::AssignSchema(Box::new(AssignForm {
            source: doc.id(),
            name: doc.name().to_owned(),
            document: doc.path().map(Path::to_path_buf),
            location: String::new(),
            kind: Kind::Detect,
            error: None,
            focus: true,
        })));
    }

    /// The file the picker returned, as the location of the form (from the folder of the document when it is a file).
    pub(crate) fn picked_schema_file(&mut self, path: &Path) {
        if let Some(Dialog::AssignSchema(form)) = &mut self.dialog {
            form.location = match &form.document {
                // The document is known by its real path, so the file is too (`/var` and `/private/var` are one place).
                Some(document) => relative_location(document, &in_real_folder(path)),
                None => path.to_string_lossy().replace('\\', "/"),
            };
            form.error = None;
        }
    }

    /// Draw the form and act on what was done in it.
    pub(crate) fn show_assign(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let t = Tokens::get(ctx);
        let mut action = None;
        let response = egui::Modal::new(egui::Id::unique("xmlcraft-assign")).show(ctx, |ui| {
            ui.set_width(460.0);
            if let Dialog::AssignSchema(form) = &mut dialog {
                action = form_ui(ui, &t, form);
            }
        });
        if action.is_none() && response.should_close() {
            action = Some(Action::Cancel);
        }
        match action {
            Some(Action::Cancel) => {}
            Some(Action::Browse) => {
                self.pick_schema_file(ctx);
                self.dialog = Some(dialog);
            }
            Some(Action::Assign) => {
                if let Dialog::AssignSchema(form) = &mut dialog {
                    match self.assign_now(form) {
                        Ok(message) => {
                            self.status = message;
                            return;
                        }
                        Err(why) => form.error = Some(why),
                    }
                }
                self.dialog = Some(dialog);
            }
            None => self.dialog = Some(dialog),
        }
    }

    /// Write the assignment into the document; the message to show, or why not.
    fn assign_now(&mut self, form: &AssignForm) -> Result<String, String> {
        let location = form.location.trim();
        if location.is_empty() {
            return Err(tl!("Type where the schema or the DTD is, or choose a file.").to_owned());
        }
        let Some(doc) = self.session.position_of(form.source).and_then(|i| self.session.document_mut(i)) else {
            return Err(tl!("The document is gone.").to_owned());
        };
        let is_dtd = match form.kind {
            Kind::Dtd => true,
            Kind::Schema => false,
            Kind::Detect => is_dtd_location(location),
        };
        // The namespace a schema is for is read from the schema document when it is a file that can be read.
        let namespace = if is_dtd {
            None
        } else {
            let wanted = match &form.document {
                Some(document) => xmlcraft_resolve::join(&document.to_string_lossy(), location),
                None => location.to_owned(),
            };
            xmlcraft_platform::files::read_file(Path::new(&wanted)).ok().and_then(|loaded| {
                schema_namespace(&xmlcraft_text::encoding::decode(&loaded.bytes, &xmlcraft_text::encoding::sniff(&loaded.bytes)).text)
            })
        };
        let what = if is_dtd { Assignment::Dtd { location } } else { Assignment::Schema { namespace: namespace.as_deref(), location } };
        let edits = assign(&doc.editor().text(), &what).map_err(|e| e.to_string())?;
        // Nothing to write when it is assigned already: the document stays as it is, with no step to undo.
        if edits.is_empty() {
            self.dialog = None;
            return Ok(fmt(tl!("{name} uses {location} already"), &[("name", &form.name), ("location", location)]));
        }
        if !doc.editor_mut().apply_edits(&edits) {
            return Err(tl!("The text could not be changed.").to_owned());
        }
        self.dialog = None;
        Ok(fmt(tl!("Assigned {location} to {name}"), &[("location", location), ("name", &form.name)]))
    }
}

/// `path` with its folder spelled as the system spells it (links followed), and the name of the file as it is, so that a link to a
/// schema stays a link. As it is when the folder cannot be found.
fn in_real_folder(path: &Path) -> PathBuf {
    match (path.parent().and_then(|dir| std::fs::canonicalize(dir).ok()), path.file_name()) {
        (Some(dir), Some(name)) => dir.join(name),
        _ => path.to_path_buf(),
    }
}

fn form_ui(ui: &mut egui::Ui, t: &Tokens, form: &mut AssignForm) -> Option<Action> {
    let mut action = None;
    ui.label(egui::RichText::new(tl!("Assign Schema")).font(theme::semibold(15.0)).color(t.text));
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(fmt(tl!("Say which XML Schema or DTD {name} is checked against."), &[("name", &form.name)]))
            .font(theme::regular(13.0))
            .color(t.text_muted),
    );
    ui.add_space(10.0);
    ui.label(egui::RichText::new(tl!("Location")).font(theme::medium(12.0)).color(t.text_muted));
    ui.horizontal(|ui| {
        let field = ui.add(egui::TextEdit::singleline(&mut form.location).hint_text(tl!("schema.xsd, a DTD or a URL")).desired_width(330.0));
        widgets::name_for_assistive_technology(ui, &field, tl!("The location of the schema or the DTD"));
        if std::mem::take(&mut form.focus) {
            field.request_focus();
        }
        if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            action = Some(Action::Assign);
        }
        if field.changed() {
            form.error = None;
        }
        if ui.button(tl!("Browse…")).clicked() {
            action = Some(Action::Browse);
        }
    });
    ui.add_space(6.0);
    ui.label(egui::RichText::new(tl!("It is")).font(theme::medium(12.0)).color(t.text_muted));
    ui.horizontal(|ui| {
        for (kind, label) in [(Kind::Detect, tl!("told by its name")), (Kind::Schema, tl!("an XML Schema")), (Kind::Dtd, tl!("a DTD"))] {
            if ui.add(widgets::radio(form.kind == kind, label)).clicked() {
                form.kind = kind;
            }
        }
    });
    if let Some(message) = &form.error {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(message).font(theme::regular(13.0)).color(crate::editor_view::SyntaxTokens::for_kind(t.kind).error));
    }
    ui.add_space(14.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let button = egui::Button::new(egui::RichText::new(tl!("Assign")).color(egui::Color32::WHITE)).fill(t.accent);
        if ui.add(button).clicked() {
            action = Some(Action::Assign);
        }
        if ui.button(tl!("Cancel")).clicked() {
            action = Some(Action::Cancel);
        }
    });
    action
}
