//! HTTP & OpenAPI, the tool window that makes an HTTP request from an OpenAPI description (2.0, 3.0, 3.1 or 3.2, JSON or YAML): the
//! operations and servers of the description being shown, the request made for one with example values (method, address, headers and
//! body, all of it editable), Send, and the answer (see [`crate::http_pane`]). The description follows the document in front; nothing
//! is changed in it. Sending is a person's act (the Send button) and goes through the network the person allowed.

use xmlcraft_engine::rest::{Described, RestOperation, describe, prepare};

use crate::bar_actions::BarKind;
use crate::http_pane::{Pane, PaneView, Prepared};
use crate::i18n::{fmt, tl};
use crate::kit;
use crate::tools;
use crate::{Panel, XmlCraftApp, widgets};

/// What the description of a document came to, for the version of the text it was read from.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Read {
    doc: u64,
    version: u64,
    text: String,
    yaml: bool,
    result: Result<Operations, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Operations {
    ops: Vec<RestOperation>,
    servers: Vec<String>,
    warnings: Vec<String>,
}

/// The form of the HTTP & OpenAPI tool window and the request in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HttpTool {
    read: Option<Read>,
    /// The operation (by id) and the server (by place in the list) chosen.
    chosen: Option<String>,
    server: usize,
    /// The document, operation and server the fields were made for (a new choice makes them again).
    prepared_for: Option<(u64, String, usize)>,
    pub(crate) pane: Pane,
}

impl Default for HttpTool {
    fn default() -> Self {
        Self { read: None, chosen: None, server: 0, prepared_for: None, pane: Pane { method: "GET".to_owned(), ..Pane::default() } }
    }
}

impl HttpTool {
    /// The operation to make a request for: the one chosen when the description still has it, else its first.
    fn operation<'a>(&self, ops: &'a Operations) -> Option<&'a RestOperation> {
        self.chosen.as_ref().and_then(|id| ops.ops.iter().find(|o| &o.id == id)).or_else(|| ops.ops.first())
    }

    /// Make the request of the chosen operation to the chosen server (the work is on the text only: no files, no network).
    fn make_request(&mut self) {
        let Some(read) = &self.read else { return };
        let Ok(operations) = &read.result else { return };
        let Some(op) = self.operation(operations) else { return };
        let id = op.id.clone();
        self.prepared_for = Some((read.doc, id.clone(), self.server));
        let server = operations.servers.get(self.server).map(String::as_str);
        match prepare(&read.text, read.yaml, &id, server) {
            Ok(made) => {
                self.pane.error = None;
                self.pane.apply(Prepared {
                    method: Some(made.method),
                    url: Some(made.url),
                    headers: made.headers,
                    body: made.body,
                    warnings: operations.warnings.clone(),
                });
            }
            Err(why) => self.pane.error = Some(why),
        }
    }
}

impl XmlCraftApp {
    /// What the HTTP & OpenAPI tool holds, for tests.
    pub fn openapi_window(&self) -> Option<PaneView> {
        let ready = self.panel_visible(Panel::Http) && self.http_tool.prepared_for.is_some();
        ready.then(|| Self::pane_view(&self.http_tool.pane))
    }

    /// The method and the address of the request in the HTTP & OpenAPI tool, for tests.
    pub fn openapi_request(&self) -> Option<(String, String)> {
        let ready = self.panel_visible(Panel::Http) && self.http_tool.prepared_for.is_some();
        ready.then(|| (self.http_tool.pane.method.clone(), self.http_tool.pane.url.clone()))
    }

    /// Send OpenAPI Request…: show the HTTP & OpenAPI panel on the description being shown, and say in the status bar when the
    /// document is not one.
    pub(crate) fn open_openapi(&mut self) {
        let Some(doc) = self.session.active_document() else { return };
        let language = doc.language();
        let verdict = if language.is_json_like() || language.is_yaml() {
            match describe(&doc.editor().text(), language.is_yaml()) {
                Ok(found) if found.operations.is_empty() => Some(tl!("The description has no operation.").to_owned()),
                Ok(_) => None,
                Err(why) => Some(fmt(tl!("This is not an OpenAPI description: {why}"), &[("why", &why)])),
            }
        } else {
            Some(tl!("An OpenAPI description is a JSON or YAML document.").to_owned())
        };
        self.show_tool(Panel::Http);
        if let Some(message) = verdict {
            self.status = message;
        }
    }
}

