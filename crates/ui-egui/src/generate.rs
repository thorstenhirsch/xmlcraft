//! The Generate Schema and Generate Sample XML dialogs: forms for the options of the generators. The work is done on worker threads
//! (`xmlcraft_engine::generate`), so the window does not wait; the results are opened as new documents.

use xmlcraft_engine::generate::{GrammarKind, SampleJob, SchemaJob};
use xmlcraft_engine::json_infer::{InferJob, Options as JsonInferOptions};
use xmlcraft_engine::json_sample::{Options as JsonSampleOptions, SampleJob as JsonSampleJob};
use xmlcraft_infer::{EnumerationRule, Layout, Options as InferOptions, Output};
use xmlcraft_sample::{ChoicePolicy, Fill, Options as SampleOptions};
use xmlcraft_text::Buffer;

use crate::XmlCraftApp;
use crate::dialog::Dialog;
use crate::i18n::{fmt, tl};
use crate::kit::{self, Footer, Tone};
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

    /// Learn a JSON Schema from the JSON, YAML or BSON document being shown (Generate JSON Schema…), on a worker thread; the schema
    /// opens as a new document when it is ready.
    pub(crate) fn generate_json_schema(&mut self) {
        let Some(doc) = self.session.active_document() else { return };
        let language = doc.language();
        if !language.is_data() {
            self.status = tl!("Generate JSON Schema learns from a JSON, YAML or BSON document").to_owned();
            return;
        }
        let documents = vec![(doc.editor().text(), language)];
        let serial = self.next_serial();
        self.json_schema_waiting = Some(serial);
        self.json_schema_generator.submit(InferJob { serial, documents, options: JsonInferOptions::default() });
        self.status = tl!("Generating a JSON Schema…").to_owned();
    }

    /// Make a sample JSON value from the JSON Schema being shown (Generate Sample JSON…), on a worker thread: the documents the
    /// schema refers to are read through the resolver, which may download them. The sample opens as a new document when it is ready.
    pub(crate) fn generate_json_sample(&mut self) {
        let Some(doc) = self.session.active_document() else { return };
        if !doc.language().is_data() {
            self.status = tl!("Generate Sample JSON makes a sample from a JSON Schema").to_owned();
            return;
        }
        let schema = doc.editor().text();
        let location = doc.path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let open = self.open_documents();
        let serial = self.next_serial();
        self.json_sample_waiting = Some(serial);
        self.json_sample_generator.submit(JsonSampleJob { serial, schema, location, open, options: JsonSampleOptions::default() });
        self.status = tl!("Generating a sample…").to_owned();
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
    pub(crate) fn open_generated(&mut self, name: &str, text: &str) {
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
        for outcome in self.json_schema_generator.poll() {
            if self.json_schema_waiting != Some(outcome.serial) {
                continue; // an answer to something that was asked before
            }
            self.json_schema_waiting = None;
            match outcome.result {
                Ok(inferred) => {
                    self.open_generated("schema.json", &inferred.schema);
                    self.status = tl!("Generated schema.json").to_owned();
                }
                Err(message) => self.status = message,
            }
        }
        for outcome in self.json_sample_generator.poll() {
            if self.json_sample_waiting != Some(outcome.serial) {
                continue; // an answer to something that was asked before
            }
            self.json_sample_waiting = None;
            match outcome.result {
                Ok(done) => {
                    self.open_generated("sample.json", &done.text);
                    self.status = match done.warnings.first() {
                        Some(first) => fmt(tl!("Generated sample.json; {first}"), &[("first", first)]),
                        None => tl!("Generated sample.json").to_owned(),
                    };
                }
                Err(message) => self.status = message,
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
        let (icon, title, subtitle) = match &dialog {
            Dialog::GenerateSchema(_) => {
                ("file-code", tl!("Generate Schema").to_owned(), tl!("Learn an XML Schema or a DTD from the documents that are open.").to_owned())
            }
            Dialog::GenerateSample(form) => {
                let what = match form.kind {
                    Some(GrammarKind::Xsd) => tl!("an XML Schema"),
                    Some(GrammarKind::Dtd) => tl!("a DTD"),
                    None => tl!("a schema"),
                };
                (
                    "sparkles",
                    tl!("Generate Sample XML").to_owned(),
                    fmt(tl!("Make a document that is valid against {name}, {what}."), &[("name", &form.name), ("what", what)]),
                )
            }
            _ => ("sparkles", String::new(), String::new()),
        };
        let (mut action, closed) = kit::dialog(ctx, "xmlcraft-generate", 520.0, icon, &title, &subtitle, |ui| match &mut dialog {
            Dialog::GenerateSchema(form) => schema_form(ui, form),
            Dialog::GenerateSample(form) => sample_form(ui, form),
            _ => None,
        });
        if action.is_none() && closed {
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

fn number(ui: &mut egui::Ui, value: &mut usize, range: std::ops::RangeInclusive<usize>, name: &str) {
    let response = ui.add(egui::DragValue::new(value).range(range).speed(0.2));
    crate::widgets::name_for_assistive_technology(ui, &response, name);
}

/// What the dialog says while a worker is at it.
fn working(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(kit::GAP_GROUP);
    ui.horizontal(|ui| {
        ui.add(egui::Spinner::new().size(14.0).color(t.accent));
        ui.label(egui::RichText::new(text).font(theme::regular(12.5)).color(t.text_muted));
    });
}

/// The three layouts of a generated schema, as cards with a small drawing of each.
fn layout_cards(ui: &mut egui::Ui, layout: &mut Layout) {
    let t = Tokens::get(ui.ctx());
    let cards = [
        (Layout::RussianDoll, tl!("Russian doll"), tl!("Nested, as in the document.")),
        (Layout::SalamiSlice, tl!("Salami slice"), tl!("Every element declared once, by name.")),
        (Layout::VenetianBlind, tl!("Venetian blind"), tl!("Types declared once, elements use them.")),
    ];
    let gap = 10.0;
    let width = (ui.available_width() - 2.0 * gap) / 3.0;
    let enabled = ui.is_enabled();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (value, title, text) in cards {
            let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 96.0), egui::Sense::click());
            let selected = *layout == value;
            response.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, enabled, selected, title));
            let press = theme::Press::track(ui, &response);
            let fill = if selected { t.accent_soft } else { press.fill(t.card, t.hover, t.pressed) };
            let edge = if selected { t.accent } else { t.border.lerp_to_gamma(t.text_muted, press.hover * 0.4) };
            ui.painter().rect(rect, 10.0, fill, egui::Stroke::new(if selected { 1.5 } else { 1.0 }, edge), egui::StrokeKind::Inside);
            diagram(
                ui,
                egui::Rect::from_min_size(rect.min + egui::vec2(12.0, 10.0), egui::vec2(width - 24.0, 28.0)),
                value,
                if selected { t.accent_text } else { t.text_faint },
            );
            ui.painter().text(rect.min + egui::vec2(12.0, 46.0), egui::Align2::LEFT_TOP, title, theme::semibold(12.5), t.text);
            let galley = ui.painter().layout(text.to_owned(), theme::regular(11.0), t.text_muted, width - 24.0);
            ui.painter().galley(rect.min + egui::vec2(12.0, 64.0), galley, t.text_muted);
            if theme::hand(response).clicked() && enabled {
                *layout = value;
            }
        }
    });
}

