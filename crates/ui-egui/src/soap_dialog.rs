//! The SOAP window: the operations of the WSDL being shown, the request made for one (address, headers and an envelope with a sample
//! body, all of it editable) and the answer to sending it. Sending is a person's act (the Send button) and goes through the network
//! the person allowed: it is refused while the app works offline, hosts on this computer or the local network need the box ticked, and
//! redirects are not followed. The request and the answer can be opened as documents; nothing is changed in the WSDL.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};

use xmlcraft_engine::soap::{OperationEntry, Prepared, Summary, headers_text, operations, parse_headers, prepare, summarize};
use xmlcraft_engine::validation::OpenDocuments;
use xmlcraft_platform::client::{Request, send};
use xmlcraft_text::Buffer;

use crate::XmlCraftApp;
use crate::dialog::Dialog;
use crate::i18n::{fmt, tl};
use crate::kit::{self, Footer, Tone};

/// The most bytes of a request the window sends.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// What a send came back with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Answer {
    status: u16,
    reason: String,
    headers: String,
    body: String,
    elapsed_ms: u64,
    summary: Option<Summary>,
}

/// What a job on a thread came to.
pub(crate) enum Outcome {
    Prepared { serial: u64, result: Result<Prepared, String> },
    Sent { serial: u64, result: Result<Answer, String> },
}

/// The channel the threads answer on.
pub(crate) struct SoapWorker {
    tx: Sender<Outcome>,
    rx: Receiver<Outcome>,
}

impl Default for SoapWorker {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self { tx, rx }
    }
}

/// What the window was asked to do besides sending.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    OpenRequest,
    OpenResponse,
}

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
    url: String,
    headers: String,
    body: String,
    allow_private: bool,
    warnings: Vec<String>,
    preparing: Option<u64>,
    sending: Option<u64>,
    error: Option<String>,
    answer: Option<Answer>,
    act: Option<Act>,
}

/// What a thread is to do and hand back.
type Job = Box<dyn FnOnce() -> Outcome + Send>;

/// What the window holds, for tests: the request as it is, the answer (status and body) and the error shown.
pub type SoapView = (String, Option<(u16, String)>, Option<String>);

/// The extension of an answer: SOAP is XML.
fn answer_name(name: &str, what: &str) -> String {
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    format!("{stem}-{what}.xml")
}

impl XmlCraftApp {
    /// What the SOAP window holds, for tests: the request as it is in the window, the answer (status and body) and the error shown.
    pub fn soap_window(&self) -> Option<SoapView> {
        let Some(Dialog::Soap(form)) = &self.dialog else { return None };
        Some((form.body.clone(), form.answer.as_ref().map(|a| (a.status, a.body.clone())), form.error.clone()))
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
        let form = SoapForm {
            source: doc.id(),
            name: doc.name().to_owned(),
            text,
            location,
            ops,
            chosen: 0,
            prepared_for: None,
            url: String::new(),
            headers: String::new(),
            body: String::new(),
            allow_private: false,
            warnings,
            preparing: None,
            sending: None,
            error: None,
            answer: None,
            act: None,
        };
        let mut form = Box::new(form);
        self.prepare_soap(&mut form);
        self.dialog = Some(Dialog::Soap(form));
    }

