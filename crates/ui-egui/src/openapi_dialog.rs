//! The OpenAPI window: the operations and servers of the OpenAPI description being shown (2.0, 3.0, 3.1 or 3.2, JSON or YAML), the
//! request made for one with example values (method, address, headers and body, all of it editable) and the answer to sending it (see
//! [`crate::http_pane`]). Nothing is changed in the description.

use xmlcraft_engine::rest::{Described, RestOperation, describe, prepare};

use crate::XmlCraftApp;
use crate::dialog::Dialog;
use crate::http_pane::{Pane, PaneView, Prepared, pane_ui};
use crate::i18n::{fmt, tl};
use crate::kit::{self, Footer};

/// The OpenAPI window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OpenApiForm {
    source: u64,
    name: String,
    text: String,
    yaml: bool,
    ops: Vec<RestOperation>,
    servers: Vec<String>,
    chosen: usize,
    server: usize,
    /// The operation and server the fields were made for (a new choice makes them again).
    prepared_for: Option<(usize, usize)>,
    pub(crate) pane: Pane,
}

impl XmlCraftApp {
    /// What the OpenAPI window holds, for tests.
    pub fn openapi_window(&self) -> Option<PaneView> {
        let Some(Dialog::OpenApi(form)) = &self.dialog else { return None };
        Some(Self::pane_view(&form.pane))
    }

    /// The method and the address of the request in the OpenAPI window, for tests.
    pub fn openapi_request(&self) -> Option<(String, String)> {
        let Some(Dialog::OpenApi(form)) = &self.dialog else { return None };
        Some((form.pane.method.clone(), form.pane.url.clone()))
    }

    /// Open the OpenAPI window on the description being shown (Send OpenAPI Request…).
    pub(crate) fn open_openapi(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        let Some(doc) = self.session.active_document() else { return };
        let language = doc.language();
        if !(language.is_json_like() || language.is_yaml()) {
            self.status = tl!("An OpenAPI description is a JSON or YAML document.").to_owned();
            return;
        }
        let (text, yaml) = (doc.editor().text(), language.is_yaml());
        let Described { operations: ops, servers, warnings } = match describe(&text, yaml) {
            Ok(found) => found,
            Err(why) => {
                self.status = fmt(tl!("This is not an OpenAPI description: {why}"), &[("why", &why)]);
                return;
            }
        };
        if ops.is_empty() {
            self.status = tl!("The description has no operation.").to_owned();
            return;
        }
        let pane = Pane { method: "GET".to_owned(), warnings, ..Pane::default() };
        let mut form = Box::new(OpenApiForm {
            source: doc.id(),
            name: doc.name().to_owned(),
            text,
            yaml,
            ops,
            servers,
            chosen: 0,
            server: 0,
            prepared_for: None,
            pane,
        });
        make_request(&mut form);
        self.dialog = Some(Dialog::OpenApi(form));
    }

    pub(crate) fn show_openapi(&mut self, ctx: &egui::Context) {
        let Some(Dialog::OpenApi(mut form)) = self.dialog.take() else { return };
        let subtitle = fmt(tl!("Make an HTTP request from {name}, edit it and send it."), &[("name", &form.name)]);
        let (action, closed) =
            kit::dialog(ctx, "xmlcraft-openapi", 760.0, "cloud", tl!("Send OpenAPI Request"), &subtitle, |ui| openapi_form(ui, &mut form));
        // The choice of another operation or server makes the request again.
        if form.prepared_for != Some((form.chosen, form.server)) {
            make_request(&mut form);
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
                self.dialog = Some(Dialog::OpenApi(form));
            }
            Some(Footer::Cancel) => {}
            _ if closed => {}
            _ => self.dialog = Some(Dialog::OpenApi(form)),
        }
    }
}

/// Make the request of the chosen operation to the chosen server (the work is on the text only: no files, no network).
fn make_request(form: &mut OpenApiForm) {
    form.prepared_for = Some((form.chosen, form.server));
    let Some(op) = form.ops.get(form.chosen) else { return };
    let server = form.servers.get(form.server).map(String::as_str);
    match prepare(&form.text, form.yaml, &op.id, server) {
        Ok(made) => {
            form.pane.error = None;
            form.pane.apply(Prepared {
                method: Some(made.method),
                url: Some(made.url),
                headers: made.headers,
                body: made.body,
                warnings: Vec::new(),
            });
        }
        Err(why) => form.pane.error = Some(why),
    }
}

fn openapi_form(ui: &mut egui::Ui, form: &mut OpenApiForm) -> Option<Footer> {
    kit::section(ui, tl!("Operation"), |ui| {
        let current = form.ops.get(form.chosen).map(RestOperation::label).unwrap_or_default();
        egui::ComboBox::from_id_salt("xmlcraft-openapi-operation").selected_text(current).width(ui.available_width() - 8.0).show_ui(ui, |ui| {
            for (i, op) in form.ops.iter().enumerate() {
                ui.selectable_value(&mut form.chosen, i, op.label());
            }
        });
        if form.servers.len() > 1 {
            let current = form.servers.get(form.server).cloned().unwrap_or_default();
            egui::ComboBox::from_id_salt("xmlcraft-openapi-server").selected_text(current).width(ui.available_width() - 8.0).show_ui(ui, |ui| {
                for (i, server) in form.servers.iter().enumerate() {
                    ui.selectable_value(&mut form.server, i, server);
                }
            });
        }
    });
    pane_ui(ui, &mut form.pane, true);
    match kit::footer(ui, tl!("Close"), tl!("Send"), Some("cloud"), form.pane.can_send(), tl!("Give the address to send to.")) {
        Footer::Nothing => None,
        other => Some(other),
    }
}
