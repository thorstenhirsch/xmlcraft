//! What the SOAP and OpenAPI windows have in common: an HTTP request a person can edit (method, address, headers, body), the thread that
//! sends it, and the answer. Sending is a person's act (the Send button) and goes through the network the person allowed: it is refused
//! while the app works offline, a host on this computer or the local network needs the box ticked, and redirects are not followed.

use std::sync::mpsc::{Receiver, Sender, channel};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex, PoisonError};

use xmlcraft_engine::http_auth::Auth;
use xmlcraft_engine::soap::{Summary, headers_text, parse_headers, summarize};
use xmlcraft_platform::client::{Request, send};

use crate::Panel;
use crate::XmlCraftApp;
use crate::i18n::{fmt, tl};
use crate::kit::{self, Tone};

/// The most bytes of a request the windows send.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// The most requests the history of a window keeps.
pub(crate) const MAX_HISTORY: usize = 30;

/// A request of this run, for the history: what was asked (without any secret) and how it came out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HistoryEntry {
    serial: u64,
    method: String,
    url: String,
    headers: String,
    body: String,
    /// The status line, the error, or nothing while the answer is awaited.
    outcome: Option<String>,
}

impl HistoryEntry {
    /// The line the list shows.
    pub(crate) fn label(&self) -> String {
        format!("{} {} — {}", self.method, self.url, self.outcome.as_deref().unwrap_or("…"))
    }
}

/// A header that carries a secret: it is not kept in the history.
fn is_secret_header(name: &str) -> bool {
    let lower = name.trim().to_ascii_lowercase();
    matches!(lower.as_str(), "authorization" | "proxy-authorization" | "cookie" | "set-cookie")
        || ["key", "token", "secret", "password"].iter().any(|word| lower.contains(word))
}

/// What a send came back with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Answer {
    status: u16,
    reason: String,
    headers: String,
    pub(crate) body: String,
    elapsed_ms: u64,
    summary: Option<Summary>,
}

/// A request made for an operation, to put in the pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Prepared {
    /// None: the window's own method stays (SOAP always posts).
    pub method: Option<String>,
    pub url: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub warnings: Vec<String>,
}

/// What a thread came to.
pub(crate) enum Outcome {
    /// What Find in Files found.
    Files {
        serial: u64,
        result: Result<xmlcraft_platform::search::Found, String>,
    },
    /// The start of a conversion, for the Transform tool's preview.
    Preview {
        serial: u64,
        result: Result<crate::tools::transform::Preview, String>,
    },
    /// What the E-Rechnung tool's overview reads from a document (its detection and its invoice model).
    InvoiceReading {
        serial: u64,
        result: Result<Box<crate::tools::einvoice::Reading>, String>,
    },
    /// A hybrid PDF the E-Rechnung tool made.
    InvoicePdf {
        serial: u64,
        result: Box<Result<crate::tools::einvoice::Made, String>>,
    },
    /// SchemaStore's schemas for a file (the Assign Schema window).
    Catalog {
        serial: u64,
        result: Result<Vec<crate::assign::StoreMatch>, String>,
    },
    Prepared {
        serial: u64,
        result: Result<Prepared, String>,
    },
    /// The page of the Preview tool window, for a document (its id and name).
    Rendered {
        serial: u64,
        doc: u64,
        name: String,
        result: Result<Box<xmlcraft_engine::preview::Prepared>, String>,
    },
    Sent {
        serial: u64,
        result: Result<Answer, String>,
    },
}

/// What a thread is to do and hand back.
type Job = Box<dyn FnOnce() -> Outcome + Send>;

/// The channel the threads answer on.
pub(crate) struct HttpWorker {
    tx: Sender<Outcome>,
    rx: Receiver<Outcome>,
}

impl Default for HttpWorker {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self { tx, rx }
    }
}

/// What a window was asked to do besides sending.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Act {
    OpenRequest,
    OpenResponse,
}

