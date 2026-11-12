//! SOAP, the tool window of requests and the debugger.
//!
//! **Request**: the operations of the WSDL being shown, the request made for one (address, headers and an envelope with a sample body,
//! all of it editable) and the answer to sending it (see [`crate::http_pane`]). Nothing is changed in the WSDL.
//!
//! **Debugger**: a proxy on this computer that a client is pointed at. Every request it forwards to the target, and the answer, is
//! listed; with breaking on, requests (or answers) are held, can be edited and are then released or dropped. It is started with Start
//! and stops with Stop or when the app ends: hiding the panel does not stop it, and the rail icon shows a dot while a message is held.
//! See `xmlcraft_platform::proxy` for what the proxy does and does not do (plain HTTP to the client, one request to a connection,
//! forwarding through the same client as the Request tab).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use xmlcraft_engine::soap::{OperationEntry, headers_text, operations, parse_headers, prepare};
use xmlcraft_engine::validation::OpenDocuments;
use xmlcraft_platform::proxy::{Breaks, Config, Debugger, Exchange, Message, Stage};
use xmlcraft_text::Buffer;

use crate::bar_actions::BarKind;
use crate::http_pane::{Outcome, Pane, PaneView, Prepared, pane_ui};
use crate::i18n::{fmt, tl};
use crate::kit::{self, Tone};
use crate::tools;
use crate::{Panel, XmlCraftApp, widgets};

/// Which part of the tool is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Tab {
    #[default]
    Request,
    Debugger,
}

/// An operation by what tells it apart in a WSDL: its name and its binding.
type OperationKey = (String, Option<String>);

/// What the operations of a document came to, for the version of the text they were read from.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Read {
    doc: u64,
    version: u64,
    text: String,
    location: String,
    result: Result<(Vec<OperationEntry>, Vec<String>), String>,
}

/// A running proxy, shared by the copies of the state; it stops when the last one goes (with the app).
#[derive(Clone)]
pub(crate) struct Running(Arc<Debugger>);

impl std::fmt::Debug for Running {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Running({})", self.0.address())
    }
}

impl PartialEq for Running {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Running {}

/// What the tool was asked to do besides the proxy's own buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Release,
    Drop,
    OpenRequest,
    OpenResponse,
}

/// The Debugger tab: the form, the running proxy and the message being looked at.
#[derive(Clone, Debug)]
struct ProxyState {
    port: String,
    target: String,
    allow_private: bool,
    running: Option<Running>,
    /// Set by the proxy's thread whenever an exchange changes.
    changed: Arc<AtomicBool>,
    /// A message is held now (as of the last change).
    held: bool,
    error: Option<String>,
    break_requests: bool,
    break_responses: bool,
    contains: String,
    selected: Option<u64>,
    /// The held message being edited, and which one it was loaded from.
    edit_for: Option<(u64, Stage)>,
    edit_headers: String,
    edit_body: String,
    act: Option<Act>,
    start: bool,
    stop: bool,
}

impl Default for ProxyState {
    fn default() -> Self {
        Self {
            port: "8888".to_owned(),
            target: String::new(),
            allow_private: false,
            running: None,
            changed: Arc::new(AtomicBool::new(false)),
            held: false,
            error: None,
            break_requests: false,
            break_responses: false,
            contains: String::new(),
            selected: None,
            edit_for: None,
            edit_headers: String::new(),
            edit_body: String::new(),
            act: None,
            start: false,
            stop: false,
        }
    }
}

/// The SOAP tool window.
#[derive(Clone, Debug, Default)]
pub(crate) struct SoapTool {
    tab: Tab,
    read: Option<Read>,
    chosen: Option<OperationKey>,
    /// The document and operation the fields were made for (a new choice makes them again).
    prepared_for: Option<(u64, OperationKey)>,
    pub(crate) pane: Pane,
    proxy: ProxyState,
}

/// What the Debugger tab shows, for tests: where the proxy listens and its exchanges (id, stage, path).
pub type ProxyView = (Option<String>, Vec<(u64, String, String)>);

fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::HeldRequest => "request held",
        Stage::Forwarding => "forwarding",
        Stage::HeldResponse => "answer held",
        Stage::Done => "done",
        Stage::Dropped => "dropped",
        Stage::Failed => "failed",
    }
}

fn message_text(message: &Message) -> String {
    format!("{}\n\n{}", headers_text(&message.headers), String::from_utf8_lossy(&message.body))
}

fn key_of(op: &OperationEntry) -> OperationKey {
    (op.name.clone(), op.binding.clone())
}

impl SoapTool {
    /// The operation to make a request for: the one chosen when the WSDL still has it, else its first.
    fn operation<'a>(&self, ops: &'a [OperationEntry]) -> Option<&'a OperationEntry> {
        self.chosen.as_ref().and_then(|key| ops.iter().find(|o| &key_of(o) == key)).or_else(|| ops.first())
    }

    /// Whether the proxy is listening.
    pub(crate) fn proxy_running(&self) -> bool {
        self.proxy.running.is_some()
    }
}

impl XmlCraftApp {
    /// What the SOAP request holds, for tests.
    pub fn soap_window(&self) -> Option<PaneView> {
        let ready = self.panel_visible(Panel::Soap) && self.soap_tool.prepared_for.is_some();
        ready.then(|| Self::pane_view(&self.soap_tool.pane))
    }

    /// What the Debugger tab shows, for tests: `Some` while the panel is shown or the proxy is listening.
    pub fn soap_debugger(&self) -> Option<ProxyView> {
        let proxy = &self.soap_tool.proxy;
        if !self.panel_visible(Panel::Soap) && proxy.running.is_none() {
            return None;
        }
        let rows = proxy.running.iter().flat_map(|r| r.0.exchanges()).map(|e| (e.id, stage_name(e.stage).to_owned(), e.path)).collect();
        Some((proxy.running.as_ref().map(|r| r.0.address().to_string()), rows))
    }

    /// Set the fields of the Debugger tab and press Start, for tests: the target, whether it may be on this computer, and whether
    /// requests are held.
    pub fn soap_debugger_start(&mut self, port: &str, target: &str, allow_private: bool, break_requests: bool) {
        let proxy = &mut self.soap_tool.proxy;
        proxy.port = port.to_owned();
        proxy.target = target.to_owned();
        proxy.allow_private = allow_private;
        proxy.break_requests = break_requests;
        proxy.start = true;
    }

    /// Send SOAP Request…: show the SOAP panel on its Request tab, and say in the status bar when the document is not a WSDL.
    pub(crate) fn open_soap(&mut self) {
        let Some(doc) = self.session.active_document() else { return };
        let verdict = if doc.language().is_xml() {
            match operations(&doc.editor().text()) {
                Ok((ops, _)) if ops.is_empty() => Some(tl!("The WSDL has no operation with a SOAP binding.").to_owned()),
                Ok(_) => None,
                Err(why) => Some(fmt(tl!("This is not a WSDL: {why}"), &[("why", &why)])),
            }
        } else {
            Some(tl!("A SOAP request is made from a WSDL document.").to_owned())
        };
        if self.show_tool(Panel::Soap) {
            self.soap_tool.tab = Tab::Request;
        }
        if let Some(message) = verdict {
            self.status = message;
        }
    }

    /// SOAP Debugger…: show the SOAP panel on its Debugger tab.
    pub(crate) fn open_proxy(&mut self) {
        if self.show_tool(Panel::Soap) {
            self.soap_tool.tab = Tab::Debugger;
        }
    }

