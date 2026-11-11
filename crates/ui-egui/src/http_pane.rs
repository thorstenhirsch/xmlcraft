//! What the SOAP and OpenAPI windows have in common: an HTTP request a person can edit (method, address, headers, body), the thread that
//! sends it, and the answer. Sending is a person's act (the Send button) and goes through the network the person allowed: it is refused
//! while the app works offline, a host on this computer or the local network needs the box ticked, and redirects are not followed.

use std::sync::mpsc::{Receiver, Sender, channel};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex, PoisonError};

use xmlcraft_engine::soap::{Summary, headers_text, parse_headers, summarize};
use xmlcraft_platform::client::{Request, send};

use crate::XmlCraftApp;
use crate::dialog::Dialog;
use crate::i18n::{fmt, tl};
use crate::kit::{self, Tone};

/// The most bytes of a request the windows send.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

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
    /// SchemaStore's schemas for a file (the Assign Schema window).
    Catalog {
        serial: u64,
        result: Result<Vec<crate::assign::StoreMatch>, String>,
    },
    Prepared {
        serial: u64,
        result: Result<Prepared, String>,
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
            pane.error = Some(tl!("XmlCraft works offline, so nothing is sent. Turn Work Offline off (XML menu) to send.").to_owned());
            return;
        }
        let serial = self.next_http_serial();
        let mut request = Request::new(if method.is_empty() { "POST" } else { &method }, &url);
        request.headers = headers;
        request.body = pane.body.clone().into_bytes();
        request.allow_private_hosts = pane.allow_private;
        let started = self.spawn_http(move || {
            let result = send("XmlCraft", &request).map(|response| {
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
                pane.sending = Some(serial);
                pane.error = None;
                pane.answer = None;
            }
            Err(why) => pane.error = Some(why),
        }
    }

    /// The pane of the window that is open, if it is one of the HTTP windows.
    fn open_pane(&mut self) -> Option<&mut Pane> {
        match &mut self.dialog {
            Some(Dialog::Soap(form)) => Some(&mut form.pane),
            Some(Dialog::OpenApi(form)) => Some(&mut form.pane),
            _ => None,
        }
    }

    /// Collect what the threads finished. Called every frame.
    pub(crate) fn pump_http(&mut self) {
        while let Ok(outcome) = self.http.rx.try_recv() {
            if let Outcome::Catalog { serial, result } = outcome {
                self.schemastore_answered(serial, result);
                continue;
            }
            if let Outcome::Files { serial, result } = outcome {
                self.search_answered(serial, result);
                continue;
            }
            let Some(pane) = self.open_pane() else { continue };
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
                    match result {
                        Ok(answer) => pane.answer = Some(answer),
                        Err(why) => pane.error = Some(why),
                    }
                }
                _ => {}
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

fn mono_edit(ui: &mut egui::Ui, text: &mut String, rows: usize, id: &str) {
    egui::ScrollArea::vertical().id_salt(id).max_height(rows as f32 * 17.0).show(ui, |ui| {
        ui.add(egui::TextEdit::multiline(text).font(crate::theme::mono(12.0)).desired_rows(rows).desired_width(f32::INFINITY));
    });
}

/// The fields of the request and, below them, the state of sending and the answer. `with_method`: the method is for the person to
/// choose (SOAP always posts).
pub(crate) fn pane_ui(ui: &mut egui::Ui, pane: &mut Pane, with_method: bool) {
    kit::section(ui, tl!("Address"), |ui| {
        ui.horizontal(|ui| {
            if with_method {
                ui.add(egui::TextEdit::singleline(&mut pane.method).desired_width(80.0).font(crate::theme::mono(12.0)));
            }
            ui.add(egui::TextEdit::singleline(&mut pane.url).desired_width(f32::INFINITY).hint_text("https://example.com/service"));
        });
        ui.add(crate::widgets::checkbox(&mut pane.allow_private, tl!("Allow this computer and the local network")));
    });
    kit::section(ui, tl!("Headers"), |ui| mono_edit(ui, &mut pane.headers, 3, "xmlcraft-http-headers"));
    kit::section(ui, tl!("Request"), |ui| {
        mono_edit(ui, &mut pane.body, 8, "xmlcraft-http-request");
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
            ui.add(egui::TextEdit::multiline(&mut headers).font(crate::theme::mono(11.5)).desired_rows(2).desired_width(f32::INFINITY));
            let mut body = answer.body.as_str();
            egui::ScrollArea::vertical().id_salt("xmlcraft-http-answer").max_height(180.0).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut body).font(crate::theme::mono(12.0)).desired_rows(8).desired_width(f32::INFINITY));
            });
            if ui.small_button(tl!("Open the answer as a document")).clicked() {
                act = Some(Act::OpenResponse);
            }
        });
        if act.is_some() {
            pane.act = act;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
