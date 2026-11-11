//! The Assign Schema dialog: says which DTD or XML Schema the document being shown is to be checked against, written as a minimal edit
//! (a DOCTYPE, or `xsi:` attributes on the root element) that can be undone.

use std::path::{Path, PathBuf};

use xmlcraft_engine::assign::{Assignment, assign, is_dtd_location, relative_location, schema_namespace};
use xmlcraft_engine::bundled::{self, BundledFormat};

use crate::XmlCraftApp;
use crate::dialog::Dialog;
use crate::i18n::{fmt, tl};
use crate::kit::{self, Footer, Tone};
use crate::widgets;

/// A schema SchemaStore has for the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StoreMatch {
    pub name: String,
    pub description: String,
    pub url: String,
}

/// What is known of the asking of SchemaStore.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Store {
    /// The request that is out.
    pub asking: Option<u64>,
    pub found: Vec<StoreMatch>,
    /// What came of the last asking, in words (nothing found, or why not).
    pub note: Option<String>,
}

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
    /// The document is JSON or YAML: a JSON Schema is assigned (a `$schema` member, a modeline), not a DTD or an XML Schema.
    data: bool,
    /// Whether the bundled list is open, and what filters it.
    browse: bool,
    filter: String,
    /// What SchemaStore said (JSON and YAML only).
    pub(crate) store: Store,
}

