//! Transform, the tool window of converting between formats: the document being shown, written as XML, JSON, YAML or BSON (its Extended
//! JSON text), made into a new document in another of them (`xmlcraft_convert`). The form is the target format and its options; beside
//! it (or under it, in a narrow panel) a read-only preview of the start of the result, which is worked out on a thread a moment after
//! the last change to the form or the text, so typing and clicking never wait for a conversion. "Open as New Document" opens the whole
//! result; nothing is changed in the document itself.

use std::hash::{Hash, Hasher};

use xmlcraft_convert::{ConvertOptions, DataToXml, Format, XmlToData, convert};
use xmlcraft_engine::language::{JsonFlavor, Language};

use crate::http_pane::Outcome;
use crate::i18n::{fmt, tl};
use crate::kit::{self, Tone};
use crate::tools::{self, Bound};
use crate::{Panel, XmlCraftApp, widgets};

/// The most bytes of a document the tool converts.
const MAX_BYTES: usize = 16 * 1024 * 1024;
/// The most lines of the result the preview shows.
const PREVIEW_LINES: usize = 200;
/// How long the form and the text are left alone before the preview is worked out again, in seconds.
const DEBOUNCE: f64 = 0.25;

/// The start of a result, for the preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Preview {
    pub text: String,
    /// The lines of the whole result.
    pub lines: usize,
    /// Things the conversion could not keep as they were.
    pub warnings: Vec<String>,
}

/// The preview being worked out or shown.
#[derive(Clone, Debug, Default, PartialEq)]
struct PreviewState {
    /// What the preview is for now (a hash of the document, its text and the form); changes start the wait.
    key: u64,
    /// When (egui time, seconds) the wait for a quiet moment ends.
    due: Option<f64>,
    /// The request being waited for, and the key it was asked for.
    waiting: Option<(u64, u64)>,
    /// The last answer, and the key it was for.
    shown: Option<(u64, Result<Preview, String>)>,
}

/// The form of the Transform tool window.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TransformTool {
    /// The target; until one is chosen, JSON (YAML for a JSON document).
    to: Option<Format>,
    /// Between XML and data: the mapping of XPath 3.1 (`fn:json-to-xml`) instead of the natural one.
    standard: bool,
    guess_types: bool,
    declaration: bool,
    root: String,
    indent: usize,
    error: Option<String>,
    preview: PreviewState,
}

impl Default for TransformTool {
    fn default() -> Self {
        Self {
            to: None,
            standard: false,
            guess_types: false,
            declaration: true,
            root: DataToXml::default().root,
            indent: 2,
            error: None,
            preview: PreviewState::default(),
        }
    }
}

/// What a document is written as: the format and whether it is JSON Lines.
fn format_of(language: Language) -> (Format, bool) {
    match language {
        // (HTML is converted as the markup it is; text that is not well-formed XML is said so.)
        Language::Xml | Language::Html => (Format::Xml, false),
        Language::Json(flavor) => (Format::Json, flavor == JsonFlavor::Lines),
        Language::Yaml => (Format::Yaml, false),
        Language::Bson => (Format::Bson, false),
    }
}

fn extension(format: Format) -> &'static str {
    match format {
        Format::Xml => "xml",
        Format::Json => "json",
        Format::Yaml => "yaml",
        Format::Bson => "bson",
    }
}

fn label(format: Format) -> &'static str {
    match format {
        Format::Xml => "XML",
        Format::Json => "JSON",
        Format::Yaml => "YAML",
        Format::Bson => "BSON",
    }
}

/// The name of the converted document: that of the source with the extension of the target.
fn converted_name(name: &str, to: Format) -> String {
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    format!("{stem}.{}", extension(to))
}

/// The first `lines` lines of `text`, and how many lines it has.
fn start_of(text: &str, lines: usize) -> (String, usize) {
    let total = text.lines().count();
    (text.lines().take(lines).collect::<Vec<_>>().join("\n"), total)
}

impl TransformTool {
    /// Whether either side is XML.
    fn involves_xml(&self, from: Format, to: Format) -> bool {
        from == Format::Xml || to == Format::Xml
    }

    /// The target now: the one chosen, else JSON (YAML for JSON).
    fn target(&self, from: Format) -> Format {
        self.to.unwrap_or(if from == Format::Json { Format::Yaml } else { Format::Json })
    }

