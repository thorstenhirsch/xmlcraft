//! The Generate Schema and Generate Sample XML dialogs: forms for the options of the generators. The work is done on worker threads
//! (`xmlcraft_engine::generate`), so the window does not wait; the results are opened as new documents.

use xmlcraft_engine::generate::{GrammarKind, SampleJob, SchemaJob};
use xmlcraft_infer::{EnumerationRule, Layout, Options as InferOptions, Output};
use xmlcraft_sample::{ChoicePolicy, Fill, Options as SampleOptions};
use xmlcraft_text::Buffer;

use crate::XmlCraftApp;
use crate::dialog::Dialog;
use crate::i18n::{fmt, tl};
use crate::theme::{self, Tokens};
use crate::widgets;

/// A document the schema can be learned from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Input {
    id: u64,
    name: String,
    used: bool,
}

/// The options of Generate Schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SchemaForm {
    dtd: bool,
    layout: Layout,
    detect_types: bool,
    enumerations: bool,
    rule: EnumerationRule,
    unbounded: bool,
    /// The open documents, the one the command was given in first and checked.
    inputs: Vec<Input>,
    /// The request being waited for (0: none).
    waiting: u64,
    error: Option<String>,
}

/// Where the sample dialog is: reading the schema to learn its elements, ready, or making the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Reading(u64),
    Ready,
    Making(u64),
}

/// The options of Generate Sample XML.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SampleForm {
    source: u64,
    name: String,
    kind: Option<GrammarKind>,
    /// The elements the document can start with; `root` is 0 for "the first that can be made", else 1 + an index of these.
    roots: Vec<String>,
    root: usize,
    options: SampleOptions,
    /// Say where the schema is (it has to be a file).
    location: bool,
    has_file: bool,
    phase: Phase,
    error: Option<String>,
}

/// What the person did in a form.
enum Action {
    Generate,
    Cancel,
}

impl XmlCraftApp {
    /// The documents that are open, by their paths, so that a schema being edited is read as it is.
    pub(crate) fn open_documents(&self) -> Vec<(String, Buffer)> {
        self.session.documents().iter().filter_map(|d| d.path().map(|p| (p.to_string_lossy().into_owned(), d.editor().buffer().clone()))).collect()
    }

    fn next_serial(&mut self) -> u64 {
        self.generate_serial += 1;
        self.generate_serial
    }

    /// Ask what to generate a schema from, and how (Generate Schema…).
    pub(crate) fn open_generate_schema(&mut self) {
        let Some(active) = self.session.active_document().map(|d| d.id()) else { return };
        let mut inputs: Vec<Input> = vec![];
        for d in self.session.documents() {
            let input = Input { id: d.id(), name: d.name().to_owned(), used: d.id() == active };
            if d.id() == active { inputs.insert(0, input) } else { inputs.push(input) }
        }
        let rule = EnumerationRule::default();
        self.dialog = Some(Dialog::GenerateSchema(Box::new(SchemaForm {
            dtd: false,
            layout: Layout::default(),
            detect_types: true,
            enumerations: true,
            rule,
            unbounded: true,
            inputs,
            waiting: 0,
            error: None,
        })));
    }

    /// Ask how to make a document from the schema or the DTD being shown (Generate Sample XML…), and start reading it to learn the
    /// elements it can start with.
    pub(crate) fn open_generate_sample(&mut self) {
        let Some(doc) = self.session.active_document() else { return };
        let (source, name, has_file) = (doc.id(), doc.name().to_owned(), doc.path().is_some());
        let serial = self.next_serial();
        let form = SampleForm {
            source,
            name,
            kind: None,
            roots: Vec::new(),
            root: 0,
            options: SampleOptions::default(),
            location: false,
            has_file,
            phase: Phase::Reading(serial),
            error: None,
        };
        let job = self.sample_job(&form, serial);
        self.dialog = Some(Dialog::GenerateSample(Box::new(form)));
        match job {
            Some(job) => self.sample_generator.submit(job),
            None => self.set_form_error("the document is gone"),
        }
    }