enum Action {
    Assign,
    Browse,
    Cancel,
    /// Ask SchemaStore which schemas are for this file.
    Find,
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
            data: doc.language().is_data(),
            browse: false,
            filter: String::new(),
            store: Store::default(),
        })));
    }

    /// Ask SchemaStore (its catalog, fetched through the app's network as any schema is, so the offline switch and the download cache
    /// apply) which schemas are for the file of the document; the answer comes through `pump_http`.
    fn ask_schemastore(&mut self, form: &mut AssignForm) {
        let file = form.document.as_ref().map_or_else(|| form.name.clone(), |p| p.to_string_lossy().into_owned());
        let serial = self.next_http_serial();
        let network = self.network.clone();
        let started = self.spawn_http(move || {
            let resolver = network.resolver(std::sync::Arc::new(xmlcraft_engine::validation::OpenDocuments::new(Vec::new())));
            let result = xmlcraft_engine::guard(|| {
                let found = resolver
                    .resolve(&xmlcraft_resolve::Request::uri(xmlcraft_engine::schemastore::CATALOG_URL, ""), &mut xmlcraft_resolve::Budget::default())
                    .map_err(|f| f.message)?;
                let decoded = xmlcraft_text::encoding::decode(&found.bytes, &xmlcraft_text::encoding::sniff(&found.bytes));
                let entries = xmlcraft_engine::schemastore::parse_catalog(&decoded.text)?;
                Ok::<Vec<StoreMatch>, String>(
                    xmlcraft_engine::schemastore::matching(&entries, &file)
                        .into_iter()
                        .take(20)
                        .map(|e| StoreMatch { name: e.name.clone(), description: e.description.clone(), url: e.url.clone() })
                        .collect(),
                )
            })
            .unwrap_or_else(|_| Err("asking SchemaStore failed (a bug); nothing was changed".to_owned()));
            crate::http_pane::Outcome::Catalog { serial, result }
        });
        match started {
            Ok(()) => {
                form.store.asking = Some(serial);
                form.store.found.clear();
                form.store.note = None;
            }
            Err(why) => form.store.note = Some(why),
        }
    }

    /// The answer of SchemaStore, if the window is still waiting for it.
    pub(crate) fn schemastore_answered(&mut self, serial: u64, result: Result<Vec<StoreMatch>, String>) {
        let Some(Dialog::AssignSchema(form)) = &mut self.dialog else { return };
        if form.store.asking != Some(serial) {
            return;
        }
        form.store.asking = None;
        match result {
            Ok(found) if found.is_empty() => form.store.note = Some(tl!("SchemaStore has no schema for this file name.").to_owned()),
            Ok(found) => form.store.found = found,
            Err(why) => form.store.note = Some(why),
        }
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
        let name = match &dialog {
            Dialog::AssignSchema(form) => form.name.clone(),
            _ => String::new(),
        };
        let data = matches!(&dialog, Dialog::AssignSchema(form) if form.data);
        let subtitle = if data {
            fmt(tl!("Say which JSON Schema {name} is checked against."), &[("name", &name)])
        } else {
            fmt(tl!("Say which XML Schema or DTD {name} is checked against."), &[("name", &name)])
        };
        let (mut action, closed) = kit::dialog(ctx, "xmlcraft-assign", 520.0, "book-open", tl!("Assign Schema"), &subtitle, |ui| match &mut dialog {
            Dialog::AssignSchema(form) => form_ui(ui, form),
            _ => None,
        });
        if action.is_none() && closed {
            action = Some(Action::Cancel);
        }
        match action {
            Some(Action::Cancel) => {}
            Some(Action::Browse) => {
                self.pick_schema_file(ctx);
                self.dialog = Some(dialog);
            }
            Some(Action::Find) => {
                if let Dialog::AssignSchema(form) = &mut dialog {
                    self.ask_schemastore(form);
                }
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
        if form.data {
            let language = doc.language();
            let edits = xmlcraft_engine::assign::assign_data(&doc.editor().text(), language, location).map_err(|e| e.to_string())?;
            if edits.is_empty() {
                self.dialog = None;
                return Ok(fmt(tl!("{name} uses {location} already"), &[("name", &form.name), ("location", location)]));
            }
            if !doc.editor_mut().apply_edits(&edits) {
                return Err(tl!("The text could not be changed.").to_owned());
            }
            self.dialog = None;
            return Ok(fmt(tl!("Assigned {location} to {name}"), &[("location", location), ("name", &form.name)]));
        }
        // A bundled file is named by its publisher's address (the bundled catalog maps it back), never by a bundled:/ URI.
        let written = bundled::location_to_write(location)?;
        let location = written.as_str();
        let is_dtd = match form.kind {
            Kind::Dtd => true,
            Kind::Schema => false,
            Kind::Detect => is_dtd_location(location),
        };
        // The namespace a schema is for is read from the schema document: a bundled one, or a file that can be read.
        let namespace = if is_dtd {
            None
        } else if let Some(namespace) = bundled::namespace_of(location) {
            Some(namespace)
        } else {
            let wanted = match &form.document {
                Some(document) => xmlcraft_resolve::join(&document.to_string_lossy(), location),
                None => location.to_owned(),
            };
            xmlcraft_platform::files::read_file(Path::new(&wanted)).ok().and_then(|loaded| {
                schema_namespace(&xmlcraft_text::encoding::decode(&loaded.bytes, &xmlcraft_text::encoding::sniff(&loaded.bytes)).text)
            })
        };
        let public = if is_dtd { bundled::public_id_of(location) } else { None };
        let what = if is_dtd {
            Assignment::Dtd { public: public.as_deref(), location }
        } else {
            Assignment::Schema { namespace: namespace.as_deref(), location }
        };
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

fn form_ui(ui: &mut egui::Ui, form: &mut AssignForm) -> Option<Action> {
    let mut action = None;
    kit::section(ui, tl!("Location"), |ui| {
        // Browse at the right edge, the field in what is left.
        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), kit::PILL_HEIGHT), egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = kit::GAP;
            if kit::pill(ui, kit::PillKind::Secondary, Some("folder-open"), tl!("Browse…")).clicked() {
                action = Some(Action::Browse);
            }
            let field = ui.add(
                widgets::line(&mut form.location)
                    .hint_text(if form.data { tl!("schema.json or a URL") } else { tl!("schema.xsd, a DTD or a URL") })
                    .desired_width(ui.available_width()),
            );
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
        });
        kit::help(ui, tl!("A file name is written from the folder of the document. An address starting with http is downloaded when it is needed."));
    });
    if !form.data {
        kit::section(ui, tl!("Bundled schemas"), |ui| {
            if kit::pill(ui, kit::PillKind::Secondary, Some("book-open"), tl!("Browse bundled schemas…")).clicked() {
                form.browse = !form.browse;
            }
            if !form.browse {
                kit::help(ui, tl!("The XML Schemas and DTDs that ship with XmlCraft, for choosing one without a file."));
                return;
            }
            let field = ui.add(widgets::line(&mut form.filter).hint_text(tl!("Filter by name, set or path")).desired_width(f32::INFINITY));
            widgets::name_for_assistive_technology(ui, &field, tl!("Filter the bundled schemas"));
            let found = bundled::matching(&form.filter);
            if found.is_empty() {
                kit::help(ui, tl!("No bundled schema or DTD matches."));
                return;
            }
            let mut chosen = None;
            egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
                for schema in &found {
                    let label = format!("{} ({})", schema.name, schema.set);
                    // A file known only by its namespace has no address to write: it is listed, but cannot be chosen.
                    let Some(location) = &schema.location else {
                        ui.add_enabled(false, egui::Button::selectable(false, label))
                            .on_disabled_hover_text(tl!("Found by its namespace without being named: nothing to assign"));
                        continue;
                    };
                    if ui.selectable_label(form.location == *location, label).on_hover_text(location).clicked() {
                        chosen = Some(schema);
                    }
                }
            });
            if let Some(schema) = chosen {
                form.location = schema.location.clone().unwrap_or_default();
                form.kind = match schema.format {
                    BundledFormat::Xsd => Kind::Schema,
                    BundledFormat::Dtd => Kind::Dtd,
                };
                form.error = None;
                form.browse = false;
            }
        });
    }
    if form.data {
        kit::section(ui, tl!("SchemaStore"), |ui| {
            ui.horizontal(|ui| {
                let asking = form.store.asking.is_some();
                if ui.add_enabled(!asking, egui::Button::new(tl!("Find a schema for this file"))).clicked() {
                    action = Some(Action::Find);
                }
                if asking {
                    ui.spinner();
                }
            });
            for found in &form.store.found {
                let label = if found.description.is_empty() { found.name.clone() } else { format!("{} — {}", found.name, found.description) };
                if ui.selectable_label(form.location == found.url, label).on_hover_text(&found.url).clicked() {
                    form.location = found.url.clone();
                    form.error = None;
                }
            }
            if let Some(note) = &form.store.note {
                kit::help(ui, note);
            } else if form.store.found.is_empty() && form.store.asking.is_none() {
                kit::help(
                    ui,
                    tl!(
                        "Asks schemastore.org for the schemas that are for this file name (it downloads its catalog, which the Work Offline switch stops)."
                    ),
                );
            }
        });
    }
    if !form.data {
        kit::section(ui, tl!("It is"), |ui| {
            kit::segmented(
                ui,
                &mut form.kind,
                &[(Kind::Detect, tl!("Told by its name")), (Kind::Schema, tl!("XML Schema")), (Kind::Dtd, tl!("DTD"))],
            );
        });
    }
    if let Some(message) = &form.error {
        ui.add_space(kit::GAP_GROUP);
        kit::banner(ui, Tone::Bad, message);
    }
    match kit::footer(ui, tl!("Cancel"), tl!("Assign"), Some("check"), true, "") {
        Footer::Primary => action = Some(Action::Assign),
        Footer::Cancel => action = Some(Action::Cancel),
        Footer::Nothing => {}
    }
    action
}