    fn options(&self, json_lines: bool) -> ConvertOptions {
        let root = self.root.trim().to_owned();
        ConvertOptions {
            xml_to_data: XmlToData { guess_types: self.guess_types, ..XmlToData::default() },
            data_to_xml: DataToXml {
                declaration: self.declaration,
                root: if root.is_empty() { DataToXml::default().root } else { root },
                ..DataToXml::default()
            },
            indent: Some(self.indent),
            json_lines,
            standard_mapping: self.standard,
        }
    }

    /// A number that changes when anything the result depends on changes.
    fn key(&self, bound: &Bound, version: u64, from: Format, json_lines: bool, to: Format) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (bound.id, version, extension(from), json_lines, extension(to)).hash(&mut hasher);
        (self.standard, self.guess_types, self.declaration, &self.root, self.indent).hash(&mut hasher);
        hasher.finish()
    }
}

impl XmlCraftApp {
    /// Convert…: show the Transform panel.
    pub(crate) fn open_convert(&mut self) {
        self.show_tool(Panel::Transform);
    }

    /// Convert with the form and open the result.
    fn run_convert(&mut self) {
        let Some(doc) = self.session.active_document() else { return };
        let (from, json_lines) = format_of(doc.language());
        let to = self.transform.target(from);
        let (text, name) = (doc.editor().text(), doc.name().to_owned());
        if text.len() > MAX_BYTES {
            let limit = (MAX_BYTES / (1024 * 1024)).to_string();
            self.transform.error = Some(fmt(tl!("The document is larger than {limit} MB."), &[("limit", &limit)]));
            return;
        }
        let options = self.transform.options(json_lines);
        match convert(&text, from, to, &options) {
            Ok(done) => {
                let name = converted_name(&name, to);
                self.transform.error = None;
                self.open_generated(&name, &done.text);
                self.status = match done.warnings.first() {
                    Some(first) => fmt(
                        tl!("Converted to {name}; {count} things could not be kept as they were: {first}"),
                        &[("name", &name), ("count", &done.warnings.len().to_string()), ("first", first)],
                    ),
                    None => fmt(tl!("Converted to {name}"), &[("name", &name)]),
                };
            }
            Err(e) => self.transform.error = Some(fmt(tl!("The document cannot be converted: {why}"), &[("why", &e.to_string())])),
        }
    }

    /// Work out the preview on a thread: the whole conversion, of which the start is kept.
    fn start_preview(&mut self, key: u64) {
        let Some(doc) = self.session.active_document() else { return };
        let (from, json_lines) = format_of(doc.language());
        let to = self.transform.target(from);
        let text = doc.editor().text();
        if text.len() > MAX_BYTES {
            let limit = (MAX_BYTES / (1024 * 1024)).to_string();
            let message = fmt(tl!("The document is larger than {limit} MB."), &[("limit", &limit)]);
            self.transform.preview.shown = Some((key, Err(message)));
            return;
        }
        let options = self.transform.options(json_lines);
        let serial = self.next_http_serial();
        let started = self.spawn_http(move || {
            let result = xmlcraft_engine::guard(|| convert(&text, from, to, &options))
                .map_err(|_| tl!("the conversion failed (a bug); nothing was changed").to_owned())
                .and_then(|made| made.map_err(|e| fmt(tl!("The document cannot be converted: {why}"), &[("why", &e.to_string())])))
                .map(|done| {
                    let (start, lines) = start_of(&done.text, PREVIEW_LINES);
                    Preview { text: start, lines, warnings: done.warnings.iter().map(ToString::to_string).collect() }
                });
            Outcome::Preview { serial, result }
        });
        match started {
            Ok(()) => self.transform.preview.waiting = Some((serial, key)),
            Err(why) => self.transform.preview.shown = Some((key, Err(why))),
        }
    }

    /// The answer of a preview thread, whatever else is open. An answer for an older request is dropped.
    pub(crate) fn preview_answered(&mut self, serial: u64, result: Result<Preview, String>) {
        let preview = &mut self.transform.preview;
        let Some((_, key)) = preview.waiting.take_if(|(s, _)| *s == serial) else { return };
        preview.shown = Some((key, result));
    }
}