    fn set_form_error(&mut self, message: &str) {
        match &mut self.dialog {
            Some(Dialog::GenerateSample(form)) => {
                form.error = Some(message.to_owned());
                form.phase = Phase::Ready;
            }
            Some(Dialog::GenerateSchema(form)) => {
                form.error = Some(message.to_owned());
                form.waiting = 0;
            }
            _ => {}
        }
    }

    /// The request that makes a document from the grammar in the document `form` is about, with the options of the form.
    fn sample_job(&self, form: &SampleForm, serial: u64) -> Option<SampleJob> {
        let doc = self.session.position_of(form.source).and_then(|i| self.session.document(i))?;
        let mut options = form.options.clone();
        if form.location && form.has_file {
            options.schema_location = doc.path().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        }
        let root = form.root.checked_sub(1).and_then(|i| form.roots.get(i)).cloned();
        Some(SampleJob {
            serial,
            grammar: doc.editor().text(),
            location: doc.path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            open: self.open_documents(),
            kind: None,
            root,
            options,
        })
    }

    /// A new document that is not a file yet, with `text`, shown.
    fn open_generated(&mut self, name: &str, text: &str) {
        let index = self.session.new_document();
        if let Some(doc) = self.session.document_mut(index) {
            doc.name_untitled(name);
            doc.editor_mut().load(text);
        }
        self.focus_editor = true;
    }

    /// Collect what the generators finished, and act on what the dialog waited for. Called every frame.
    pub(crate) fn pump_generate(&mut self) {
        for outcome in self.schema_generator.poll() {
            let waiting = matches!(&self.dialog, Some(Dialog::GenerateSchema(form)) if form.waiting == outcome.serial);
            if !waiting {
                continue; // cancelled meanwhile
            }
            match outcome.result {
                Ok(generated) => {
                    let names: Vec<String> = generated.files.iter().map(|f| f.name.clone()).collect();
                    for file in &generated.files {
                        self.open_generated(&file.name, &file.text);
                    }
                    self.dialog = None;
                    self.status = fmt(tl!("Generated {names}"), &[("names", &names.join(", "))]);
                }
                Err(message) => self.set_form_error(&message),
            }
        }
        for outcome in self.sample_generator.poll() {
            let phase = match &self.dialog {
                Some(Dialog::GenerateSample(form)) => form.phase,
                _ => continue, // cancelled meanwhile
            };
            match (phase, outcome.result) {
                (Phase::Reading(serial), Ok(done)) if serial == outcome.serial => {
                    if let Some(Dialog::GenerateSample(form)) = &mut self.dialog {
                        form.roots = done.roots;
                        form.kind = Some(done.kind);
                        form.phase = Phase::Ready;
                    }
                }
                (Phase::Making(serial), Ok(done)) if serial == outcome.serial => {
                    self.open_generated("sample.xml", &done.text);
                    self.dialog = None;
                    self.status = match done.warnings.first() {
                        Some(first) => fmt(
                            tl!("Generated sample.xml; {count} things could not be made as asked: {first}"),
                            &[("count", &done.warnings.len().to_string()), ("first", first)],
                        ),
                        None => tl!("Generated sample.xml").to_owned(),
                    };
                }
                (Phase::Reading(serial) | Phase::Making(serial), Err(message)) if serial == outcome.serial => self.set_form_error(&message),
                _ => {} // an answer to something that was asked before
            }
        }
    }

