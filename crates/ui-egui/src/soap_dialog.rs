//! The SOAP window: the operations of the WSDL being shown, the request made for one (address, headers and an envelope with a sample
//! body, all of it editable) and the answer to sending it (see [`crate::http_pane`]). Nothing is changed in the WSDL.

use std::sync::Arc;

use xmlcraft_engine::soap::{OperationEntry, operations, prepare};
use xmlcraft_engine::validation::OpenDocuments;
use xmlcraft_text::Buffer;

use crate::XmlCraftApp;
use crate::dialog::Dialog;
use crate::http_pane::{Outcome, Pane, PaneView, Prepared, pane_ui};
use crate::i18n::{fmt, tl};
use crate::kit::{self, Footer};

/// The SOAP window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SoapForm {
    source: u64,
    name: String,
    text: String,
    location: String,
    ops: Vec<OperationEntry>,
    chosen: usize,
    /// The operation the fields were made for (a new choice makes them again).
    prepared_for: Option<usize>,
    pub(crate) pane: Pane,
}

impl XmlCraftApp {
    /// What the SOAP window holds, for tests.
    pub fn soap_window(&self) -> Option<PaneView> {
        let Some(Dialog::Soap(form)) = &self.dialog else { return None };
        Some(Self::pane_view(&form.pane))
    }

    /// Open the SOAP window on the WSDL being shown (Send SOAP Request…).
    pub(crate) fn open_soap(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        let Some(doc) = self.session.active_document() else { return };
        if !doc.language().is_xml() {
            self.status = tl!("A SOAP request is made from a WSDL document.").to_owned();
            return;
        }
        let text = doc.editor().text();
        let (ops, warnings) = match operations(&text) {
            Ok(found) => found,
            Err(why) => {
                self.status = fmt(tl!("This is not a WSDL: {why}"), &[("why", &why)]);
                return;
            }
        };
        if ops.is_empty() {
            self.status = tl!("The WSDL has no operation with a SOAP binding.").to_owned();
            return;
        }
        let location = doc.path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let pane = Pane { method: "POST".to_owned(), warnings, ..Pane::default() };
        let mut form = Box::new(SoapForm { source: doc.id(), name: doc.name().to_owned(), text, location, ops, chosen: 0, prepared_for: None, pane });
        self.prepare_soap(&mut form);
        self.dialog = Some(Dialog::Soap(form));
    }

    /// Make the request of the chosen operation (on a thread: the schemas it imports are read, which may take a moment).
    fn prepare_soap(&mut self, form: &mut SoapForm) {
        let Some(op) = form.ops.get(form.chosen).cloned() else { return };
        let serial = self.next_http_serial();
        let (text, location, network) = (form.text.clone(), form.location.clone(), self.network.clone());
        let open: Vec<(String, Buffer)> = self.open_documents();
        let started = self.spawn_http(move || {
            let resolver = network.resolver(Arc::new(OpenDocuments::new(open)));
            let result = xmlcraft_engine::guard(|| prepare(&text, &location, &op.name, op.binding.as_deref(), &resolver))
                .unwrap_or_else(|_| Err("making the request failed (a bug); nothing was changed".to_owned()))
                .map(|made| Prepared {
                    method: Some("POST".to_owned()),
                    url: made.url,
                    headers: made.headers,
                    body: made.body,
                    warnings: made.warnings,
                });
            Outcome::Prepared { serial, result }
        });
        match started {
            Ok(()) => {
                form.pane.preparing = Some(serial);
                form.prepared_for = Some(form.chosen);
                form.pane.error = None;
            }
            Err(why) => form.pane.error = Some(why),
        }
    }

    pub(crate) fn show_soap(&mut self, ctx: &egui::Context) {
        let Some(Dialog::Soap(mut form)) = self.dialog.take() else { return };
        let subtitle = fmt(tl!("Make a SOAP request from {name}, edit it and send it."), &[("name", &form.name)]);
        let (action, closed) = kit::dialog(ctx, "xmlcraft-soap", 760.0, "cloud", tl!("Send SOAP Request"), &subtitle, |ui| soap_form(ui, &mut form));
        // The choice of another operation makes the request again.
        if form.prepared_for != Some(form.chosen) && form.pane.preparing.is_none() {
            self.prepare_soap(&mut form);
        }
        let name = form.name.clone();
        let mut pane = std::mem::take(&mut form.pane);
        self.do_pane_act(&mut pane, &name);
        form.pane = pane;
        match action {
            Some(Footer::Primary) => {
                let mut pane = std::mem::take(&mut form.pane);
                self.send_pane(&mut pane);
                form.pane = pane;
                self.dialog = Some(Dialog::Soap(form));
            }
            Some(Footer::Cancel) => {}
            _ if closed => {}
            _ => self.dialog = Some(Dialog::Soap(form)),
        }
    }
}

fn soap_form(ui: &mut egui::Ui, form: &mut SoapForm) -> Option<Footer> {
    kit::section(ui, tl!("Operation"), |ui| {
        let current = form.ops.get(form.chosen).map(OperationEntry::label).unwrap_or_default();
        egui::ComboBox::from_id_salt("xmlcraft-soap-operation").selected_text(current).width(ui.available_width() - 8.0).show_ui(ui, |ui| {
            for (i, op) in form.ops.iter().enumerate() {
                ui.selectable_value(&mut form.chosen, i, op.label());
            }
        });
        if form.pane.preparing.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tl!("Making the request…"));
            });
        }
    });
    pane_ui(ui, &mut form.pane, false);
    match kit::footer(ui, tl!("Close"), tl!("Send"), Some("cloud"), form.pane.can_send(), tl!("Wait for the request to be made.")) {
        Footer::Nothing => None,
        other => Some(other),
    }
}