/// Read the description of the document in front again when its text changed.
fn ensure_read(app: &mut XmlCraftApp) {
    let Some(doc) = app.session.active_document() else { return };
    let (id, version) = (doc.id(), doc.editor().version());
    if app.http_tool.read.as_ref().is_some_and(|r| r.doc == id && r.version == version) {
        return;
    }
    let (text, yaml) = (doc.editor().text(), doc.language().is_yaml());
    let result = match describe(&text, yaml) {
        Ok(Described { operations, servers, warnings }) if !operations.is_empty() => Ok(Operations { ops: operations, servers, warnings }),
        Ok(_) => Err(tl!("The description has no operation.").to_owned()),
        Err(why) => Err(fmt(tl!("This is not an OpenAPI description: {why}"), &[("why", &why)])),
    };
    app.http_tool.read = Some(Read { doc: id, version, text, yaml, result });
}

/// The body of the HTTP & OpenAPI panel.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    let bound = tools::bound(app);
    let is_description = app.active_bar_kind(ui.ctx()) == Some(BarKind::OpenApi);
    let Some(bound) = bound.filter(|_| is_description) else {
        kit::tool_body(ui, false, |ui| {
            if let Some(name) = tools::bound(app).map(|b| b.name) {
                tools::bound_line(ui, &name);
            }
            tools::needs(ui, tl!("HTTP & OpenAPI needs an OpenAPI description"), tl!("Show a JSON or YAML document that describes an API."));
        });
        return;
    };
    ensure_read(app);
    // A choice of another document, operation or server makes the request again.
    let tool = &mut app.http_tool;
    let wanted =
        tool.read.as_ref().and_then(|r| r.result.as_ref().ok().and_then(|ops| tool.operation(ops)).map(|op| (r.doc, op.id.clone(), tool.server)));
    if wanted.is_some() && tool.prepared_for != wanted {
        tool.make_request();
    }
    let can = app.http_tool.pane.can_send();
    let send = kit::tool_footer(ui, tl!("Send"), Some("send"), can, tl!("Give the address to send to."));
    let mut act = false;
    kit::tool_body(ui, true, |ui| {
        tools::bound_line(ui, &bound.name);
        ui.add_space(kit::GAP);
        let tool = &mut app.http_tool;
        match tool.read.as_ref().map(|r| &r.result) {
            Some(Ok(operations)) => {
                let operations = operations.clone();
                act = form(ui, tool, &operations);
            }
            Some(Err(message)) => {
                kit::banner(ui, kit::Tone::Bad, message);
            }
            None => {}
        }
        crate::http_pane::pane_ui(ui, &mut tool.pane, true);
    });
    if act {
        app.http_tool.make_request();
    }
    let name = bound.name.clone();
    let mut pane = std::mem::take(&mut app.http_tool.pane);
    app.do_pane_act(&mut pane, &name);
    if send {
        app.send_pane(&mut pane);
    }
    app.http_tool.pane = pane;
}

/// The operation and the server. True when "Make the request again" was clicked.
fn form(ui: &mut egui::Ui, tool: &mut HttpTool, operations: &Operations) -> bool {
    let t = crate::theme::Tokens::get(ui.ctx());
    let label = |ui: &mut egui::Ui, text: &str| ui.label(egui::RichText::new(text).font(crate::theme::medium(11.5)).color(t.text_muted));
    label(ui, tl!("Operation"));
    let current = tool.operation(operations).map(RestOperation::label).unwrap_or_default();
    let combo = egui::ComboBox::from_id_salt("xmlcraft-openapi-operation")
        .selected_text(current)
        .width((ui.available_width() - 8.0).max(120.0))
        .show_ui(ui, |ui| {
            for op in &operations.ops {
                let selected = tool.chosen.as_ref().is_some_and(|c| c == &op.id);
                if ui.selectable_label(selected, op.label()).clicked() {
                    tool.chosen = Some(op.id.clone());
                }
            }
        });
    widgets::name_for_assistive_technology(ui, &combo.response, tl!("The operation"));
    if operations.servers.len() > 1 {
        ui.add_space(4.0);
        label(ui, tl!("Server"));
        let current = operations.servers.get(tool.server).cloned().unwrap_or_default();
        egui::ComboBox::from_id_salt("xmlcraft-openapi-server").selected_text(current).width((ui.available_width() - 8.0).max(120.0)).show_ui(
            ui,
            |ui| {
                for (i, server) in operations.servers.iter().enumerate() {
                    ui.selectable_value(&mut tool.server, i, server);
                }
            },
        );
    }
    ui.add_space(4.0);
    ui.small_button(tl!("Make the request again")).on_hover_text(tl!("Replaces the request below with the example of the operation.")).clicked()
}