/// The request and the answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Pane {
    pub method: String,
    pub url: String,
    pub headers: String,
    pub body: String,
    pub allow_private: bool,
    pub warnings: Vec<String>,
    pub preparing: Option<u64>,
    pub sending: Option<u64>,
    pub error: Option<String>,
    pub answer: Option<Answer>,
    pub act: Option<Act>,
    /// How the request is authorized: set from the window's form when it sends, held in memory only (never in the history).
    pub auth: Auth,
    /// The requests of this run, the newest first (at most [`MAX_HISTORY`]).
    pub history: Vec<HistoryEntry>,
}

impl Pane {
    /// Put a request that was made in the fields.
    pub(crate) fn apply(&mut self, made: Prepared) {
        if let Some(method) = made.method {
            self.method = method;
        }
        self.url = made.url.unwrap_or_default();
        self.headers = headers_text(&made.headers);
        self.body = made.body;
        self.warnings = made.warnings;
        self.answer = None;
    }

    /// Remember a request that is being sent. The headers are those the person typed, without the ones that carry a secret; the
    /// authorization of the window is not part of it.
    fn remember(&mut self, serial: u64, method: &str, url: &str, headers: &[(String, String)], body: &str) {
        let kept: Vec<(String, String)> = headers.iter().filter(|(n, _)| !is_secret_header(n)).cloned().collect();
        self.history.insert(
            0,
            HistoryEntry {
                serial,
                method: method.to_owned(),
                url: url.to_owned(),
                headers: headers_text(&kept),
                body: body.to_owned(),
                outcome: None,
            },
        );
        self.history.truncate(MAX_HISTORY);
    }

    /// Say how the request `serial` came out.
    fn remembered(&mut self, serial: u64, result: &Result<Answer, String>) {
        let outcome = match result {
            Ok(answer) => format!("{} {} in {} ms", answer.status, answer.reason, answer.elapsed_ms),
            Err(why) => why.chars().take(120).collect(),
        };
        if let Some(entry) = self.history.iter_mut().find(|e| e.serial == serial) {
            entry.outcome = Some(outcome);
        }
    }

    /// Put the request of history entry `index` back in the fields (without the authorization, which stays as the form has it).
    pub(crate) fn restore(&mut self, index: usize) {
        let Some(entry) = self.history.get(index).cloned() else { return };
        self.method = entry.method;
        self.url = entry.url;
        self.headers = entry.headers;
        self.body = entry.body;
        self.answer = None;
        self.error = None;
    }

    /// Whether Send may be pressed.
    pub(crate) fn can_send(&self) -> bool {
        self.preparing.is_none() && self.sending.is_none() && !self.url.trim().is_empty()
    }
}

/// The name of a request or an answer opened as a document.
pub(crate) fn document_name(name: &str, what: &str, extension: &str) -> String {
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    format!("{stem}-{what}.{extension}")
}

/// The extension an answer is saved under, by what it looks like.
pub(crate) fn answer_extension(body: &str) -> &'static str {
    let t = body.trim_start();
    if t.starts_with('<') {
        "xml"
    } else if t.starts_with(['{', '[']) {
        "json"
    } else {
        "txt"
    }
}