    /// Make the request of the chosen operation (on a thread: the schemas it imports are read, which may take a moment).
    fn prepare_soap(&mut self) {
        let Some(read) = self.soap_tool.read.clone() else { return };
        let Ok((ops, _)) = &read.result else { return };
        let Some(op) = self.soap_tool.operation(ops).cloned() else { return };
        let serial = self.next_http_serial();
        let (text, location, network) = (read.text.clone(), read.location.clone(), self.network.clone());
        let open: Vec<(String, Buffer)> = self.open_documents();
        let warnings = match &read.result {
            Ok((_, warnings)) => warnings.clone(),
            Err(_) => Vec::new(),
        };
        let (op_name, op_binding) = (op.name.clone(), op.binding.clone());
        let started = self.spawn_http(move || {
            let resolver = network.resolver(Arc::new(OpenDocuments::new(open)));
            let result = xmlcraft_engine::guard(|| prepare(&text, &location, &op_name, op_binding.as_deref(), &resolver))
                .unwrap_or_else(|_| Err("making the request failed (a bug); nothing was changed".to_owned()))
                .map(|made| {
                    let mut all = warnings;
                    all.extend(made.warnings);
                    Prepared { method: Some("POST".to_owned()), url: made.url, headers: made.headers, body: made.body, warnings: all }
                });
            Outcome::Prepared { serial, result }
        });
        let pane = &mut self.soap_tool.pane;
        match started {
            Ok(()) => {
                pane.preparing = Some(serial);
                self.soap_tool.prepared_for = Some((read.doc, key_of(&op)));
                pane.error = None;
            }
            Err(why) => pane.error = Some(why),
        }
    }

    fn start_proxy(&mut self) {
        let proxy = &mut self.soap_tool.proxy;
        let Ok(port) = proxy.port.trim().parse::<u16>() else {
            proxy.error = Some(tl!("The port is a number from 0 to 65535.").to_owned());
            return;
        };
        let config =
            Config { port, target: proxy.target.trim().to_owned(), allow_private_hosts: proxy.allow_private, timeout: Duration::from_secs(60) };
        match Debugger::start(config) {
            Ok(debugger) => {
                let (ctx, changed) = (self.ctx.clone(), proxy.changed.clone());
                debugger.on_change(move || {
                    changed.store(true, Ordering::SeqCst);
                    if let Some(ctx) = &ctx {
                        ctx.request_repaint();
                    }
                });
                proxy.running = Some(Running(Arc::new(debugger)));
                proxy.error = None;
                proxy.changed.store(true, Ordering::SeqCst);
            }
            Err(why) => proxy.error = Some(why),
        }
    }

    /// What the proxy needs at every frame, whether or not its panel is shown: the breaks of the form, a press of Start or Stop, the
    /// attention dot while a message is held. Called every frame.
    pub(crate) fn tick_proxy(&mut self) {
        if std::mem::take(&mut self.soap_tool.proxy.start) {
            self.start_proxy();
        }
        let proxy = &mut self.soap_tool.proxy;
        if std::mem::take(&mut proxy.stop) {
            proxy.running = None;
            proxy.selected = None;
            proxy.edit_for = None;
            proxy.held = false;
        }
        let Some(running) = proxy.running.clone() else { return };
        running.0.set_breaks(Breaks { requests: proxy.break_requests, responses: proxy.break_responses, contains: proxy.contains.clone() });
        if proxy.changed.swap(false, Ordering::SeqCst) {
            proxy.held = running.0.exchanges().iter().any(|e| matches!(e.stage, Stage::HeldRequest | Stage::HeldResponse));
        }
        if proxy.held {
            self.workbench.set_attention(Panel::Soap);
        }
        self.proxy_act();
    }

    /// Release or drop the held message, or open a request or an answer as a document.
    fn proxy_act(&mut self) {
        let proxy = &mut self.soap_tool.proxy;
        let (Some(act), Some(running), Some(id)) = (proxy.act.take(), proxy.running.clone(), proxy.selected) else { return };
        let exchange: Option<Exchange> = running.0.exchanges().into_iter().find(|e| e.id == id);
        match act {
            Act::Release => match parse_headers(&proxy.edit_headers) {
                Ok(headers) => {
                    running.0.release(id, Some(Message { headers, body: proxy.edit_body.clone().into_bytes() }));
                    proxy.edit_for = None;
                    proxy.error = None;
                    proxy.changed.store(true, Ordering::SeqCst);
                }
                Err(why) => proxy.error = Some(why),
            },
            Act::Drop => {
                running.0.drop_held(id);
                proxy.edit_for = None;
                proxy.changed.store(true, Ordering::SeqCst);
            }
            Act::OpenRequest => {
                if let Some(e) = exchange {
                    let text = String::from_utf8_lossy(&e.request.body).into_owned();
                    let name = crate::http_pane::document_name("debugger", &format!("request-{id}"), crate::http_pane::answer_extension(&text));
                    self.open_generated(&name, &text);
                }
            }
            Act::OpenResponse => {
                if let Some(e) = exchange {
                    let text = String::from_utf8_lossy(&e.response.body).into_owned();
                    let name = crate::http_pane::document_name("debugger", &format!("response-{id}"), crate::http_pane::answer_extension(&text));
                    self.open_generated(&name, &text);
                }
            }
        }
    }
}