    /// Run `job` on a thread (without one, on the web, at once); its outcome comes back through `pump_soap`.
    fn spawn_soap(&self, job: impl FnOnce() -> Outcome + Send + 'static) -> Result<(), String> {
        let (tx, ctx) = (self.soap.tx.clone(), self.ctx.clone());
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
                .name("soap".into())
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

    /// Make the request of the chosen operation (on a thread: the schemas it imports are read, which may take a moment).
    fn prepare_soap(&mut self, form: &mut SoapForm) {
        let Some(op) = form.ops.get(form.chosen).cloned() else { return };
        self.generate_serial += 1;
        let serial = self.generate_serial;
        let (text, location, network) = (form.text.clone(), form.location.clone(), self.network.clone());
        let open: Vec<(String, Buffer)> = self.open_documents();
        let started = self.spawn_soap(move || {
            let resolver = network.resolver(Arc::new(OpenDocuments::new(open)));
            let result = xmlcraft_engine::guard(|| prepare(&text, &location, &op.name, op.binding.as_deref(), &resolver))
                .unwrap_or_else(|_| Err("making the request failed (a bug); nothing was changed".to_owned()));
            Outcome::Prepared { serial, result }
        });
        match started {
            Ok(()) => {
                form.preparing = Some(serial);
                form.prepared_for = Some(form.chosen);
                form.error = None;
            }
            Err(why) => form.error = Some(why),
        }
    }

    /// Send the request as it is in the window.
    fn send_soap(&mut self, form: &mut SoapForm) {
        let headers = match parse_headers(&form.headers) {
            Ok(h) => h,
            Err(why) => {
                form.error = Some(why);
                return;
            }
        };
        let url = form.url.trim().to_owned();
        if url.is_empty() {
            form.error = Some(tl!("Give the address to send to.").to_owned());
            return;
        }
        if form.body.len() > MAX_BODY_BYTES {
            form.error = Some(fmt(tl!("The request is larger than {limit} MB."), &[("limit", &(MAX_BODY_BYTES / (1024 * 1024)).to_string())]));
            return;
        }
        if self.network.is_offline() || !self.network.can_download() {
            form.error = Some(tl!("XmlCraft works offline, so nothing is sent. Turn Work Offline off (XML menu) to send.").to_owned());
            return;
        }
        self.generate_serial += 1;
        let serial = self.generate_serial;
        let mut request = Request::new("POST", &url);
        request.headers = headers;
        request.body = form.body.clone().into_bytes();
        request.allow_private_hosts = form.allow_private;
        let started = self.spawn_soap(move || {
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
                form.sending = Some(serial);
                form.error = None;
                form.answer = None;
            }
            Err(why) => form.error = Some(why),
        }
    }

    /// Collect what the threads finished. Called every frame.
    pub(crate) fn pump_soap(&mut self) {
        while let Ok(outcome) = self.soap.rx.try_recv() {
            let Some(Dialog::Soap(form)) = &mut self.dialog else { continue };
            match outcome {
                Outcome::Prepared { serial, result } if form.preparing == Some(serial) => {
                    form.preparing = None;
                    match result {
                        Ok(made) => {
                            form.url = made.url.unwrap_or_default();
                            form.headers = headers_text(&made.headers);
                            form.body = made.body;
                            form.warnings = made.warnings;
                            form.answer = None;
                        }
                        Err(why) => form.error = Some(why),
                    }
                }
                Outcome::Sent { serial, result } if form.sending == Some(serial) => {
                    form.sending = None;
                    match result {
                        Ok(answer) => form.answer = Some(answer),
                        Err(why) => form.error = Some(why),
                    }
                }
                _ => {}
            }
        }
    }

    pub(crate) fn show_soap(&mut self, ctx: &egui::Context) {
        let Some(Dialog::Soap(mut form)) = self.dialog.take() else { return };
        let subtitle = fmt(tl!("Make a SOAP request from {name}, edit it and send it."), &[("name", &form.name)]);
        let (action, closed) = kit::dialog(ctx, "xmlcraft-soap", 760.0, "cloud", tl!("Send SOAP Request"), &subtitle, |ui| soap_form(ui, &mut form));
        // The choice of another operation makes the request again.
        if form.prepared_for != Some(form.chosen) && form.preparing.is_none() {
            self.prepare_soap(&mut form);
        }
        match form.act.take() {
            Some(Act::OpenRequest) => {
                let name = answer_name(&form.name, "request");
                self.open_generated(&name, &form.body);
            }
            Some(Act::OpenResponse) => {
                if let Some(answer) = &form.answer {
                    let name = answer_name(&form.name, "response");
                    self.open_generated(&name, &answer.body);
                }
            }
            None => {}
        }
        match action {
            Some(Footer::Primary) => {
                self.send_soap(&mut form);
                self.dialog = Some(Dialog::Soap(form));
            }
            Some(Footer::Cancel) => {}
            _ if closed => {}
            _ => self.dialog = Some(Dialog::Soap(form)),
        }
    }
}

fn mono_edit(ui: &mut egui::Ui, text: &mut String, rows: usize, id: &str) {
    egui::ScrollArea::vertical().id_salt(id).max_height(rows as f32 * 17.0).show(ui, |ui| {
        ui.add(egui::TextEdit::multiline(text).font(crate::theme::mono(12.0)).desired_rows(rows).desired_width(f32::INFINITY));
    });
}

fn soap_form(ui: &mut egui::Ui, form: &mut SoapForm) -> Option<Footer> {
    kit::section(ui, tl!("Operation"), |ui| {
        let current = form.ops.get(form.chosen).map(OperationEntry::label).unwrap_or_default();
        egui::ComboBox::from_id_salt("xmlcraft-soap-operation").selected_text(current).width(ui.available_width() - 8.0).show_ui(ui, |ui| {
            for (i, op) in form.ops.iter().enumerate() {
                ui.selectable_value(&mut form.chosen, i, op.label());
            }
        });
        if form.preparing.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(tl!("Making the request…"));
            });
        }
    });
    kit::section(ui, tl!("Address"), |ui| {
        ui.add(egui::TextEdit::singleline(&mut form.url).desired_width(f32::INFINITY).hint_text("https://example.com/service"));
        ui.add(crate::widgets::checkbox(&mut form.allow_private, tl!("Allow this computer and the local network")));
    });
    kit::section(ui, tl!("Headers"), |ui| mono_edit(ui, &mut form.headers, 3, "xmlcraft-soap-headers"));
    kit::section(ui, tl!("Request"), |ui| {
        mono_edit(ui, &mut form.body, 10, "xmlcraft-soap-request");
        if ui.small_button(tl!("Open the request as a document")).clicked() {
            form.act = Some(Act::OpenRequest);
        }
    });
    for warning in &form.warnings {
        kit::help(ui, warning);
    }
    if form.sending.is_some() {
        ui.add_space(kit::GAP_GROUP);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Waiting for the answer…"));
        });
    }
    if let Some(message) = &form.error {
        ui.add_space(kit::GAP_GROUP);
        kit::banner(ui, Tone::Bad, message);
    }
    if let Some(answer) = &mut form.answer {
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
        kit::section(ui, tl!("Answer"), |ui| {
            let mut headers = answer.headers.as_str();
            ui.add(egui::TextEdit::multiline(&mut headers).font(crate::theme::mono(11.5)).desired_rows(2).desired_width(f32::INFINITY));
            let mut body = answer.body.as_str();
            egui::ScrollArea::vertical().id_salt("xmlcraft-soap-answer").max_height(180.0).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut body).font(crate::theme::mono(12.0)).desired_rows(8).desired_width(f32::INFINITY));
            });
            if ui.small_button(tl!("Open the answer as a document")).clicked() {
                form.act = Some(Act::OpenResponse);
            }
        });
    }
    let can = form.preparing.is_none() && form.sending.is_none() && !form.body.is_empty();
    match kit::footer(ui, tl!("Close"), tl!("Send"), Some("cloud"), can, tl!("Wait for the request to be made.")) {
        Footer::Nothing => None,
        other => Some(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_and_the_answer_are_named_after_the_wsdl() {
        assert_eq!(answer_name("calc.wsdl", "request"), "calc-request.xml");
        assert_eq!(answer_name("Untitled-1", "response"), "Untitled-1-response.xml");
        assert_eq!(answer_name("a.b.wsdl", "response"), "a.b-response.xml");
    }
}