/// The body of the Transform panel.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    let bound = tools::bound(app);
    let language = app.session.active_document().map(|d| (d.language(), d.editor().version()));
    let Some((bound, (language, version))) = bound.zip(language) else {
        kit::tool_body(ui, false, |ui| {
            tools::needs(ui, tl!("Transform needs a document"), tl!("Open an XML, JSON, YAML or BSON document to convert it."))
        });
        return;
    };
    let (from, json_lines) = format_of(language);
    let to = app.transform.target(from);
    let same = from == to && !json_lines;
    let why_not = tl!("Choose another format than the one the document is in.");
    let run = kit::tool_footer(ui, tl!("Open as New Document"), Some("arrow-left-right"), !same, why_not);
    // The preview: a new key starts the wait for a quiet moment; when it is over, the thread starts.
    let key = app.transform.key(&bound, version, from, json_lines, to);
    let now = ui.input(|i| i.time);
    if app.transform.preview.key != key {
        app.transform.preview.key = key;
        app.transform.preview.due = Some(now + DEBOUNCE);
    }
    if same {
        app.transform.preview.due = None;
    } else if let Some(due) = app.transform.preview.due {
        if now >= due && app.transform.preview.waiting.is_none() {
            app.transform.preview.due = None;
            app.start_preview(key);
        } else if now < due {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(due - now));
        }
    }
    kit::tool_body(ui, false, |ui| {
        tools::bound_line(ui, &bound.name);
        ui.add_space(kit::GAP);
        tools::columns(ui, &mut app.transform, |ui, tool| form(ui, tool, from, to, json_lines, same), |ui, tool| preview(ui, tool, key, same));
    });
    if run && !same {
        app.run_convert();
    }
}

fn caption(ui: &mut egui::Ui, text: &str) {
    let t = crate::theme::Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(text).font(crate::theme::medium(11.5)).color(t.text_muted));
}

fn form(ui: &mut egui::Ui, tool: &mut TransformTool, from: Format, to: Format, json_lines: bool, same: bool) {
    caption(ui, tl!("Convert to"));
    let formats = [Format::Xml, Format::Json, Format::Yaml, Format::Bson];
    let options: Vec<(Format, &str)> = formats.iter().map(|f| (*f, label(*f))).collect();
    let mut chosen = to;
    if kit::segmented(ui, &mut chosen, &options) {
        tool.to = Some(chosen);
    }
    ui.add_space(4.0);
    let from_line =
        if json_lines { fmt(tl!("From {from} (lines)."), &[("from", label(from))]) } else { fmt(tl!("From {from}."), &[("from", label(from))]) };
    kit::help(ui, &from_line);
    if same {
        kit::help(ui, tl!("Choose another format than the one the document is in."));
    }
    if from == Format::Bson || to == Format::Bson {
        kit::help(ui, tl!("BSON is shown as MongoDB Extended JSON text; its types are plain values in the other formats."));
    }
    if tool.involves_xml(from, to) {
        ui.add_space(kit::GAP);
        caption(ui, tl!("XML and data"));
        kit::segmented(ui, &mut tool.standard, &[(false, tl!("Natural")), (true, tl!("XPath 3.1 (json-to-xml)"))]);
        ui.add_space(4.0);
        kit::help(
            ui,
            if tool.standard {
                tl!("The standard mapping of fn:json-to-xml: lossless for JSON, with map, array and string elements.")
            } else {
                tl!("Elements become members, attributes @name, text beside them #text, repeated elements lists.")
            },
        );
        if !tool.standard {
            if from == Format::Xml {
                ui.add(widgets::checkbox(&mut tool.guess_types, tl!("Numbers, true, false and null in the text become those")));
            }
            if to == Format::Xml {
                ui.add(widgets::checkbox(&mut tool.declaration, tl!("Write the XML declaration")));
                caption(ui, tl!("Root element (used when the data is not one element)"));
                let response = ui.add(egui::TextEdit::singleline(&mut tool.root).desired_width(160.0));
                widgets::name_for_assistive_technology(ui, &response, tl!("The name of the root element"));
            }
        }
    }
    ui.add_space(kit::GAP);
    caption(ui, tl!("Indentation"));
    kit::segmented(ui, &mut tool.indent, &[(0, tl!("One line")), (2, tl!("2 spaces")), (4, tl!("4 spaces"))]);
    kit::help(ui, tl!("One line is for JSON and XML; YAML is always indented."));
    // (A refusal the preview already shows is not said twice.)
    let previewed = matches!(&tool.preview.shown, Some((key, Err(_))) if *key == tool.preview.key);
    if let Some(message) = tool.error.as_ref().filter(|_| !previewed) {
        ui.add_space(kit::GAP);
        kit::banner(ui, Tone::Bad, message);
    }
}