/// Read the operations of the document in front again when its text changed.
fn ensure_read(app: &mut XmlCraftApp) {
    let Some(doc) = app.session.active_document() else { return };
    let (id, version) = (doc.id(), doc.editor().version());
    if app.soap_tool.read.as_ref().is_some_and(|r| r.doc == id && r.version == version) {
        return;
    }
    let text = doc.editor().text();
    let location = doc.path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let result = match operations(&text) {
        Ok((ops, _)) if ops.is_empty() => Err(tl!("The WSDL has no operation with a SOAP binding.").to_owned()),
        Ok(found) => Ok(found),
        Err(why) => Err(fmt(tl!("This is not a WSDL: {why}"), &[("why", &why)])),
    };
    app.soap_tool.read = Some(Read { doc: id, version, text, location, result });
}

/// The body of the SOAP panel.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    match app.soap_tool.tab {
        Tab::Request => request(ui, app),
        Tab::Debugger => debugger(ui, app),
    }
}

fn tabs(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    kit::segmented(ui, &mut app.soap_tool.tab, &[(Tab::Request, tl!("Request")), (Tab::Debugger, tl!("Debugger"))]);
    ui.add_space(kit::GAP);
}

fn caption(ui: &mut egui::Ui, text: &str) {
    let t = crate::theme::Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(text).font(crate::theme::medium(11.5)).color(t.text_muted));
}

fn request(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    let bound = tools::bound(app);
    let is_wsdl = app.active_bar_kind(ui.ctx()) == Some(BarKind::Wsdl);
    let Some(bound) = bound.filter(|_| is_wsdl) else {
        let shown = tools::bound(app).map(|b| b.name);
        let can = false;
        let _ = kit::tool_footer(ui, tl!("Send"), Some("send"), can, tl!("Show a WSDL document first."));
        kit::tool_body(ui, false, |ui| {
            tabs(ui, app);
            if let Some(name) = &shown {
                tools::bound_line(ui, name);
            }
            tools::needs(ui, tl!("A SOAP request needs a WSDL document"), tl!("Show a WSDL to make a request from one of its operations."));
        });
        return;
    };
    ensure_read(app);
    // A choice of another document or operation makes the request again.
    let wanted = app.soap_tool.read.as_ref().and_then(|r| {
        let (ops, _) = r.result.as_ref().ok()?;
        Some((r.doc, key_of(app.soap_tool.operation(ops)?)))
    });
    if wanted.is_some() && app.soap_tool.prepared_for != wanted && app.soap_tool.pane.preparing.is_none() {
        app.prepare_soap();
    }
    let can = app.soap_tool.pane.can_send();
    let send = kit::tool_footer(ui, tl!("Send"), Some("send"), can, tl!("Wait for the request to be made."));
    let mut again = false;
    kit::tool_body(ui, true, |ui| {
        tabs(ui, app);
        tools::bound_line(ui, &bound.name);
        ui.add_space(kit::GAP);
        let tool = &mut app.soap_tool;
        match tool.read.clone().as_ref().map(|r| &r.result) {
            Some(Ok((ops, _))) => {
                caption(ui, tl!("Operation"));
                let current = tool.operation(ops).map(OperationEntry::label).unwrap_or_default();
                let combo = egui::ComboBox::from_id_salt("xmlcraft-soap-operation")
                    .selected_text(current)
                    .width((ui.available_width() - 8.0).max(120.0))
                    .show_ui(ui, |ui| {
                        for op in ops {
                            let selected = tool.chosen.as_ref().is_some_and(|c| c == &key_of(op));
                            if ui.selectable_label(selected, op.label()).clicked() {
                                tool.chosen = Some(key_of(op));
                            }
                        }
                    });
                widgets::name_for_assistive_technology(ui, &combo.response, tl!("The operation"));
                if tool.pane.preparing.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(tl!("Making the request…"));
                    });
                } else {
                    again = ui
                        .small_button(tl!("Make the request again"))
                        .on_hover_text(tl!("Replaces the request below with the example of the operation."))
                        .clicked();
                }
            }
            Some(Err(message)) => {
                kit::banner(ui, Tone::Bad, message);
            }
            None => {}
        }
        pane_ui(ui, &mut tool.pane, false);
    });
    if again {
        app.soap_tool.prepared_for = None;
    }
    let name = bound.name.clone();
    let mut pane = std::mem::take(&mut app.soap_tool.pane);
    app.do_pane_act(&mut pane, &name);
    if send {
        app.send_pane(&mut pane);
    }
    app.soap_tool.pane = pane;
}