impl XmlCraftApp {
    /// Run `job` on a thread (without one, on the web, at once); its outcome comes back through `pump_http`.
    pub(crate) fn spawn_http(&self, job: impl FnOnce() -> Outcome + Send + 'static) -> Result<(), String> {
        let (tx, ctx) = (self.http.tx.clone(), self.ctx.clone());
        let run = move |job: Job| {
            let _ = tx.send(job());
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        };
        #[cfg(target_arch = "wasm32")]
        {
            run(Box::new(job));
            Ok(())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let slot: Arc<Mutex<Option<Job>>> = Arc::new(Mutex::new(Some(Box::new(job))));
            std::thread::Builder::new()
                .name("http".into())
                .spawn(move || {
                    let taken = slot.lock().unwrap_or_else(PoisonError::into_inner).take();
                    if let Some(job) = taken {
                        run(job);
                    }
                })
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
    }

    /// The next number for a request to a thread.
    pub(crate) fn next_http_serial(&mut self) -> u64 {
        self.generate_serial += 1;
        self.generate_serial
    }

    /// Send the request as it is in the pane.
    pub(crate) fn send_pane(&mut self, pane: &mut Pane) {
        let headers = match parse_headers(&pane.headers) {
            Ok(h) => h,
            Err(why) => {
                pane.error = Some(why);
                return;
            }
        };
        let url = pane.url.trim().to_owned();
        if url.is_empty() {
            pane.error = Some(tl!("Give the address to send to.").to_owned());
            return;
        }
        let method = pane.method.trim().to_ascii_uppercase();
        if pane.body.len() > MAX_BODY_BYTES {
            pane.error = Some(fmt(tl!("The request is larger than {limit} MB."), &[("limit", &(MAX_BODY_BYTES / (1024 * 1024)).to_string())]));
            return;
        }
        if self.network.is_offline() || !self.network.can_download() {
            pane.error = Some(tl!("XmlCraft works offline, so nothing is sent. Turn Work Offline off (Tools menu) to send.").to_owned());
            return;
        }
        let serial = self.next_http_serial();
        let sent_method = if method.is_empty() { "POST".to_owned() } else { method.clone() };
        // The authorization goes into the request that is sent, never into what is remembered or shown.
        let (mut sent_headers, mut sent_url) = (headers.clone(), url.clone());
        if let Err(why) = pane.auth.apply(&mut sent_headers, &mut sent_url) {
            pane.error = Some(why);
            return;
        }
        let auth = pane.auth.clone();
        let mut request = Request::new(&sent_method, &sent_url);
        request.headers = sent_headers;
        request.body = pane.body.clone().into_bytes();
        request.allow_private_hosts = pane.allow_private;
        let started = self.spawn_http(move || {
            let result = send("XmlCraft", &request).map_err(|why| auth.redact(&why)).map(|response| {
                let decoded = xmlcraft_text::encoding::decode(&response.body, &xmlcraft_text::encoding::sniff(&response.body));
                Answer {
                    status: response.status,
                    reason: response.reason.clone(),
                    headers: headers_text(&response.headers),
                    summary: summarize(&decoded.text),
                    body: decoded.text,
                    elapsed_ms: u64::try_from(response.elapsed.as_millis()).unwrap_or(u64::MAX),
                }
            });
            Outcome::Sent { serial, result }
        });
        match started {
            Ok(()) => {
                pane.remember(serial, &sent_method, &url, &headers, &pane.body.clone());
                pane.sending = Some(serial);
                pane.error = None;
                pane.answer = None;
            }
            Err(why) => pane.error = Some(why),
        }
    }

    /// The pane that waits for the answer `serial`, and the tool window it belongs to. Whatever else is
    /// open, an answer finds its pane.
    fn pane_waiting_for(&mut self, serial: u64) -> Option<(&mut Pane, Panel)> {
        let waits = |pane: &Pane| pane.preparing == Some(serial) || pane.sending == Some(serial);
        if waits(&self.http_tool.pane) {
            return Some((&mut self.http_tool.pane, Panel::Http));
        }
        waits(&self.soap_tool.pane).then_some((&mut self.soap_tool.pane, Panel::Soap))
    }

    /// Collect what the threads finished. Called every frame.
    pub(crate) fn pump_http(&mut self) {
        while let Ok(outcome) = self.http.rx.try_recv() {
            if let Outcome::Catalog { serial, result } = outcome {
                self.schemastore_answered(serial, result);
                continue;
            }
            if let Outcome::Preview { serial, result } = outcome {
                self.preview_answered(serial, result);
                continue;
            }
            if let Outcome::Files { serial, result } = outcome {
                self.search_answered(serial, result);
                continue;
            }
            if let Outcome::InvoiceReading { serial, result } = outcome {
                self.invoice_read(serial, result);
                continue;
            }
            if let Outcome::InvoicePdf { serial, result } = outcome {
                self.invoice_pdf_made(serial, *result);
                continue;
            }
            if let Outcome::Rendered { serial, doc, name, result } = outcome {
                self.page_rendered(serial, doc, name, result);
                continue;
            }
            let (Outcome::Prepared { serial, .. } | Outcome::Sent { serial, .. }) = &outcome else { continue };
            let Some((pane, owner)) = self.pane_waiting_for(*serial) else { continue };
            let mut news = false;
            match outcome {
                Outcome::Prepared { serial, result } if pane.preparing == Some(serial) => {
                    pane.preparing = None;
                    match result {
                        Ok(made) => pane.apply(made),
                        Err(why) => pane.error = Some(why),
                    }
                }
                Outcome::Sent { serial, result } if pane.sending == Some(serial) => {
                    pane.sending = None;
                    news = true;
                    pane.remembered(serial, &result);
                    match result {
                        Ok(answer) => pane.answer = Some(answer),
                        Err(why) => pane.error = Some(why),
                    }
                }
                _ => {}
            }
            // An answer that comes while the panel is hidden marks its rail icon.
            if news {
                self.workbench.set_attention(owner);
            }
        }
    }

    /// Do what the pane was asked to besides sending: open the request or the answer as a document.
    pub(crate) fn do_pane_act(&mut self, pane: &mut Pane, name: &str) {
        match pane.act.take() {
            Some(Act::OpenRequest) => {
                let extension = answer_extension(&pane.body);
                self.open_generated(&document_name(name, "request", extension), &pane.body);
            }
            Some(Act::OpenResponse) => {
                if let Some(answer) = &pane.answer {
                    self.open_generated(&document_name(name, "response", answer_extension(&answer.body)), &answer.body);
                }
            }
            None => {}
        }
    }

    /// What a pane holds, for tests: the request body, the answer (status and body) and the error shown.
    pub(crate) fn pane_view(pane: &Pane) -> PaneView {
        (pane.body.clone(), pane.answer.as_ref().map(|a| (a.status, a.body.clone())), pane.error.clone())
    }
}

/// What a window holds, for tests: the request as it is, the answer (status and body) and the error shown.
pub type PaneView = (String, Option<(u16, String)>, Option<String>);

/// The height of a line of the request and answer fields.
const LINE: f32 = 15.0;

/// A monospace field of `rows` lines that fills the width.
fn mono_edit(ui: &mut egui::Ui, text: &mut String, rows: usize) {
    ui.add(egui::TextEdit::multiline(text).font(crate::theme::mono(12.0)).desired_rows(rows).desired_width(f32::INFINITY));
}

/// The fields of the request and, below them, the state of sending and the answer. `with_method`: the method is for the person to
/// choose (SOAP always posts). The text areas share the height of the view they are in (the request about a fifth of it, the answer
/// about a third), and the panel's own scroll area takes what does not fit.
pub(crate) fn pane_ui(ui: &mut egui::Ui, pane: &mut Pane, with_method: bool) {
    let view = ui.clip_rect().height().clamp(120.0, 3000.0);
    let rows = |share: f32| ((view * share / LINE) as usize).clamp(4, 80);
    kit::section(ui, tl!("Address"), |ui| {
        ui.horizontal(|ui| {
            if with_method {
                let method = ui.add(egui::TextEdit::singleline(&mut pane.method).desired_width(64.0).font(crate::theme::mono(12.0)));
                crate::widgets::name_for_assistive_technology(ui, &method, tl!("The HTTP method"));
            }
            let url = ui.add(egui::TextEdit::singleline(&mut pane.url).desired_width(f32::INFINITY).hint_text("https://example.com/service"));
            crate::widgets::name_for_assistive_technology(ui, &url, tl!("The address to send to"));
        });
        ui.add(crate::widgets::checkbox(&mut pane.allow_private, tl!("Allow this computer and the local network")));
    });
    kit::section(ui, tl!("Headers"), |ui| mono_edit(ui, &mut pane.headers, 3));
    kit::section(ui, tl!("Request"), |ui| {
        mono_edit(ui, &mut pane.body, rows(0.2));
        if ui.small_button(tl!("Open the request as a document")).clicked() {
            pane.act = Some(Act::OpenRequest);
        }
    });
    for warning in &pane.warnings {
        kit::help(ui, warning);
    }
    if pane.sending.is_some() {
        ui.add_space(kit::GAP_GROUP);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Waiting for the answer…"));
        });
    }
    if let Some(message) = &pane.error {
        ui.add_space(kit::GAP_GROUP);
        kit::banner(ui, Tone::Bad, message);
    }
    if let Some(answer) = &pane.answer {
        ui.add_space(kit::GAP_GROUP);
        let ok = (200..300).contains(&answer.status);
        let line = fmt(
            tl!("{status} {reason} in {ms} ms"),
            &[("status", &answer.status.to_string()), ("reason", &answer.reason), ("ms", &answer.elapsed_ms.to_string())],
        );
        kit::banner(ui, if ok && answer.summary.as_ref().is_none_or(|s| s.fault.is_none()) { Tone::Good } else { Tone::Warn }, &line);
        if let Some(summary) = &answer.summary {
            let text = match &summary.fault {
                Some(fault) => fmt(tl!("SOAP {version} fault: {fault}"), &[("version", summary.version), ("fault", fault)]),
                None => fmt(tl!("A SOAP {version} message"), &[("version", summary.version)]),
            };
            kit::help(ui, &text);
        }
        let mut act = None;
        kit::section(ui, tl!("Answer"), |ui| {
            let mut headers = answer.headers.as_str();
            ui.add(egui::TextEdit::multiline(&mut headers).font(crate::theme::mono(11.5)).desired_rows(3).desired_width(f32::INFINITY));
            let mut body = answer.body.as_str();
            let rows = rows(0.35);
            ui.add(egui::TextEdit::multiline(&mut body).font(crate::theme::mono(12.0)).desired_rows(rows).desired_width(f32::INFINITY));
            if ui.small_button(tl!("Open the answer as a document")).clicked() {
                act = Some(Act::OpenResponse);
            }
        });
        if act.is_some() {
            pane.act = act;
        }
    }
    history_ui(ui, pane);
}

