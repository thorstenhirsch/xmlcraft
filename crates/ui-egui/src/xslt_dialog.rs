//! The Transform window: the document being shown transformed with an XSLT stylesheet (`xmlcraft_query::transform`, on a worker
//! thread), the result opened as a new document. The stylesheet is the one the document names (`<?xml-stylesheet?>`) or another open
//! document; the document and the stylesheet themselves are never changed.

use xmlcraft_engine::xslt::{XsltJob, stylesheet_of};

use crate::XmlCraftApp;
use crate::dialog::Dialog;
use crate::i18n::{fmt, tl};
use crate::kit::{self, Footer, Tone};

/// The most bytes of a document or stylesheet the window transforms.
const MAX_BYTES: usize = 16 * 1024 * 1024;

/// Where the stylesheet comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Sheet {
    /// The one the document names, read from the open document with that path or from the file.
    Named(String),
    /// An open document (by id).
    Open(u64),
}

/// The Transform window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct XsltForm {
    source: u64,
    name: String,
    /// What can be chosen: the label and where it comes from.
    choices: Vec<(String, Sheet)>,
    chosen: usize,
    waiting: Option<u64>,
    error: Option<String>,
}

/// The extension a result is saved under, by what it looks like.
fn result_extension(text: &str) -> &'static str {
    let t = text.trim_start();
    if t.starts_with("<!DOCTYPE html") || t.starts_with("<!doctype html") || t.starts_with("<html") {
        "html"
    } else if t.starts_with('<') {
        "xml"
    } else if t.starts_with(['{', '[']) {
        "json"
    } else {
        "txt"
    }
}

/// The name of the result: that of the source with `-result` and the extension the result needs.
fn result_name(name: &str, extension: &str) -> String {
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    format!("{stem}-result.{extension}")
}

impl XmlCraftApp {
    /// Open the Transform window on the document being shown (Transform with XSLT…).
    pub(crate) fn open_transform(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        let Some(doc) = self.session.active_document() else { return };
        if !doc.language().is_xml() {
            self.status = tl!("XSLT transforms XML documents.").to_owned();
            return;
        }
        let source = doc.id();
        let mut choices: Vec<(String, Sheet)> = Vec::new();
        if let Some(href) = stylesheet_of(&doc.editor().text()) {
            choices.push((fmt(tl!("The one the document names ({href})"), &[("href", &href)]), Sheet::Named(href)));
        }
        for other in self.session.documents().iter().filter(|d| d.id() != source) {
            choices.push((other.name().to_owned(), Sheet::Open(other.id())));
        }
        if choices.is_empty() {
            self.status = tl!("The document names no stylesheet, and no other document is open to use as one.").to_owned();
            return;
        }
        self.dialog =
            Some(Dialog::Transform(Box::new(XsltForm { source, name: doc.name().to_owned(), choices, chosen: 0, waiting: None, error: None })));
    }

    /// The text of the chosen stylesheet.
    fn sheet_text(&self, form: &XsltForm) -> Result<String, String> {
        let Some((_, sheet)) = form.choices.get(form.chosen) else { return Err(tl!("Choose a stylesheet.").to_owned()) };
        match sheet {
            Sheet::Open(id) => self
                .session
                .position_of(*id)
                .and_then(|i| self.session.document(i))
                .map(|d| d.editor().text())
                .ok_or_else(|| tl!("The stylesheet document is gone.").to_owned()),
            Sheet::Named(href) => {
                let base = self
                    .session
                    .position_of(form.source)
                    .and_then(|i| self.session.document(i))
                    .and_then(|d| d.path().map(std::path::Path::to_path_buf));
                let wanted = match base.as_deref().and_then(std::path::Path::parent) {
                    Some(dir) => dir.join(href),
                    None => std::path::PathBuf::from(href),
                };
                // An open document with that path first (what is being edited), then the file.
                if let Some(doc) = self.session.documents().iter().find(|d| d.path().is_some_and(|p| p == wanted)) {
                    return Ok(doc.editor().text());
                }
                let loaded = xmlcraft_platform::files::read_file(&wanted).map_err(|e| format!("{}: {e}", wanted.display()))?;
                let decoded = xmlcraft_text::encoding::decode(&loaded.bytes, &xmlcraft_text::encoding::sniff(&loaded.bytes));
                Ok(decoded.text)
            }
        }
    }