/// The Debugger tab.
fn debugger(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    let running = app.soap_tool.proxy.running.is_some();
    let go = kit::tool_footer(ui, if running { tl!("Stop") } else { tl!("Start") }, Some(if running { "x" } else { "send" }), true, "");
    if go {
        let proxy = &mut app.soap_tool.proxy;
        if running {
            proxy.stop = true;
        } else {
            proxy.start = true;
        }
    }
    let exchanges: Vec<Exchange> = app.soap_tool.proxy.running.as_ref().map(|r| r.0.exchanges()).unwrap_or_default();
    kit::tool_body(ui, false, |ui| {
        tabs(ui, app);
        let wide = ui.available_width() >= tools::TWO_COLUMNS;
        tools::columns(ui, &mut app.soap_tool.proxy, |ui, proxy| proxy_form(ui, proxy, &exchanges, wide), |ui, proxy| details(ui, proxy, &exchanges));
    });
}

fn proxy_form(ui: &mut egui::Ui, proxy: &mut ProxyState, exchanges: &[Exchange], wide: bool) {
    let running = proxy.running.is_some();
    caption(ui, tl!("Proxy"));
    ui.horizontal(|ui| {
        let port = ui.add_enabled(!running, egui::TextEdit::singleline(&mut proxy.port).desired_width(60.0).hint_text("8888"));
        widgets::name_for_assistive_technology(ui, &port, tl!("The port to listen on"));
        let target = ui.add_enabled(
            !running,
            egui::TextEdit::singleline(&mut proxy.target)
                .desired_width((ui.available_width() - 8.0).max(80.0))
                .hint_text("https://example.com/service"),
        );
        widgets::name_for_assistive_technology(ui, &target, tl!("The target to forward to"));
    });
    ui.add_enabled(!running, widgets::checkbox(&mut proxy.allow_private, tl!("Allow this computer and the local network as the target")));
    if let Some(running) = &proxy.running {
        let text = fmt(
            tl!("Listening on http://{address}. Point the client at it; requests go to the target with the path they ask for."),
            &[("address", &running.0.address().to_string())],
        );
        kit::banner(ui, Tone::Good, &text);
    } else {
        kit::help(ui, tl!("The proxy listens on this computer only, until you stop it or quit. The client talks plain HTTP to it."));
    }
    if let Some(message) = &proxy.error {
        kit::banner(ui, Tone::Bad, message);
    }
    ui.add_space(kit::GAP);
    caption(ui, tl!("Break"));
    ui.horizontal(|ui| {
        ui.add(widgets::checkbox(&mut proxy.break_requests, tl!("Hold requests")));
        ui.add(widgets::checkbox(&mut proxy.break_responses, tl!("Hold answers")));
    });
    let contains =
        ui.add(egui::TextEdit::singleline(&mut proxy.contains).desired_width(f32::INFINITY).hint_text(tl!("only messages that contain this text")));
    widgets::name_for_assistive_technology(ui, &contains, tl!("Hold only messages that contain"));
    ui.add_space(kit::GAP);
    caption(ui, tl!("Messages"));
    if exchanges.is_empty() {
        kit::help(ui, tl!("Nothing has come through yet."));
        return;
    }
    // In two columns the list takes the rest of the column; stacked, about two fifths of what is left (the details need room too).
    let height = if wide { ui.available_height() } else { ui.available_height() * 0.4 };
    egui::ScrollArea::vertical().id_salt("xmlcraft-proxy-list").max_height(height).auto_shrink([false, false]).show(ui, |ui| {
        for e in exchanges.iter().rev() {
            let status = e.status.as_ref().map(|(code, _)| format!(" {code}")).unwrap_or_default();
            let label = format!("#{}  {} {}  ·  {}{status}", e.id, e.method, e.path, stage_name(e.stage));
            if ui.selectable_label(proxy.selected == Some(e.id), label).clicked() {
                proxy.selected = Some(e.id);
                proxy.edit_for = None;
            }
        }
    });
}

