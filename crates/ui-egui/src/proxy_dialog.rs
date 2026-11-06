//! The SOAP Debugger window: a proxy on this computer that a client is pointed at. Every request it forwards to the target, and the
//! answer, is listed; with breaking on, requests (or answers) are held, can be edited and are then released or dropped. It is started
//! with Start and stops when the window is closed: nothing keeps running after it. See `xmlcraft_platform::proxy` for what the proxy
//! does and does not do (plain HTTP to the client, one request to a connection, forwarding through the same client as the SOAP window).

use std::sync::Arc;
use std::time::Duration;

use xmlcraft_engine::soap::{headers_text, parse_headers};
use xmlcraft_platform::proxy::{Breaks, Config, Debugger, Exchange, Message, Stage};

use crate::XmlCraftApp;
use crate::dialog::Dialog;
use crate::i18n::{fmt, tl};
use crate::kit::{self, Tone};

/// A running proxy, shared by the window's copies (the dialog value is cloned); it stops when the last one goes.
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

/// What the window was asked to do besides the proxy's own buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    Release,
    Drop,
    OpenRequest,
    OpenResponse,
}

/// The SOAP Debugger window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProxyForm {
    port: String,
    target: String,
    allow_private: bool,
    running: Option<Running>,
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

/// What the window shows, for tests: where the proxy listens and its exchanges (id, stage, path).
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

impl XmlCraftApp {
    /// Open the SOAP Debugger window (XML ▸ SOAP Debugger…).
    pub(crate) fn open_proxy(&mut self) {
        if self.dialog.is_some() {
            return;
        }
        self.dialog = Some(Dialog::Proxy(Box::new(ProxyForm {
            port: "8888".to_owned(),
            target: String::new(),
            allow_private: false,
            running: None,
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
        })));
    }

    /// What the SOAP Debugger window shows, for tests.
    pub fn soap_debugger(&self) -> Option<ProxyView> {
        let Some(Dialog::Proxy(form)) = &self.dialog else { return None };
        let rows = form.running.iter().flat_map(|r| r.0.exchanges()).map(|e| (e.id, stage_name(e.stage).to_owned(), e.path)).collect();
        Some((form.running.as_ref().map(|r| r.0.address().to_string()), rows))
    }

    /// Set the fields of the SOAP Debugger window and press Start, for tests: the target, whether it may be on this computer, and whether
    /// requests are held.
    pub fn soap_debugger_start(&mut self, port: &str, target: &str, allow_private: bool, break_requests: bool) {
        let Some(Dialog::Proxy(form)) = &mut self.dialog else { return };
        form.port = port.to_owned();
        form.target = target.to_owned();
        form.allow_private = allow_private;
        form.break_requests = break_requests;
        form.start = true;
    }