    /// Draw the form that is open and act on what was done in it.
    pub(crate) fn show_generate(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let t = Tokens::get(ctx);
        let mut action = None;
        let response = egui::Modal::new(egui::Id::unique("xmlcraft-generate")).show(ctx, |ui| {
            ui.set_width(460.0);
            match &mut dialog {
                Dialog::GenerateSchema(form) => action = schema_form(ui, &t, form),
                Dialog::GenerateSample(form) => action = sample_form(ui, &t, form),
                _ => {}
            }
        });
        if action.is_none() && response.should_close() {
            action = Some(Action::Cancel);
        }
        match action {
            Some(Action::Cancel) => {}
            Some(Action::Generate) => {
                let serial = self.next_serial();
                match &mut dialog {
                    Dialog::GenerateSchema(form) => {
                        let documents: Vec<String> = form
                            .inputs
                            .iter()
                            .filter(|i| i.used)
                            .filter_map(|i| self.session.position_of(i.id).and_then(|p| self.session.document(p)))
                            .map(|d| d.editor().text())
                            .collect();
                        let options = InferOptions {
                            output: if form.dtd { Output::Dtd } else { Output::Xsd { layout: form.layout } },
                            detect_types: form.detect_types,
                            enumerations: form.enumerations.then_some(form.rule),
                            unbounded: form.unbounded,
                        };
                        form.waiting = serial;
                        form.error = None;
                        self.schema_generator.submit(SchemaJob { serial, documents, options });
                    }
                    Dialog::GenerateSample(form) => {
                        form.phase = Phase::Making(serial);
                        form.error = None;
                        match self.sample_job(form, serial) {
                            Some(job) => self.sample_generator.submit(job),
                            None => {
                                form.error = Some("the document is gone".to_owned());
                                form.phase = Phase::Ready;
                            }
                        }
                    }
                    _ => {}
                }
                self.dialog = Some(dialog);
            }
            None => self.dialog = Some(dialog),
        }
    }
}

/// A radio button for `option` of `value`.
fn choose<T: PartialEq>(ui: &mut egui::Ui, value: &mut T, option: T, label: &str) {
    if ui.add(widgets::radio(*value == option, label)).clicked() {
        *value = option;
    }
}

fn heading(ui: &mut egui::Ui, t: &Tokens, title: &str, subtitle: &str) {
    ui.label(egui::RichText::new(title).font(theme::semibold(15.0)).color(t.text));
    ui.add_space(4.0);
    ui.label(egui::RichText::new(subtitle).font(theme::regular(13.0)).color(t.text_muted));
    ui.add_space(10.0);
}

fn caption(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(text).font(theme::medium(12.0)).color(t.text_muted));
}

/// The buttons: Cancel and the primary one (right to left, so the primary is drawn first, at the right).
fn buttons(ui: &mut egui::Ui, t: &Tokens, primary: &str, enabled: bool) -> Option<Action> {
    let mut action = None;
    ui.add_space(14.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let button = egui::Button::new(egui::RichText::new(primary).color(egui::Color32::WHITE)).fill(t.accent);
        if ui.add_enabled(enabled, button).clicked() {
            action = Some(Action::Generate);
        }
        if ui.button(tl!("Cancel")).clicked() {
            action = Some(Action::Cancel);
        }
    });
    action
}

fn error_line(ui: &mut egui::Ui, t: &Tokens, error: &Option<String>) {
    if let Some(message) = error {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(message).font(theme::regular(13.0)).color(crate::editor_view::SyntaxTokens::for_kind(t.kind).error));
    }
}

fn number(ui: &mut egui::Ui, value: &mut usize, range: std::ops::RangeInclusive<usize>, name: &str) {
    let response = ui.add(egui::DragValue::new(value).range(range));
    crate::widgets::name_for_assistive_technology(ui, &response, name);
}

fn schema_form(ui: &mut egui::Ui, t: &Tokens, form: &mut SchemaForm) -> Option<Action> {
    heading(ui, t, tl!("Generate Schema"), tl!("Learn an XML Schema or a DTD from the documents that are open."));
    let busy = form.waiting != 0;
    ui.add_enabled_ui(!busy, |ui| {
        if form.inputs.len() > 1 {
            caption(ui, t, tl!("Learn from"));
            for input in &mut form.inputs {
                ui.add(widgets::checkbox(&mut input.used, input.name.clone()));
            }
        }
        caption(ui, t, tl!("Write"));
        ui.horizontal(|ui| {
            choose(ui, &mut form.dtd, false, tl!("XML Schema"));
            choose(ui, &mut form.dtd, true, tl!("DTD"));
        });
        ui.add_enabled_ui(!form.dtd, |ui| {
            caption(ui, t, tl!("Layout"));
            ui.horizontal_wrapped(|ui| {
                choose(ui, &mut form.layout, Layout::RussianDoll, tl!("Russian doll"));
                choose(ui, &mut form.layout, Layout::SalamiSlice, tl!("Salami slice"));
                choose(ui, &mut form.layout, Layout::VenetianBlind, tl!("Venetian blind"));
            });
        });
        caption(ui, t, tl!("Values"));
        ui.add(widgets::checkbox(&mut form.detect_types, tl!("Detect types (boolean, integer, decimal, date, …)")));
        ui.horizontal(|ui| {
            ui.add(widgets::checkbox(&mut form.enumerations, tl!("Enumerations of at most")));
            ui.add_enabled_ui(form.enumerations, |ui| {
                number(ui, &mut form.rule.max_values, 2..=100, tl!("The most values of an enumeration"));
                ui.label(tl!("values, each seen at least"));
                number(ui, &mut form.rule.min_occurrences, 1..=100, tl!("The fewest times each value is seen"));
                ui.label(tl!("times"));
            });
        });
        ui.add(widgets::checkbox(&mut form.unbounded, tl!("An element seen more than once may occur any number of times")));
    });
    if busy {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(tl!("Generating…")).font(theme::regular(13.0)).color(t.text_muted));
    }
    error_line(ui, t, &form.error);
    buttons(ui, t, tl!("Generate"), !busy && form.inputs.iter().any(|i| i.used))
}