/// The message chosen in the list: the held one to edit, or the request and the answer to read.
fn details(ui: &mut egui::Ui, proxy: &mut ProxyState, exchanges: &[Exchange]) {
    ui.add_space(kit::GAP);
    let Some(e) = proxy.selected.and_then(|id| exchanges.iter().find(|e| e.id == id)) else {
        kit::help(ui, tl!("Choose a message in the list to read it. A held one can be edited, then released or dropped."));
        return;
    };
    // Each of the (up to two) text areas gets about a third of the height, so that the buttons stay in view.
    let line = 15.0;
    let available = ui.available_height();
    let rows = |share: f32| ((available * share / line) as usize).clamp(3, 60);
    egui::ScrollArea::vertical().id_salt("xmlcraft-proxy-details").auto_shrink([false, false]).show(ui, |ui| {
        if matches!(e.stage, Stage::HeldRequest | Stage::HeldResponse) {
            if proxy.edit_for != Some((e.id, e.stage)) {
                let message = if e.stage == Stage::HeldRequest { &e.request } else { &e.response };
                proxy.edit_headers = headers_text(&message.headers);
                proxy.edit_body = String::from_utf8_lossy(&message.body).into_owned();
                proxy.edit_for = Some((e.id, e.stage));
            }
            caption(
                ui,
                if e.stage == Stage::HeldRequest { tl!("Held request (edit it, then release)") } else { tl!("Held answer (edit it, then release)") },
            );
            ui.add(egui::TextEdit::multiline(&mut proxy.edit_headers).font(crate::theme::mono(11.5)).desired_rows(3).desired_width(f32::INFINITY));
            let body_rows = rows(0.45);
            ui.add(
                egui::TextEdit::multiline(&mut proxy.edit_body).font(crate::theme::mono(12.0)).desired_rows(body_rows).desired_width(f32::INFINITY),
            );
            ui.horizontal(|ui| {
                if ui.button(tl!("Release")).clicked() {
                    proxy.act = Some(Act::Release);
                }
                if ui.button(tl!("Drop")).clicked() {
                    proxy.act = Some(Act::Drop);
                }
            });
        } else {
            caption(ui, tl!("Request"));
            let text = message_text(&e.request);
            let mut shown = text.as_str();
            ui.add(egui::TextEdit::multiline(&mut shown).font(crate::theme::mono(11.5)).desired_rows(rows(0.3)).desired_width(f32::INFINITY));
            if ui.small_button(tl!("Open the request as a document")).clicked() {
                proxy.act = Some(Act::OpenRequest);
            }
            ui.add_space(kit::GAP);
            caption(ui, tl!("Answer"));
            if let Some(why) = &e.error {
                kit::banner(ui, Tone::Bad, why);
            } else if e.status.is_some() {
                let text = message_text(&e.response);
                let mut shown = text.as_str();
                ui.add(egui::TextEdit::multiline(&mut shown).font(crate::theme::mono(11.5)).desired_rows(rows(0.3)).desired_width(f32::INFINITY));
                if ui.small_button(tl!("Open the answer as a document")).clicked() {
                    proxy.act = Some(Act::OpenResponse);
                }
            }
        }
    });
}