    fn run_transform(&mut self, form: &mut XsltForm) {
        let Some(doc) = self.session.position_of(form.source).and_then(|i| self.session.document(i)) else {
            form.error = Some(tl!("The document is gone.").to_owned());
            return;
        };
        let xml = doc.editor().text();
        let xslt = match self.sheet_text(form) {
            Ok(text) => text,
            Err(why) => {
                form.error = Some(why);
                return;
            }
        };
        if xml.len() > MAX_BYTES || xslt.len() > MAX_BYTES {
            form.error = Some(fmt(tl!("A document is larger than {limit} MB."), &[("limit", &(MAX_BYTES / (1024 * 1024)).to_string())]));
            return;
        }
        self.generate_serial += 1;
        let serial = self.generate_serial;
        self.xslt.submit(XsltJob { serial, xml, xslt });
        form.waiting = Some(serial);
        form.error = None;
    }

    /// Collect what the worker finished: a result opens as a new document, a failure is said in the window. Called every frame.
    pub(crate) fn pump_transform(&mut self) {
        for done in self.xslt.poll() {
            let Some(Dialog::Transform(form)) = &mut self.dialog else { continue };
            if form.waiting != Some(done.serial) {
                continue;
            }
            form.waiting = None;
            match done.result {
                Ok(made) => {
                    let name = result_name(&form.name, result_extension(&made.text));
                    self.dialog = None;
                    self.open_generated(&name, &made.text);
                    self.status = fmt(tl!("Transformed into {name}"), &[("name", &name)]);
                }
                Err(e) => form.error = Some(e.to_string()),
            }
        }
    }

    pub(crate) fn show_transform(&mut self, ctx: &egui::Context) {
        let Some(Dialog::Transform(mut form)) = self.dialog.take() else { return };
        let subtitle = fmt(tl!("Transform {name} into a new document; it stays as it is."), &[("name", &form.name)]);
        let (action, closed) =
            kit::dialog(ctx, "xmlcraft-transform", 560.0, "wand-sparkles", tl!("Transform with XSLT"), &subtitle, |ui| transform_form(ui, &mut form));
        match action {
            Some(Footer::Primary) => {
                self.run_transform(&mut form);
                self.dialog = Some(Dialog::Transform(form));
            }
            Some(Footer::Cancel) => {}
            _ if closed => {}
            _ => self.dialog = Some(Dialog::Transform(form)),
        }
    }
}

fn transform_form(ui: &mut egui::Ui, form: &mut XsltForm) -> Option<Footer> {
    kit::section(ui, tl!("Stylesheet"), |ui| {
        let current = form.choices.get(form.chosen).map(|(label, _)| label.clone()).unwrap_or_default();
        egui::ComboBox::from_id_salt("xmlcraft-transform-sheet").selected_text(current).width(ui.available_width() - 8.0).show_ui(ui, |ui| {
            for (i, (label, _)) in form.choices.iter().enumerate() {
                ui.selectable_value(&mut form.chosen, i, label);
            }
        });
        ui.add_space(4.0);
        kit::help(ui, tl!("The engine implements part of XSLT 3.0; what it does not implement is said, and nothing is changed."));
    });
    if form.waiting.is_some() {
        ui.add_space(kit::GAP_GROUP);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Transforming…"));
        });
    }
    if let Some(message) = &form.error {
        ui.add_space(kit::GAP_GROUP);
        kit::banner(ui, Tone::Bad, message);
    }
    let can = form.waiting.is_none();
    match kit::footer(ui, tl!("Cancel"), tl!("Transform"), Some("wand-sparkles"), can, "") {
        Footer::Nothing => None,
        other => Some(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_is_named_after_its_source_and_what_it_looks_like() {
        assert_eq!(result_name("order.xml", "html"), "order-result.html");
        assert_eq!(result_name("a.b.xml", "txt"), "a.b-result.txt");
        assert_eq!(result_name("Untitled-1", "xml"), "Untitled-1-result.xml");
        assert_eq!(result_extension("<!DOCTYPE html><html/>"), "html");
        assert_eq!(result_extension("  <r/>"), "xml");
        assert_eq!(result_extension("{\"a\": 1}"), "json");
        assert_eq!(result_extension("plain"), "txt");
    }
}