fn sample_form(ui: &mut egui::Ui, t: &Tokens, form: &mut SampleForm) -> Option<Action> {
    let what = match form.kind {
        Some(GrammarKind::Xsd) => tl!("an XML Schema"),
        Some(GrammarKind::Dtd) => tl!("a DTD"),
        None => tl!("a schema"),
    };
    heading(
        ui,
        t,
        tl!("Generate Sample XML"),
        &fmt(tl!("Make a document that is valid against {name}, {what}."), &[("name", &form.name), ("what", what)]),
    );
    let reading = matches!(form.phase, Phase::Reading(_));
    let busy = !matches!(form.phase, Phase::Ready);
    ui.add_enabled_ui(!busy, |ui| {
        caption(ui, t, tl!("Start with"));
        let current = match form.root.checked_sub(1).and_then(|i| form.roots.get(i)) {
            Some(name) => name.clone(),
            None => tl!("the first element that can be made").to_owned(),
        };
        let combo = egui::ComboBox::from_id_salt("xmlcraft-sample-root").selected_text(current).width(300.0).show_ui(ui, |ui| {
            ui.selectable_value(&mut form.root, 0, tl!("the first element that can be made"));
            for (i, name) in form.roots.iter().enumerate() {
                ui.selectable_value(&mut form.root, i + 1, name.clone());
            }
        });
        crate::widgets::name_for_assistive_technology(ui, &combo.response, tl!("The element to start with"));
        caption(ui, t, tl!("Content"));
        ui.add(widgets::checkbox(&mut form.options.optional, tl!("Optional elements and attributes")));
        ui.horizontal(|ui| {
            ui.label(tl!("An element that may repeat occurs"));
            number(ui, &mut form.options.repeat, 0..=100, tl!("How often an element repeats"));
            ui.label(tl!("times"));
        });
        ui.horizontal(|ui| {
            ui.label(tl!("Of a choice:"));
            choose(ui, &mut form.options.choice, ChoicePolicy::First, tl!("the first branch"));
            choose(ui, &mut form.options.choice, ChoicePolicy::Rotate, tl!("each branch in turn"));
        });
        ui.horizontal(|ui| {
            ui.label(tl!("Text:"));
            choose(ui, &mut form.options.fill, Fill::Typed, tl!("a value of its type"));
            choose(ui, &mut form.options.fill, Fill::Named, tl!("the name of its element"));
        });
        ui.horizontal(|ui| {
            ui.label(tl!("Nest at most"));
            number(ui, &mut form.options.max_depth, 0..=50, tl!("How deep elements nest"));
            ui.label(tl!("levels deep, then only what is required"));
        });
        ui.add_enabled(form.has_file, widgets::checkbox(&mut form.location, tl!("Say where the schema is (xsi:schemaLocation)")));
    });
    if busy {
        ui.add_space(8.0);
        let text = if reading { tl!("Reading the schema…") } else { tl!("Generating…") };
        ui.label(egui::RichText::new(text).font(theme::regular(13.0)).color(t.text_muted));
    }
    error_line(ui, t, &form.error);
    buttons(ui, t, tl!("Generate"), !busy && form.kind.is_some())
}
