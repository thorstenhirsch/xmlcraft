//! The Convert dialog: a document written as XML, JSON, YAML or BSON (its Extended JSON text), made into a new document in another of
//! them (`xmlcraft_convert`). Nothing is changed in the document itself; the result opens as a new document.

use xmlcraft_convert::{ConvertOptions, DataToXml, Format, XmlToData, convert};
use xmlcraft_engine::language::{JsonFlavor, Language};

use crate::XmlCraftApp;
use crate::dialog::Dialog;
use crate::i18n::{fmt, tl};
use crate::kit::{self, Footer, Tone};
use crate::widgets;

/// The most bytes of a document the dialog converts.
const MAX_BYTES: usize = 16 * 1024 * 1024;

/// The options of Convert.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConvertForm {
    source: u64,
    name: String,
    from: Format,
    json_lines: bool,
    to: Format,
    /// Between XML and data: the mapping of XPath 3.1 (`fn:json-to-xml`) instead of the natural one.
    standard: bool,
    guess_types: bool,
    declaration: bool,
    root: String,
    indent: usize,
    error: Option<String>,
}

impl ConvertForm {
    /// Whether either side is XML.
    fn involves_xml(&self) -> bool {
        self.from == Format::Xml || self.to == Format::Xml
    }
}

fn format_of(language: Language) -> (Format, bool) {
    match language {
        Language::Xml => (Format::Xml, false),
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

impl XmlCraftApp {
    /// Ask what to convert the document being shown to (Convert…).
    pub(crate) fn open_convert(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        let Some(doc) = self.session.active_document() else { return };
        let (from, json_lines) = format_of(doc.language());
        let to = if from == Format::Json { Format::Yaml } else { Format::Json };
        self.dialog = Some(Dialog::Convert(Box::new(ConvertForm {
            source: doc.id(),
            name: doc.name().to_owned(),
            from,
            json_lines,
            to,
            standard: false,
            guess_types: false,
            declaration: true,
            root: DataToXml::default().root,
            indent: 2,
            error: None,
        })));
    }

    /// Convert with the options of the form and open the result.
    fn run_convert(&mut self, form: &mut ConvertForm) -> bool {
        let Some(doc) = self.session.position_of(form.source).and_then(|i| self.session.document(i)) else {
            form.error = Some(tl!("The document is gone.").to_owned());
            return false;
        };
        let text = doc.editor().text();
        if text.len() > MAX_BYTES {
            form.error = Some(fmt(tl!("The document is larger than {limit} MB."), &[("limit", &(MAX_BYTES / (1024 * 1024)).to_string())]));
            return false;
        }
        let root = form.root.trim().to_owned();
        let options = ConvertOptions {
            xml_to_data: XmlToData { guess_types: form.guess_types, ..XmlToData::default() },
            data_to_xml: DataToXml {
                declaration: form.declaration,
                root: if root.is_empty() { DataToXml::default().root } else { root },
                ..DataToXml::default()
            },
            indent: Some(form.indent),
            json_lines: form.json_lines,
            standard_mapping: form.standard,
        };
        match convert(&text, form.from, form.to, &options) {
            Ok(done) => {
                let name = converted_name(&form.name, form.to);
                self.open_generated(&name, &done.text);
                self.status = match done.warnings.first() {
                    Some(first) => fmt(
                        tl!("Converted to {name}; {count} things could not be kept as they were: {first}"),
                        &[("name", &name), ("count", &done.warnings.len().to_string()), ("first", first)],
                    ),
                    None => fmt(tl!("Converted to {name}"), &[("name", &name)]),
                };
                true
            }
            Err(e) => {
                form.error = Some(fmt(tl!("The document cannot be converted: {why}"), &[("why", &e.to_string())]));
                false
            }
        }
    }

    /// Draw the form and act on what was done in it.
    pub(crate) fn show_convert(&mut self, ctx: &egui::Context) {
        let Some(Dialog::Convert(mut form)) = self.dialog.take() else { return };
        let subtitle = fmt(tl!("Make a new document from {name}; the document itself stays as it is."), &[("name", &form.name)]);
        let (action, closed) =
            kit::dialog(ctx, "xmlcraft-convert", 520.0, "wand-sparkles", tl!("Convert document"), &subtitle, |ui| convert_form(ui, &mut form));
        match action {
            Some(Footer::Primary) => {
                if self.run_convert(&mut form) {
                    return;
                }
                self.dialog = Some(Dialog::Convert(form));
            }
            Some(Footer::Cancel) => {}
            _ if closed => {}
            _ => self.dialog = Some(Dialog::Convert(form)),
        }
    }
}

fn convert_form(ui: &mut egui::Ui, form: &mut ConvertForm) -> Option<Footer> {
    kit::section(ui, tl!("Convert to"), |ui| {
        let formats = [Format::Xml, Format::Json, Format::Yaml, Format::Bson];
        let options: Vec<(Format, &str)> = formats.iter().map(|f| (*f, label(*f))).collect();
        kit::segmented(ui, &mut form.to, &options);
        ui.add_space(4.0);
        kit::help(ui, &fmt(tl!("From {from}."), &[("from", label(form.from))]));
        if form.from == Format::Bson || form.to == Format::Bson {
            kit::help(ui, tl!("BSON is shown as MongoDB Extended JSON text; its types are plain values in the other formats."));
        }
    });
    if form.involves_xml() {
        kit::section(ui, tl!("XML and data"), |ui| {
            kit::segmented(ui, &mut form.standard, &[(false, tl!("Natural")), (true, tl!("XPath 3.1 (json-to-xml)"))]);
            ui.add_space(4.0);
            kit::help(
                ui,
                if form.standard {
                    tl!("The standard mapping of fn:json-to-xml: lossless for JSON, with map, array and string elements.")
                } else {
                    tl!("Elements become members, attributes @name, text beside them #text, repeated elements lists.")
                },
            );
            if !form.standard {
                if form.from == Format::Xml {
                    ui.add(widgets::checkbox(&mut form.guess_types, tl!("Numbers, true, false and null in the text become those")));
                }
                if form.to == Format::Xml {
                    ui.add(widgets::checkbox(&mut form.declaration, tl!("Write the XML declaration")));
                    kit::row(ui, tl!("Root element"), tl!("Used when the data is not one element."), |ui| {
                        let response = ui.add(egui::TextEdit::singleline(&mut form.root).desired_width(140.0));
                        widgets::name_for_assistive_technology(ui, &response, tl!("The name of the root element"));
                    });
                }
            }
        });
    }
    kit::section(ui, tl!("Layout"), |ui| {
        kit::row(ui, tl!("Indentation"), tl!("One line is for JSON and XML; YAML is always indented."), |ui| {
            kit::segmented(ui, &mut form.indent, &[(0, tl!("One line")), (2, tl!("2 spaces")), (4, tl!("4 spaces"))]);
        });
    });
    if let Some(message) = &form.error {
        ui.add_space(kit::GAP_GROUP);
        kit::banner(ui, Tone::Bad, message);
    }
    let same = form.from == form.to && !form.json_lines;
    let why = if same { tl!("Choose another format than the one the document is in.") } else { "" };
    match kit::footer(ui, tl!("Cancel"), tl!("Convert"), Some("wand-sparkles"), !same, why) {
        Footer::Nothing => None,
        other => Some(other),
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
}