/// The read-only preview of the start of the result, filling the rest of the panel.
fn preview(ui: &mut egui::Ui, tool: &mut TransformTool, key: u64, same: bool) {
    ui.add_space(kit::GAP);
    caption(ui, tl!("Preview"));
    if same {
        kit::help(ui, tl!("Nothing to preview: the target is the format the document is in."));
        return;
    }
    let state = &tool.preview;
    let updating = state.waiting.is_some() || state.due.is_some() || state.shown.as_ref().is_none_or(|(k, _)| *k != key);
    if updating {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Working out the preview…"));
        });
    }
    let Some((_, shown)) = &state.shown else { return };
    match shown {
        Err(message) => {
            kit::banner(ui, Tone::Bad, message);
        }
        Ok(done) => {
            if done.lines > PREVIEW_LINES {
                kit::help(
                    ui,
                    &fmt(tl!("The first {shown} of {lines} lines."), &[("shown", &PREVIEW_LINES.to_string()), ("lines", &done.lines.to_string())]),
                );
            }
            if let Some(first) = done.warnings.first() {
                kit::help(
                    ui,
                    &fmt(
                        tl!("{count} things could not be kept as they were: {first}"),
                        &[("count", &done.warnings.len().to_string()), ("first", first)],
                    ),
                );
            }
            let mut text = done.text.as_str();
            let size = egui::vec2(ui.available_width(), ui.available_height());
            egui::ScrollArea::vertical().id_salt("xmlcraft-transform-preview").auto_shrink([false, false]).show(ui, |ui| {
                let response =
                    ui.add(egui::TextEdit::multiline(&mut text).font(crate::theme::mono(12.0)).desired_width(f32::INFINITY).min_size(size));
                widgets::name_for_assistive_technology(ui, &response, tl!("The preview of the result"));
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_new_document_is_named_after_the_source() {
        assert_eq!(converted_name("order.xml", Format::Json), "order.json");
        assert_eq!(converted_name("a.b.json", Format::Yaml), "a.b.yaml");
        assert_eq!(converted_name("data", Format::Bson), "data.bson");
        assert_eq!(converted_name("Untitled-1", Format::Xml), "Untitled-1.xml");
    }

    #[test]
    fn a_document_is_read_as_the_format_of_its_language() {
        assert_eq!(format_of(Language::Xml), (Format::Xml, false));
        assert_eq!(format_of(Language::Json(JsonFlavor::Jsonc)), (Format::Json, false));
        assert_eq!(format_of(Language::Json(JsonFlavor::Lines)), (Format::Json, true), "JSON Lines are a list");
        assert_eq!(format_of(Language::Yaml), (Format::Yaml, false));
        assert_eq!(format_of(Language::Bson), (Format::Bson, false));
    }

    #[test]
    fn the_preview_is_the_start_of_the_result_by_lines_and_says_how_long_the_result_is() {
        let text = (1..=300).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
        let (start, lines) = start_of(&text, 200);
        assert_eq!((start.lines().count(), lines), (200, 300));
        assert_eq!(start_of("", 200), (String::new(), 0));
        assert_eq!(start_of("a\r\nb\n", 1), ("a".to_owned(), 2), "both line ends count");
    }

    #[test]
    fn the_key_changes_with_the_text_the_target_and_the_options_but_not_with_a_repeat() {
        let tool = TransformTool::default();
        let bound = Bound { id: 1, name: "a.json".into(), xml: false };
        let base = tool.key(&bound, 3, Format::Json, false, Format::Yaml);
        assert_eq!(base, tool.key(&bound, 3, Format::Json, false, Format::Yaml));
        assert_ne!(base, tool.key(&bound, 4, Format::Json, false, Format::Yaml), "the text changed");
        assert_ne!(base, tool.key(&bound, 3, Format::Json, false, Format::Xml), "another target");
        assert_ne!(base, tool.key(&Bound { id: 2, ..bound.clone() }, 3, Format::Json, false, Format::Yaml), "another document");
        let other = TransformTool { indent: 4, ..TransformTool::default() };
        assert_ne!(base, other.key(&bound, 3, Format::Json, false, Format::Yaml), "an option changed");
    }
}