/// A tiny drawing of how a layout arranges declarations.
fn diagram(ui: &egui::Ui, rect: egui::Rect, layout: Layout, color: egui::Color32) {
    let p = ui.painter();
    let stroke = egui::Stroke::new(1.5, color);
    let (w, h) = (rect.width().min(120.0), rect.height());
    let o = rect.min;
    match layout {
        // Boxes inside boxes.
        Layout::RussianDoll => {
            p.rect_stroke(egui::Rect::from_min_size(o, egui::vec2(w, h)), 4.0, stroke, egui::StrokeKind::Inside);
            p.rect_stroke(egui::Rect::from_min_size(o + egui::vec2(8.0, 6.0), egui::vec2(w - 16.0, h - 12.0)), 3.0, stroke, egui::StrokeKind::Inside);
            p.rect_stroke(
                egui::Rect::from_min_size(o + egui::vec2(16.0, 11.0), egui::vec2(w - 32.0, h - 22.0)),
                2.0,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        // Separate boxes, joined by lines.
        Layout::SalamiSlice => {
            let bw = (w - 20.0) / 3.0;
            for i in 0..3 {
                let x = o.x + i as f32 * (bw + 10.0);
                p.rect_stroke(egui::Rect::from_min_size(egui::pos2(x, o.y + 4.0), egui::vec2(bw, h - 8.0)), 3.0, stroke, egui::StrokeKind::Inside);
                if i < 2 {
                    p.hline(x + bw..=x + bw + 10.0, o.y + h / 2.0, stroke);
                }
            }
        }
        // Slats: the types, with elements that use them.
        Layout::VenetianBlind => {
            for i in 0..4 {
                let y = o.y + 2.0 + i as f32 * (h - 4.0) / 3.5;
                p.line_segment([egui::pos2(o.x, y), egui::pos2(o.x + w, y)], stroke);
            }
        }
    }
}

fn schema_form(ui: &mut egui::Ui, form: &mut SchemaForm) -> Option<Action> {
    let busy = form.waiting != 0;
    ui.add_enabled_ui(!busy, |ui| {
        if form.inputs.len() > 1 {
            kit::section(ui, tl!("Learn from"), |ui| {
                for input in &mut form.inputs {
                    ui.add(widgets::checkbox(&mut input.used, input.name.clone()));
                }
            });
        }
        kit::section(ui, tl!("Write"), |ui| {
            let mut kind = form.dtd;
            kit::segmented(ui, &mut kind, &[(false, tl!("XML Schema")), (true, tl!("DTD"))]);
            form.dtd = kind;
        });
        ui.add_enabled_ui(!form.dtd, |ui| {
            kit::section(ui, tl!("Layout"), |ui| layout_cards(ui, &mut form.layout));
        });
        kit::section(ui, tl!("Values"), |ui| {
            ui.add(widgets::checkbox(&mut form.detect_types, tl!("Detect types (boolean, integer, decimal, date, …)")));
            ui.add(widgets::checkbox(&mut form.unbounded, tl!("An element seen more than once may occur any number of times")));
            ui.add(widgets::checkbox(&mut form.enumerations, tl!("List the values of what has only a few")));
            ui.add_enabled_ui(form.enumerations, |ui| {
                ui.add_space(4.0);
                kit::row(ui, tl!("Most values"), tl!("More distinct values than this are not made a list."), |ui| {
                    number(ui, &mut form.rule.max_values, 2..=100, tl!("The most values of an enumeration"));
                });
                kit::row(ui, tl!("Seen at least"), tl!("A value has to occur this many times to be listed."), |ui| {
                    number(ui, &mut form.rule.min_occurrences, 1..=100, tl!("The fewest times each value is seen"));
                });
            });
        });
    });
    if busy {
        working(ui, tl!("Generating…"));
    }
    if let Some(message) = &form.error {
        ui.add_space(kit::GAP_GROUP);
        kit::banner(ui, Tone::Bad, message);
    }
    let ready = !busy && form.inputs.iter().any(|i| i.used);
    let why = if form.inputs.iter().any(|i| i.used) { "" } else { tl!("Choose a document to learn from.") };
    action_of(kit::footer(ui, tl!("Cancel"), tl!("Generate"), Some("sparkles"), ready, why))
}

fn sample_form(ui: &mut egui::Ui, form: &mut SampleForm) -> Option<Action> {
    let reading = matches!(form.phase, Phase::Reading(_));
    let busy = !matches!(form.phase, Phase::Ready);
    ui.add_enabled_ui(!busy, |ui| {
        kit::section(ui, tl!("Start with"), |ui| {
            let current = match form.root.checked_sub(1).and_then(|i| form.roots.get(i)) {
                Some(name) => name.clone(),
                None => tl!("the first element that can be made").to_owned(),
            };
            let combo =
                egui::ComboBox::from_id_salt("xmlcraft-sample-root").selected_text(current).width(ui.available_width() - 8.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut form.root, 0, tl!("the first element that can be made"));
                    for (i, name) in form.roots.iter().enumerate() {
                        ui.selectable_value(&mut form.root, i + 1, name.clone());
                    }
                });
            crate::widgets::name_for_assistive_technology(ui, &combo.response, tl!("The element to start with"));
        });
        kit::section(ui, tl!("Content"), |ui| {
            ui.add(widgets::checkbox(&mut form.options.optional, tl!("Optional elements and attributes")));
            ui.add_enabled(form.has_file, widgets::checkbox(&mut form.location, tl!("Say where the schema is (xsi:schemaLocation)")));
            ui.add_space(4.0);
            kit::row(ui, tl!("Repetitions"), tl!("How often an element that may repeat occurs."), |ui| {
                number(ui, &mut form.options.repeat, 0..=100, tl!("How often an element repeats"));
            });
            kit::row(ui, tl!("Choices"), "", |ui| {
                kit::segmented(
                    ui,
                    &mut form.options.choice,
                    &[(ChoicePolicy::First, tl!("First branch")), (ChoicePolicy::Rotate, tl!("Each branch in turn"))],
                );
            });
            kit::row(ui, tl!("Text"), "", |ui| {
                kit::segmented(ui, &mut form.options.fill, &[(Fill::Typed, tl!("Typed value")), (Fill::Named, tl!("Element name"))]);
            });
            kit::row(ui, tl!("Depth"), tl!("Elements nest at most this deep, then only what is required."), |ui| {
                number(ui, &mut form.options.max_depth, 0..=50, tl!("How deep elements nest"));
            });
        });
    });
    if busy {
        working(ui, if reading { tl!("Reading the schema…") } else { tl!("Generating…") });
    }
    if let Some(message) = &form.error {
        ui.add_space(kit::GAP_GROUP);
        kit::banner(ui, Tone::Bad, message);
    }
    action_of(kit::footer(ui, tl!("Cancel"), tl!("Generate"), Some("sparkles"), !busy && form.kind.is_some(), ""))
}

fn action_of(footer: Footer) -> Option<Action> {
    match footer {
        Footer::Nothing => None,
        Footer::Cancel => Some(Action::Cancel),
        Footer::Primary => Some(Action::Generate),
    }
}