    pub(crate) fn show_proxy(&mut self, ctx: &egui::Context) {
        let Some(Dialog::Proxy(mut form)) = self.dialog.take() else { return };
        let subtitle = tl!("Point a client at this computer; see and hold what it sends and what comes back.");
        let (closed, wants_close) =
            kit::dialog(ctx, "xmlcraft-proxy", 900.0, "cloud", tl!("SOAP Debugger"), subtitle, |ui| proxy_form(ui, &mut form));
        if form.start {
            form.start = false;
            self.start_proxy(&mut form);
        }
        if form.stop {
            form.stop = false;
            form.running = None;
            form.selected = None;
            form.edit_for = None;
        }
        if let Some(running) = &form.running {
            running.0.set_breaks(Breaks { requests: form.break_requests, responses: form.break_responses, contains: form.contains.clone() });
            self.proxy_act(&mut form);
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        // The window closes with the proxy: the last copy of the form drops it.
        if closed || wants_close {
            return;
        }
        self.dialog = Some(Dialog::Proxy(form));
    }

    fn start_proxy(&mut self, form: &mut ProxyForm) {
        let Ok(port) = form.port.trim().parse::<u16>() else {
            form.error = Some(tl!("The port is a number from 0 to 65535.").to_owned());
            return;
        };
        let config =
            Config { port, target: form.target.trim().to_owned(), allow_private_hosts: form.allow_private, timeout: Duration::from_secs(60) };
        match Debugger::start(config) {
            Ok(debugger) => {
                if let Some(ctx) = self.ctx.clone() {
                    debugger.on_change(move || ctx.request_repaint());
                }
                form.running = Some(Running(Arc::new(debugger)));
                form.error = None;
            }
            Err(why) => form.error = Some(why),
        }
    }

    /// Release or drop the held message, or open a request or an answer as a document.
    fn proxy_act(&mut self, form: &mut ProxyForm) {
        let (Some(act), Some(running), Some(id)) = (form.act.take(), form.running.clone(), form.selected) else { return };
        let exchange: Option<Exchange> = running.0.exchanges().into_iter().find(|e| e.id == id);
        match act {
            Act::Release => match parse_headers(&form.edit_headers) {
                Ok(headers) => {
                    running.0.release(id, Some(Message { headers, body: form.edit_body.clone().into_bytes() }));
                    form.edit_for = None;
                    form.error = None;
                }
                Err(why) => form.error = Some(why),
            },
            Act::Drop => {
                running.0.drop_held(id);
                form.edit_for = None;
            }
            Act::OpenRequest => {
                if let Some(e) = exchange {
                    let text = String::from_utf8_lossy(&e.request.body).into_owned();
                    self.open_generated(
                        &crate::http_pane::document_name("debugger", &format!("request-{id}"), crate::http_pane::answer_extension(&text)),
                        &text,
                    );
                }
            }
            Act::OpenResponse => {
                if let Some(e) = exchange {
                    let text = String::from_utf8_lossy(&e.response.body).into_owned();
                    self.open_generated(
                        &crate::http_pane::document_name("debugger", &format!("response-{id}"), crate::http_pane::answer_extension(&text)),
                        &text,
                    );
                }
            }
        }
    }
}

fn proxy_form(ui: &mut egui::Ui, form: &mut ProxyForm) -> bool {
    kit::section(ui, tl!("Proxy"), |ui| {
        let running = form.running.is_some();
        ui.horizontal(|ui| {
            ui.add_enabled(!running, egui::TextEdit::singleline(&mut form.port).desired_width(70.0).hint_text("8888"));
            ui.add_enabled(
                !running,
                egui::TextEdit::singleline(&mut form.target).desired_width(ui.available_width() - 90.0).hint_text("https://example.com/service"),
            );
            let label = if running { tl!("Stop") } else { tl!("Start") };
            if ui.button(label).clicked() {
                if running {
                    form.stop = true;
                } else {
                    form.start = true;
                }
            }
        });
        ui.add_enabled(!running, crate::widgets::checkbox(&mut form.allow_private, tl!("Allow this computer and the local network as the target")));
        if let Some(running) = &form.running {
            let text = fmt(
                tl!("Listening on http://{address}. Point the client at it; requests go to the target with the path they ask for."),
                &[("address", &running.0.address().to_string())],
            );
            kit::banner(ui, Tone::Good, &text);
        } else {
            kit::help(ui, tl!("The proxy listens on this computer only and stops when this window is closed. The client talks plain HTTP to it."));
        }
    });
    if let Some(message) = &form.error {
        kit::banner(ui, Tone::Bad, message);
    }
    kit::section(ui, tl!("Break"), |ui| {
        ui.horizontal(|ui| {
            ui.add(crate::widgets::checkbox(&mut form.break_requests, tl!("Hold requests")));
            ui.add(crate::widgets::checkbox(&mut form.break_responses, tl!("Hold answers")));
        });
        ui.add(egui::TextEdit::singleline(&mut form.contains).desired_width(f32::INFINITY).hint_text(tl!("only messages that contain this text")));
    });
    let exchanges: Vec<Exchange> = form.running.as_ref().map(|r| r.0.exchanges()).unwrap_or_default();
    kit::section(ui, tl!("Messages"), |ui| {
        if exchanges.is_empty() {
            kit::help(ui, tl!("Nothing has come through yet."));
        }
        egui::ScrollArea::vertical().id_salt("xmlcraft-proxy-list").max_height(150.0).show(ui, |ui| {
            for e in exchanges.iter().rev() {
                let status = e.status.as_ref().map(|(code, _)| format!(" {code}")).unwrap_or_default();
                let label = format!("#{}  {} {}  ·  {}{status}", e.id, e.method, e.path, stage_name(e.stage));
                if ui.selectable_label(form.selected == Some(e.id), label).clicked() {
                    form.selected = Some(e.id);
                    form.edit_for = None;
                }
            }
        });
    });
    if let Some(e) = form.selected.and_then(|id| exchanges.iter().find(|e| e.id == id)) {
        let held = matches!(e.stage, Stage::HeldRequest | Stage::HeldResponse);
        if held {
            if form.edit_for != Some((e.id, e.stage)) {
                let message = if e.stage == Stage::HeldRequest { &e.request } else { &e.response };
                form.edit_headers = headers_text(&message.headers);
                form.edit_body = String::from_utf8_lossy(&message.body).into_owned();
                form.edit_for = Some((e.id, e.stage));
            }
            let what =
                if e.stage == Stage::HeldRequest { tl!("Held request (edit it, then release)") } else { tl!("Held answer (edit it, then release)") };
            kit::section(ui, what, |ui| {
                ui.add(egui::TextEdit::multiline(&mut form.edit_headers).font(crate::theme::mono(11.5)).desired_rows(3).desired_width(f32::INFINITY));
                egui::ScrollArea::vertical().id_salt("xmlcraft-proxy-edit").max_height(170.0).show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut form.edit_body).font(crate::theme::mono(12.0)).desired_rows(8).desired_width(f32::INFINITY),
                    );
                });
                ui.horizontal(|ui| {
                    if ui.button(tl!("Release")).clicked() {
                        form.act = Some(Act::Release);
                    }
                    if ui.button(tl!("Drop")).clicked() {
                        form.act = Some(Act::Drop);
                    }
                });
            });
        } else {
            kit::section(ui, tl!("Request"), |ui| {
                let text = message_text(&e.request);
                let mut shown = text.as_str();
                egui::ScrollArea::vertical().id_salt("xmlcraft-proxy-request").max_height(120.0).show(ui, |ui| {
                    ui.add(egui::TextEdit::multiline(&mut shown).font(crate::theme::mono(11.5)).desired_rows(5).desired_width(f32::INFINITY));
                });
                if ui.small_button(tl!("Open the request as a document")).clicked() {
                    form.act = Some(Act::OpenRequest);
                }
            });
            kit::section(ui, tl!("Answer"), |ui| {
                if let Some(why) = &e.error {
                    kit::banner(ui, Tone::Bad, why);
                } else if e.status.is_some() {
                    let text = message_text(&e.response);
                    let mut shown = text.as_str();
                    egui::ScrollArea::vertical().id_salt("xmlcraft-proxy-response").max_height(120.0).show(ui, |ui| {
                        ui.add(egui::TextEdit::multiline(&mut shown).font(crate::theme::mono(11.5)).desired_rows(5).desired_width(f32::INFINITY));
                    });
                    if ui.small_button(tl!("Open the answer as a document")).clicked() {
                        form.act = Some(Act::OpenResponse);
                    }
                }
            });
        }
    }
    kit::close_footer(ui, tl!("Close"))
}