/// The requests of this run: a click puts one back in the fields.
fn history_ui(ui: &mut egui::Ui, pane: &mut Pane) {
    if pane.history.is_empty() {
        return;
    }
    let mut restore = None;
    let mut clear = false;
    kit::section(ui, tl!("History"), |ui| {
        for (i, entry) in pane.history.iter().enumerate() {
            let response =
                ui.selectable_label(false, entry.label()).on_hover_text(tl!("Put this request back in the fields (without its authorization)"));
            if response.clicked() {
                restore = Some(i);
            }
        }
        if ui.small_button(tl!("Clear history")).clicked() {
            clear = true;
        }
        kit::help(ui, tl!("Kept for this run only; the secrets of the Authorization section are never in it."));
    });
    if let Some(i) = restore {
        pane.restore(i);
    }
    if clear {
        pane.history.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_history_keeps_thirty_requests_without_secret_headers_and_restores_one() {
        let mut pane = Pane::default();
        for i in 0..40u64 {
            let headers = vec![
                ("Accept".to_owned(), "*/*".to_owned()),
                ("Authorization".to_owned(), "Bearer s3cret".to_owned()),
                ("X-Api-Key".to_owned(), "k".to_owned()),
            ];
            pane.remember(i, "GET", &format!("https://x.test/{i}"), &headers, "body");
        }
        assert_eq!(pane.history.len(), MAX_HISTORY);
        assert_eq!(pane.history[0].url, "https://x.test/39", "the newest first");
        assert!(pane.history.iter().all(|e| !e.headers.contains("s3cret") && !e.headers.contains("X-Api-Key") && e.headers.contains("Accept")));
        pane.remembered(39, &Err("refused".to_owned()));
        assert_eq!(pane.history[0].label(), "GET https://x.test/39 — refused");
        pane.restore(2);
        assert_eq!((pane.method.as_str(), pane.url.as_str(), pane.body.as_str()), ("GET", "https://x.test/37", "body"));
        assert!(!pane.headers.contains("Authorization"));
        pane.restore(999);
    }

    #[test]
    fn documents_made_from_a_request_or_an_answer_are_named_after_the_description_and_what_they_look_like() {
        assert_eq!(document_name("calc.wsdl", "request", "xml"), "calc-request.xml");
        assert_eq!(document_name("Untitled-1", "response", "json"), "Untitled-1-response.json");
        assert_eq!(document_name("a.b.yaml", "response", "txt"), "a.b-response.txt");
        assert_eq!(
            (answer_extension(" <a/>"), answer_extension("{\"a\":1}"), answer_extension("[1]"), answer_extension("hi")),
            ("xml", "json", "json", "txt")
        );
    }
}
