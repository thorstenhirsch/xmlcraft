//! E-Rechnung, the tool window of electronic invoices (XRechnung, ZUGFeRD / Factur-X, Peppol BIS, plain UBL and CII, FatturaPA). It
//! follows the document being shown and has three tabs:
//!
//! - **Overview**: a summary card (syntax, standard and profile, number, dates, seller and buyer, currency, amount due, lines) and the
//!   invoice as a person reads it (parties, lines, VAT breakdown, totals, payment, notes), from the neutral model of
//!   `xmlcraft_einvoice::model`, worked out on a thread a moment after the text stood still. Every value keeps where it is in the text:
//!   a click selects it there.
//! - **Validate**: the stages of the e-invoice validation (well-formedness, detection, XML Schema, EN 16931, the CIUS) and their rule
//!   findings. It is the same result as F8's (`Validator::submit_with_invoice_rules`): the button here runs F8.
//! - **PDF**: a hybrid PDF made from the invoice (`xmlcraft_invoice_pdf`, on a thread), and, for a PDF document, what is inside it.
//!
//! Everything a result says belongs to the document and the version of its text it was made from: after an edit it is shown as out of
//! date, and its clicks no longer select anything (the ranges would be wrong).

use std::ops::Range;
use std::path::{Path, PathBuf};

use xmlcraft_einvoice::model::{Address, Field, Invoice, Party};
use xmlcraft_einvoice::validate::{Report, Severity, Stage, Status};
use xmlcraft_einvoice::{Detection, Standard, Syntax};

use crate::http_pane::Outcome;
use crate::i18n::{fmt, tl};
use crate::kit::{self, Tone};
use crate::theme::{self, Tokens};
use crate::tools;
use crate::{Panel, XmlCraftApp, widgets};

/// The most bytes of a document the tool reads.
const MAX_BYTES: usize = 32 * 1024 * 1024;
/// How long the text is left alone before the overview is worked out again, in seconds.
const DEBOUNCE: f64 = 0.3;
/// The most invoice lines the overview lists (the others are counted).
const LINES_SHOWN: usize = 300;
/// The height of a row of the findings.
const ROW: f32 = 26.0;
/// The most characters of a value shown (the text keeps all of it).
const VALUE_CHARS: usize = 240;

/// A document and a version of its text.
pub(crate) type Key = (u64, u64);

/// Which part of the tool is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Tab {
    #[default]
    Overview,
    Validate,
    Pdf,
}

/// What the thread found the document to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Reading {
    pub detection: Option<Detection>,
    pub invoice: Option<Invoice>,
}

/// The overview being worked out or shown.
#[derive(Debug, Default)]
struct ModelState {
    /// What the overview is for now; a change starts the wait.
    key: Option<Key>,
    /// When (egui time, seconds) the wait for a quiet moment ends.
    due: Option<f64>,
    /// The request being waited for, and what it was asked for.
    waiting: Option<(u64, Key)>,
    /// The last answer, and what it was for.
    shown: Option<(Key, Result<Reading, String>)>,
}

/// The template of a hybrid PDF.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Template {
    #[default]
    German,
    English,
    File,
}

/// What a rendering made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Made {
    pub path: PathBuf,
    pub pages: usize,
    pub bytes: usize,
    pub hybrid: bool,
    pub profile: Option<String>,
    pub file_name: String,
    pub relationship: String,
    pub reason: Option<String>,
    pub pdfa: bool,
    pub pdfa_reason: Option<String>,
    pub warnings: Vec<String>,
}

/// The form of the PDF tab.
#[derive(Debug, Default)]
struct PdfForm {
    template: Template,
    template_file: Option<PathBuf>,
    /// The PDF to write, as typed or chosen.
    out: String,
    /// The document the output path was suggested for (another document gets its own suggestion).
    out_for: Option<u64>,
    open_result: bool,
    /// The file exists: the next click replaces it only after "Replace".
    confirm: Option<PathBuf>,
    /// The rendering being waited for.
    waiting: Option<u64>,
    /// The last rendering, and the document and version it was made from.
    result: Option<(Key, Result<Made, String>)>,
}

/// The state of the E-Rechnung tool window. A hidden tool keeps it, and the threads' answers land in it whatever is shown.
#[derive(Debug, Default)]
pub(crate) struct EInvoiceTool {
    tab: Tab,
    model: ModelState,
    /// The validation (F8 or the button) being waited for.
    waiting: Option<Key>,
    /// The last validation report of an invoice, and what it was made from.
    report: Option<(Key, Box<Report>)>,
    /// The text the findings are filtered by.
    filter: String,
    pdf: PdfForm,
}

impl EInvoiceTool {
    /// The last report, for `ui.state`.
    pub(crate) fn state(&self) -> serde_json::Value {
        let report = self.report.as_ref().map(|((doc, version), r)| {
            serde_json::json!({
                "document": doc,
                "version": version,
                "valid": r.valid(),
                "findings": r.findings_total,
                "not_evaluated": r.not_evaluated_total,
                "stages": r.stages.iter().map(|s| serde_json::json!({ "stage": s.stage.key(), "status": status_word(&s.status) })).collect::<Vec<_>>(),
            })
        });
        let tab = match self.tab {
            Tab::Overview => "overview",
            Tab::Validate => "validate",
            Tab::Pdf => "pdf",
        };
        serde_json::json!({ "tab": tab, "validating": self.waiting.is_some(), "report": report, "rendering": self.pdf.waiting.is_some() })
    }
}

fn status_word(status: &Status) -> &'static str {
    match status {
        Status::Passed => "passed",
        Status::Failed => "failed",
        Status::NotRun(_) => "not run",
        Status::Error(_) => "error",
    }
}

fn syntax_name(syntax: Syntax) -> &'static str {
    match syntax {
        Syntax::Ubl => "UBL",
        Syntax::Cii => "UN/CEFACT CII",
        Syntax::CiiLegacy => "UN/CEFACT CII (ZUGFeRD 1.0)",
        Syntax::FatturaPa => "FatturaPA",
        Syntax::OrderX => "Order-X",
    }
}

/// The standard, its version and its profile in a few words: `XRechnung 3.0`, `Factur-X / ZUGFeRD 2.1+ (EN 16931)`.
fn standard_line(d: &Detection) -> String {
    let mut line = if d.standard == Standard::Unspecified { tl!("no standard named").to_owned() } else { d.standard.name().to_owned() };
    if let Some(v) = &d.version {
        line.push(' ');
        line.push_str(v);
    }
    if let Some(p) = &d.profile {
        line.push_str(&format!(" ({p})"));
    }
    line
}

/// What a stage is called in the list.
fn stage_name(stage: Stage, detection: Option<&Detection>) -> String {
    match stage {
        Stage::WellFormedness => tl!("Well-formed").to_owned(),
        Stage::Detection => tl!("Detection").to_owned(),
        Stage::Schema => tl!("XSD").to_owned(),
        Stage::En16931 => tl!("EN 16931").to_owned(),
        Stage::Cius => match detection.map(|d| d.standard) {
            Some(Standard::XRechnung | Standard::XRechnungExtension) => tl!("CIUS (XRechnung)").to_owned(),
            Some(Standard::PeppolBis) => tl!("CIUS (Peppol BIS)").to_owned(),
            _ => tl!("CIUS").to_owned(),
        },
    }
}

/// The chip of a stage: its tone and its words.
fn stage_chip(report: &xmlcraft_einvoice::validate::StageReport) -> (Tone, String) {
    match &report.status {
        Status::Passed => (Tone::Good, tl!("passed").to_owned()),
        Status::Failed => {
            let failed = report.totals.map_or(report.findings, |t| t.failed);
            (Tone::Bad, fmt(tl!("{count} failed"), &[("count", &failed.to_string())]))
        }
        Status::NotRun(_) => (Tone::Neutral, tl!("not run").to_owned()),
        Status::Error(_) => (Tone::Warn, tl!("error").to_owned()),
    }
}

fn severity_word(severity: Severity) -> &'static str {
    match severity {
        Severity::Fatal => tl!("fatal"),
        Severity::Warning => tl!("warning"),
        Severity::Information => tl!("information"),
    }
}

/// The report as plain text, for "Copy report".
pub(crate) fn report_text(name: &str, report: &Report) -> String {
    let mut out = fmt(tl!("E-invoice validation of {name}"), &[("name", name)]);
    out.push('\n');
    if let Some(d) = &report.detection {
        out.push_str(&format!("{}: {}, {}\n", d.document, syntax_name(d.syntax), standard_line(d)));
    }
    out.push_str(if report.valid() { tl!("Valid: no stage that ran failed.") } else { tl!("Not valid.") });
    out.push('\n');
    for s in &report.stages {
        let name = stage_name(s.stage, report.detection.as_ref());
        let (_, chip) = stage_chip(s);
        match &s.status {
            Status::NotRun(why) | Status::Error(why) => out.push_str(&format!("{name}: {chip} ({why})\n")),
            _ => match &s.against {
                Some(against) => out.push_str(&format!("{name}: {chip} ({against})\n")),
                None => out.push_str(&format!("{name}: {chip}\n")),
            },
        }
    }
    out.push_str(&counts_line(report));
    out.push('\n');
    out.push_str(&fmt(tl!("Findings: {count}"), &[("count", &report.findings_total.to_string())]));
    out.push('\n');
    for f in &report.findings {
        let rule = f.rule.as_deref().unwrap_or("-");
        out.push_str(&format!("  [{}] {rule} (line {}, column {}): {}\n", severity_word(f.severity), f.line, f.column, f.message));
    }
    if report.findings_total > report.findings.len() {
        out.push_str(&fmt(tl!("  and {more} more"), &[("more", &(report.findings_total - report.findings.len()).to_string())]));
        out.push('\n');
    }
    if report.not_evaluated_total > 0 {
        out.push_str(&fmt(tl!("Not evaluated: {count}"), &[("count", &report.not_evaluated_total.to_string())]));
        out.push('\n');
        for n in &report.not_evaluated {
            out.push_str(&format!("  {}: {}\n", n.rule.as_deref().unwrap_or(&n.test), n.reason));
        }
    }
    out
}

/// "N checks: X fired, Y passed, Z failed, W not evaluated".
fn counts_line(report: &Report) -> String {
    let t = &report.totals;
    fmt(
        tl!("Rules: {checks} checks, {fired} fired, {passed} passed, {failed} failed, {not} not evaluated"),
        &[
            ("checks", &t.checks.to_string()),
            ("fired", &t.checks.saturating_sub(t.not_fired).to_string()),
            ("passed", &t.passed.to_string()),
            ("failed", &t.failed.to_string()),
            ("not", &t.not_evaluated.to_string()),
        ],
    )
}

/// Where a hybrid PDF of the document goes unless another place is chosen: beside it, `<name>.pdf` (`<name>-hybrid.pdf` for a PDF
/// document, which is not to be written over).
fn suggested_out(path: Option<&Path>, name: &str) -> String {
    let file = path.and_then(Path::file_name).map_or_else(|| name.to_owned(), |n| n.to_string_lossy().into_owned());
    let (stem, ext) = file.rsplit_once('.').unwrap_or((file.as_str(), ""));
    let out = if ext.eq_ignore_ascii_case("pdf") { format!("{stem}-hybrid.pdf") } else { format!("{stem}.pdf") };
    match path.and_then(Path::parent) {
        Some(dir) => dir.join(out).to_string_lossy().into_owned(),
        None => out,
    }
}

/// A finding passes the filter when its rule or its message has the text (any case).
fn admits(filter: &str, rule: Option<&str>, message: &str) -> bool {
    let wanted = filter.trim().to_lowercase();
    wanted.is_empty() || rule.is_some_and(|r| r.to_lowercase().contains(&wanted)) || message.to_lowercase().contains(&wanted)
}

/// At most [`VALUE_CHARS`] characters of `text`, on one line.
fn shown(text: &str) -> String {
    let flat: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let mut out: String = flat.chars().take(VALUE_CHARS).collect();
    if flat.chars().count() > VALUE_CHARS {
        out.push('…');
    }
    out
}

/// What a field says: its date in ISO form, an amount or quantity with its currency or unit.
fn field_text(f: &Field) -> String {
    match (&f.date, &f.unit) {
        (Some(_), _) => f.normalized(),
        (None, Some(unit)) if f.decimal.is_some() => format!("{} {unit}", shown(&f.text)),
        _ => shown(&f.text),
    }
}

impl XmlCraftApp {
    /// E-Rechnung…: show the tool window.
    pub(crate) fn open_einvoice(&mut self) {
        self.show_tool(Panel::EInvoice);
    }

    /// Validate Invoice: show the tool window on its Validate tab and validate (what F8 does).
    pub(crate) fn validate_invoice(&mut self) {
        if self.show_tool(Panel::EInvoice) {
            self.einvoice.tab = Tab::Validate;
        }
        self.validate_now();
    }

    /// F8 asked for the validation of an invoice: the tool waits for it.
    pub(crate) fn invoice_validation_asked(&mut self, key: Key) {
        self.einvoice.waiting = Some(key);
    }

    /// A validation of document `id` at `version` arrived, with the report of its invoice rules when it is an invoice. Called for every
    /// validation, whatever is shown.
    pub(crate) fn invoice_validated(&mut self, id: u64, version: u64, report: Option<Box<Report>>) {
        let waited = self.einvoice.waiting.is_some_and(|(doc, v)| doc == id && version >= v);
        if waited {
            self.einvoice.waiting = None;
        }
        if let Some(report) = report {
            self.einvoice.report = Some(((id, version), report));
            self.workbench.set_attention(Panel::EInvoice);
        }
    }

    /// Work out the overview on a thread.
    fn start_reading(&mut self, key: Key) {
        let Some(doc) = self.session.active_document() else { return };
        let text = doc.editor().text();
        if text.len() > MAX_BYTES {
            let limit = (MAX_BYTES / (1024 * 1024)).to_string();
            self.einvoice.model.shown = Some((key, Err(fmt(tl!("The document is larger than {limit} MB."), &[("limit", &limit)]))));
            return;
        }
        let serial = self.next_http_serial();
        let started = self.spawn_http(move || {
            let result =
                xmlcraft_engine::guard(|| Reading { detection: xmlcraft_einvoice::detect(&text), invoice: xmlcraft_einvoice::model::read(&text) })
                    .map(Box::new)
                    .map_err(|_| tl!("Reading the invoice failed (a bug); nothing was changed.").to_owned());
            Outcome::InvoiceReading { serial, result }
        });
        match started {
            Ok(()) => self.einvoice.model.waiting = Some((serial, key)),
            Err(why) => self.einvoice.model.shown = Some((key, Err(why))),
        }
    }

    /// The answer of an overview thread, whatever is shown. An answer for an older request is dropped.
    pub(crate) fn invoice_read(&mut self, serial: u64, result: Result<Box<Reading>, String>) {
        let model = &mut self.einvoice.model;
        let Some((_, key)) = model.waiting.take_if(|(s, _)| *s == serial) else { return };
        model.shown = Some((key, result.map(|r| *r)));
    }

    /// A template file chosen with "Choose…".
    pub(crate) fn picked_invoice_template(&mut self, path: &Path) {
        self.einvoice.pdf.template_file = Some(path.to_path_buf());
        self.einvoice.pdf.template = Template::File;
    }

    /// Where the PDF goes, chosen with the save panel (which asked about a file that is there).
    pub(crate) fn picked_invoice_out(&mut self, path: &Path) {
        self.einvoice.pdf.out = path.to_string_lossy().into_owned();
        self.einvoice.pdf.confirm = None;
    }

    /// Make the hybrid PDF with the form; `replace`: the person said a file that is there may be replaced.
    fn make_pdf(&mut self, replace: bool) {
        let out_text = self.einvoice.pdf.out.trim().to_owned();
        if out_text.is_empty() {
            self.einvoice.pdf.confirm = None;
            self.status = tl!("Give the PDF to write.").to_owned();
            return;
        }
        let out = PathBuf::from(out_text);
        let Some(doc) = self.session.active_document() else { return };
        let key = (doc.id(), doc.editor().version());
        let itself = doc.path().is_some_and(|p| p == out);
        let lossy = doc.is_lossy();
        // The XML embedded: the document's text in its encoding (for a file that was not edited, its bytes as they are on disk).
        let bytes = doc.to_bytes().map_err(|e| e.to_string());
        let text = doc.editor().text();
        let template = self.einvoice.pdf.template;
        let template_file = self.einvoice.pdf.template_file.clone().filter(|_| template == Template::File);
        let refused = if itself {
            Some(tl!("That is the document itself: choose another file.").to_owned())
        } else if lossy {
            Some(tl!("The document has bytes that are not valid in its encoding; it would not be embedded unchanged.").to_owned())
        } else if template == Template::File && template_file.is_none() {
            Some(tl!("Choose a template file, or one of the built-in templates.").to_owned())
        } else {
            None
        };
        if let Some(why) = refused {
            self.einvoice.pdf.result = Some((key, Err(why)));
            return;
        }
        if !replace && out.exists() {
            self.einvoice.pdf.confirm = Some(out);
            return;
        }
        self.einvoice.pdf.confirm = None;
        let bytes = match bytes {
            Ok(b) => b,
            Err(why) => {
                self.einvoice.pdf.result = Some((key, Err(fmt(tl!("The XML cannot be written in its encoding: {why}"), &[("why", &why)]))));
                return;
            }
        };
        let serial = self.next_http_serial();
        let started = self.spawn_http(move || {
            let result = xmlcraft_engine::guard(|| render(&text, &bytes, template, template_file.as_deref(), &out, replace))
                .unwrap_or_else(|_| Err(tl!("Making the PDF failed (a bug); nothing was written.").to_owned()));
            Outcome::InvoicePdf { serial, result: Box::new(result) }
        });
        match started {
            Ok(()) => {
                self.einvoice.pdf.waiting = Some(serial);
                self.einvoice.pdf.result = None;
            }
            Err(why) => self.einvoice.pdf.result = Some((key, Err(why))),
        }
    }

    /// The answer of a rendering thread, whatever is shown.
    pub(crate) fn invoice_pdf_made(&mut self, serial: u64, result: Result<Made, String>) {
        if self.einvoice.pdf.waiting != Some(serial) {
            return;
        }
        self.einvoice.pdf.waiting = None;
        let key = self.session.active_document().map_or((0, 0), |d| (d.id(), d.editor().version()));
        let open = match &result {
            Ok(made) if self.einvoice.pdf.open_result => Some(made.path.clone()),
            _ => None,
        };
        if let Ok(made) = &result {
            self.status = fmt(tl!("Wrote {path}"), &[("path", &made.path.display().to_string())]);
        }
        self.einvoice.pdf.result = Some((key, result));
        self.workbench.set_attention(Panel::EInvoice);
        if let Some(path) = open {
            self.open_path(&path);
        }
    }

    /// Save the XML of the document being shown (a PDF's embedded invoice) as a file of its own.
    pub(crate) fn save_invoice_xml(&mut self, path: &Path) {
        let Some(doc) = self.session.active_document() else { return };
        let written =
            doc.to_bytes().map_err(|e| e.to_string()).and_then(|bytes| xmlcraft_platform::files::write_file(path, &bytes).map_err(|e| e.to_string()));
        self.status = match written {
            Ok(_) => fmt(tl!("Saved the XML as {path}"), &[("path", &path.display().to_string())]),
            Err(why) => fmt(tl!("The XML was not saved: {why}"), &[("why", &why)]),
        };
    }
}

/// Make the hybrid PDF of `text` (whose bytes `bytes` are embedded) and write it to `out` (a file that is there only when `replace`).
fn render(text: &str, bytes: &[u8], template: Template, template_file: Option<&Path>, out: &Path, replace: bool) -> Result<Made, String> {
    let template_text = match template_file {
        Some(path) => {
            let loaded = xmlcraft_platform::files::read_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
            Some(
                String::from_utf8(loaded.bytes)
                    .map_err(|_| fmt(tl!("The template {path} is not UTF-8."), &[("path", &path.display().to_string())]))?,
            )
        }
        None => None,
    };
    let source = match (template, template_text.as_deref()) {
        (_, Some(t)) => xmlcraft_invoice_pdf::TemplateSource::Text(t),
        (Template::English, None) => xmlcraft_invoice_pdf::TemplateSource::En,
        _ => xmlcraft_invoice_pdf::TemplateSource::De,
    };
    // (Asked again here: a file may have appeared since the form asked.)
    if std::fs::symlink_metadata(out).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(fmt(tl!("{path} is a symbolic link, and is not written through."), &[("path", &out.display().to_string())]));
    }
    if !replace && out.exists() {
        return Err(fmt(tl!("{path} is there already; nothing was written."), &[("path", &out.display().to_string())]));
    }
    let time = xmlcraft_resolve::Clock::now(&xmlcraft_platform::network::SystemClock);
    let options = xmlcraft_invoice_pdf::Options { template: source, time, icc_profile: None };
    let rendered = xmlcraft_engine::einvoice::einvoice_render_pdf(text, bytes, &options)
        .map_err(|why| fmt(tl!("Making the PDF failed: {why}"), &[("why", &why)]))?
        .map_err(|e| e.to_string())?;
    xmlcraft_platform::files::write_file(out, &rendered.bytes).map_err(|e| format!("{}: {e}", out.display()))?;
    let f = rendered.facturx;
    Ok(Made {
        path: out.to_path_buf(),
        pages: rendered.pages,
        bytes: rendered.bytes.len(),
        hybrid: f.hybrid,
        profile: f.profile,
        file_name: f.file_name,
        relationship: f.relationship,
        reason: f.reason,
        pdfa: rendered.pdfa,
        pdfa_reason: rendered.pdfa_reason,
        warnings: rendered.warnings,
    })
}

/// What a frame of the tool asked for.
#[derive(Default)]
struct Asked {
    jump: Option<Range<usize>>,
    validate: bool,
    make: Option<bool>,
    pick_template: bool,
    pick_out: bool,
    save_xml: bool,
    copy: Option<String>,
}

/// What the PDF tab knows of the document.
struct DocFacts {
    name: String,
    path: Option<PathBuf>,
    /// For a PDF document: the embedded file, its relationship, its size and the size of the PDF.
    pdf: Option<(String, Option<String>, usize, usize)>,
}

/// The body of the E-Rechnung panel.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    let bound = tools::bound(app);
    let invoice = app.active_bar_kind(ui.ctx()) == Some(crate::bar_actions::BarKind::Invoice);
    let facts = app.session.active_document().map(|d| DocFacts {
        name: d.name().to_owned(),
        path: d.path().map(Path::to_path_buf),
        pdf: d.pdf_embedded().map(|e| (e.name.to_owned(), e.relationship.map(str::to_owned), e.xml_bytes, e.pdf_bytes)),
    });
    let key = app.session.active_document().map(|d| (d.id(), d.editor().version()));
    let (Some(bound), Some(key), Some(facts)) = (bound, key, facts) else {
        kit::tool_body(ui, false, |ui| {
            tools::needs(
                ui,
                tl!("E-Rechnung needs an invoice"),
                tl!("Open an XRechnung, ZUGFeRD / Factur-X, Peppol, UBL or CII invoice, or a PDF that carries one."),
            )
        });
        return;
    };
    if !invoice {
        kit::tool_body(ui, false, |ui| {
            tools::bound_line(ui, &bound.name);
            tools::needs(
                ui,
                tl!("This document is no e-invoice"),
                tl!("E-Rechnung works on UBL and CII invoices and credit notes, FatturaPA, and PDFs that carry one."),
            );
        });
        return;
    }
    // A new document gets its own suggestion of where its PDF goes.
    if app.einvoice.pdf.out_for != Some(key.0) {
        app.einvoice.pdf.out_for = Some(key.0);
        app.einvoice.pdf.out = suggested_out(facts.path.as_deref(), &facts.name);
        app.einvoice.pdf.confirm = None;
    }
    // The overview: a new text starts the wait for a quiet moment; when it is over, the thread starts.
    let now = ui.input(|i| i.time);
    let model = &mut app.einvoice.model;
    if model.key != Some(key) {
        model.key = Some(key);
        model.due = Some(now + DEBOUNCE);
    }
    if let Some(due) = model.due {
        if now >= due && model.waiting.is_none() {
            model.due = None;
            app.start_reading(key);
        } else if now < due {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(due - now));
        }
    }
    let mut asked = Asked::default();
    let tab = app.einvoice.tab;
    let footer = match tab {
        Tab::Overview => None,
        Tab::Validate => Some((tl!("Validate invoice"), "receipt-text", app.einvoice.waiting.is_none(), tl!("The invoice is being validated."))),
        Tab::Pdf => Some((tl!("Make hybrid PDF"), "file-text", app.einvoice.pdf.waiting.is_none(), tl!("The PDF is being made."))),
    };
    if let Some((label, icon, enabled, why_not)) = footer
        && kit::tool_footer(ui, label, Some(icon), enabled, why_not)
    {
        match tab {
            Tab::Validate => asked.validate = true,
            _ => asked.make = Some(false),
        }
    }
    let tool = &mut app.einvoice;
    kit::tool_body(ui, false, |ui| {
        tools::bound_line(ui, &bound.name);
        ui.add_space(kit::GAP);
        kit::segmented(ui, &mut tool.tab, &[(Tab::Overview, tl!("Overview")), (Tab::Validate, tl!("Validate")), (Tab::Pdf, tl!("PDF"))]);
        ui.add_space(kit::GAP);
        match tool.tab {
            Tab::Overview => overview(ui, tool, key, &mut asked),
            Tab::Validate => validate(ui, tool, key, &facts.name, &mut asked),
            Tab::Pdf => pdf(ui, tool, key, &facts, &mut asked),
        }
    });
    if let Some(range) = asked.jump {
        app.jump_to(range);
    }
    if asked.validate {
        app.validate_now();
    }
    if let Some(replace) = asked.make {
        app.make_pdf(replace);
    }
    if asked.pick_template {
        app.pick_invoice_template(ui.ctx());
    }
    if asked.pick_out {
        let suggested = Path::new(app.einvoice.pdf.out.trim()).to_path_buf();
        app.pick_invoice_out(ui.ctx(), &suggested);
    }
    if asked.save_xml {
        app.pick_invoice_xml(ui.ctx());
    }
    if let Some(text) = asked.copy {
        ui.ctx().copy_text(text);
        app.status = tl!("Copied the validation report").to_owned();
    }
}

/// Side by side in a wide panel (each with its own scroll area), else one scroll area with the first above the second.
fn split(ui: &mut egui::Ui, id: &str, first: impl FnOnce(&mut egui::Ui), second: impl FnOnce(&mut egui::Ui)) {
    if ui.available_width() < tools::TWO_COLUMNS {
        egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]).show(ui, |ui| {
            first(ui);
            second(ui);
        });
        return;
    }
    tools::columns(
        ui,
        &mut (),
        |ui, _| {
            egui::ScrollArea::vertical().id_salt((id, 0)).auto_shrink([false, false]).show(ui, first);
        },
        |ui, _| {
            egui::ScrollArea::vertical().id_salt((id, 1)).auto_shrink([false, false]).show(ui, second);
        },
    );
}

fn caption(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new(text).font(theme::medium(11.5)).color(t.text_muted));
}

/// A value of the invoice: a click selects it in the text when `live` (the reading is of the text as it is). Missing: a faint dash.
fn value(ui: &mut egui::Ui, field: Option<&Field>, live: bool, strong: bool, jump: &mut Option<Range<usize>>) {
    match field {
        Some(f) => {
            if let Some(range) = text_value(ui, &field_text(f), live.then_some(&f.range), strong) {
                *jump = Some(range);
            }
        }
        None => missing(ui),
    }
}

/// A value that is not in the text (what was worked out from it): not clickable.
fn label_value(ui: &mut egui::Ui, text: &str) {
    let _ = text_value(ui, text, None, false);
}

fn missing(ui: &mut egui::Ui) {
    let t = Tokens::get(ui.ctx());
    ui.label(egui::RichText::new("—").font(theme::regular(12.5)).color(t.text_faint));
}

/// A text that selects `range` when clicked (when there is one). Returns the range when it was clicked.
fn text_value(ui: &mut egui::Ui, text: &str, range: Option<&Range<usize>>, strong: bool) -> Option<Range<usize>> {
    let t = Tokens::get(ui.ctx());
    let font = if strong { theme::semibold(13.0) } else { theme::regular(12.5) };
    let galley = ui.painter().layout(text.to_owned(), font, t.text, ui.available_width().max(24.0));
    let sense = if range.is_some() { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(galley.size(), sense);
    let clickable = range.is_some();
    response.widget_info(|| egui::WidgetInfo::labeled(if clickable { egui::Role::Button } else { egui::Role::Label }, true, text));
    if clickable && response.hovered() {
        ui.painter().rect_filled(rect.expand2(egui::vec2(3.0, 1.0)), 3.0, t.hover);
    }
    ui.painter().galley(rect.min, galley, if clickable { t.text } else { t.text_muted });
    if clickable && theme::hand(response).on_hover_text(tl!("Select it in the text")).clicked() {
        return range.cloned();
    }
    None
}

/// A form of `label: value` rows.
fn rows(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id).num_columns(2).spacing(egui::vec2(12.0, 4.0)).show(ui, add);
}

fn row(ui: &mut egui::Ui, label: &str, field: Option<&Field>, live: bool, jump: &mut Option<Range<usize>>) {
    caption(ui, label);
    value(ui, field, live, false, jump);
    ui.end_row();
}

/// The Overview tab.
fn overview(ui: &mut egui::Ui, tool: &EInvoiceTool, key: Key, asked: &mut Asked) {
    let state = &tool.model;
    let updating = state.waiting.is_some() || state.due.is_some() || state.shown.as_ref().is_none_or(|(k, _)| *k != key);
    // Another document's reading is not shown; one of an older text is, without its clicks.
    let shown = state.shown.as_ref().filter(|(k, _)| k.0 == key.0);
    if updating {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Reading the invoice…"));
        });
    }
    let Some((at, shown)) = shown else { return };
    let live = *at == key;
    let reading = match shown {
        Err(message) => {
            kit::banner(ui, Tone::Bad, message);
            return;
        }
        Ok(reading) => reading,
    };
    if !live {
        kit::help(ui, tl!("The text changed since: the values are updated in a moment."));
    }
    let (mut first, mut second) = (None, None);
    split(
        ui,
        "xmlcraft-einvoice-overview",
        |ui| card(ui, reading, live, &mut first),
        |ui| match &reading.invoice {
            Some(invoice) => readable(ui, invoice, live, &mut second),
            None => {
                ui.add_space(kit::GAP);
                kit::help(ui, tl!("The readable view is not available for this syntax yet: the invoice model reads UBL and UN/CEFACT CII invoices."));
            }
        },
    );
    asked.jump = first.or(second);
}

/// The summary card.
fn card(ui: &mut egui::Ui, reading: &Reading, live: bool, jump: &mut Option<Range<usize>>) {
    let inv = reading.invoice.as_ref();
    kit::card(ui, false, |ui| {
        rows(ui, "xmlcraft-einvoice-card", |ui| {
            caption(ui, tl!("Syntax"));
            match &reading.detection {
                Some(d) => {
                    let version = d.syntax_version.as_deref().map(|v| format!(" {v}")).unwrap_or_default();
                    label_value(ui, &format!("{}{version} · {}", syntax_name(d.syntax), d.document));
                }
                None => missing(ui),
            }
            ui.end_row();
            caption(ui, tl!("Standard"));
            match &reading.detection {
                Some(d) => label_value(ui, &standard_line(d)),
                None => missing(ui),
            }
            ui.end_row();
            caption(ui, tl!("Number"));
            value(ui, inv.and_then(|i| i.number.as_ref()), live, true, jump);
            ui.end_row();
            row(ui, tl!("Issued"), inv.and_then(|i| i.issue_date.as_ref()), live, jump);
            row(ui, tl!("Due"), inv.and_then(|i| i.due_date.as_ref()), live, jump);
            caption(ui, tl!("Seller → buyer"));
            ui.vertical(|ui| {
                value(ui, inv.and_then(|i| i.seller.name.as_ref()), live, false, jump);
                ui.horizontal(|ui| {
                    ui.label("→");
                    value(ui, inv.and_then(|i| i.buyer.name.as_ref()), live, false, jump);
                });
            });
            ui.end_row();
            row(ui, tl!("Currency"), inv.and_then(|i| i.currency.as_ref()), live, jump);
            caption(ui, tl!("Amount due"));
            value(ui, inv.and_then(|i| i.totals.payable.as_ref()), live, true, jump);
            ui.end_row();
            caption(ui, tl!("Lines"));
            match inv {
                Some(i) => label_value(ui, &i.lines_total.to_string()),
                None => missing(ui),
            }
            ui.end_row();
        });
    });
}

/// The invoice as a person reads it.
fn readable(ui: &mut egui::Ui, inv: &Invoice, live: bool, jump: &mut Option<Range<usize>>) {
    kit::section(ui, tl!("Parties"), |ui| {
        let wide = ui.available_width() >= 520.0;
        let party_card = |ui: &mut egui::Ui, title: &str, id: &str, p: &Party, jump: &mut Option<Range<usize>>| {
            kit::card(ui, false, |ui| {
                caption(ui, title);
                party(ui, id, p, live, jump);
            });
        };
        if wide {
            ui.columns(2, |cols| {
                if let [left, right] = cols {
                    party_card(left, tl!("Seller"), "xmlcraft-einvoice-seller", &inv.seller, jump);
                    party_card(right, tl!("Buyer"), "xmlcraft-einvoice-buyer", &inv.buyer, jump);
                }
            });
        } else {
            party_card(ui, tl!("Seller"), "xmlcraft-einvoice-seller", &inv.seller, jump);
            ui.add_space(kit::GAP);
            party_card(ui, tl!("Buyer"), "xmlcraft-einvoice-buyer", &inv.buyer, jump);
        }
    });
    kit::section(ui, tl!("Lines"), |ui| {
        egui::ScrollArea::horizontal().id_salt("xmlcraft-einvoice-lines").show(ui, |ui| {
            egui::Grid::new("xmlcraft-einvoice-lines-grid").num_columns(6).striped(true).spacing(egui::vec2(12.0, 4.0)).show(ui, |ui| {
                for head in [tl!("Pos."), tl!("Item"), tl!("Quantity"), tl!("Unit price"), tl!("VAT %"), tl!("Amount")] {
                    caption(ui, head);
                }
                ui.end_row();
                for line in inv.lines.iter().take(LINES_SHOWN) {
                    value(ui, line.id.as_ref(), live, false, jump);
                    value(ui, line.name.as_ref(), live, false, jump);
                    value(ui, line.quantity.as_ref(), live, false, jump);
                    value(ui, line.net_price.as_ref(), live, false, jump);
                    value(ui, line.vat_rate.as_ref(), live, false, jump);
                    value(ui, line.line_total.as_ref(), live, false, jump);
                    ui.end_row();
                }
            });
        });
        let listed = inv.lines.len().min(LINES_SHOWN);
        if inv.lines_total > listed {
            kit::help(
                ui,
                &fmt(
                    tl!("The first {shown} of {total} lines are listed."),
                    &[("shown", &listed.to_string()), ("total", &inv.lines_total.to_string())],
                ),
            );
        }
    });
    if !inv.vat_breakdown.is_empty() {
        kit::section(ui, tl!("VAT breakdown"), |ui| {
            egui::Grid::new("xmlcraft-einvoice-vat").num_columns(4).spacing(egui::vec2(12.0, 4.0)).show(ui, |ui| {
                for head in [tl!("Category"), tl!("Rate"), tl!("Taxable amount"), tl!("VAT")] {
                    caption(ui, head);
                }
                ui.end_row();
                for r in &inv.vat_breakdown {
                    value(ui, r.category.as_ref(), live, false, jump);
                    value(ui, r.rate.as_ref(), live, false, jump);
                    value(ui, r.taxable.as_ref(), live, false, jump);
                    value(ui, r.tax.as_ref(), live, false, jump);
                    ui.end_row();
                }
            });
        });
    }
    kit::section(ui, tl!("Totals"), |ui| {
        let t = &inv.totals;
        rows(ui, "xmlcraft-einvoice-totals", |ui| {
            row(ui, tl!("Sum of the lines"), t.line_total.as_ref(), live, jump);
            row(ui, tl!("Allowances"), t.allowance_total.as_ref(), live, jump);
            row(ui, tl!("Charges"), t.charge_total.as_ref(), live, jump);
            row(ui, tl!("Total without VAT"), t.tax_exclusive.as_ref(), live, jump);
            row(ui, tl!("VAT"), t.tax_total.as_ref(), live, jump);
            row(ui, tl!("Total with VAT"), t.tax_inclusive.as_ref(), live, jump);
            row(ui, tl!("Paid in advance"), t.prepaid.as_ref(), live, jump);
            row(ui, tl!("Rounding"), t.rounding.as_ref(), live, jump);
            caption(ui, tl!("Amount due"));
            value(ui, t.payable.as_ref(), live, true, jump);
            ui.end_row();
        });
    });
    kit::section(ui, tl!("Payment"), |ui| {
        let p = &inv.payment;
        rows(ui, "xmlcraft-einvoice-payment", |ui| {
            row(ui, tl!("Means (code)"), p.means_code.as_ref(), live, jump);
            row(ui, tl!("IBAN"), p.iban.as_ref(), live, jump);
            row(ui, tl!("BIC"), p.bic.as_ref(), live, jump);
            row(ui, tl!("Account name"), p.account_name.as_ref(), live, jump);
            row(ui, tl!("Reference"), p.reference.as_ref(), live, jump);
            row(ui, tl!("Mandate"), p.mandate.as_ref(), live, jump);
            row(ui, tl!("Terms"), inv.payment_terms.as_ref(), live, jump);
            row(ui, tl!("Buyer reference"), inv.buyer_reference.as_ref(), live, jump);
        });
    });
    if !inv.notes.is_empty() {
        kit::section(ui, tl!("Notes"), |ui| {
            for note in &inv.notes {
                value(ui, Some(note), live, false, jump);
            }
        });
    }
    if !inv.cut.is_empty() {
        ui.add_space(kit::GAP);
        kit::help(ui, &fmt(tl!("Shortened: {what}"), &[("what", &inv.cut.join(", "))]));
    }
}

fn party(ui: &mut egui::Ui, id: &str, p: &Party, live: bool, jump: &mut Option<Range<usize>>) {
    value(ui, p.name.as_ref(), live, true, jump);
    if p.trading_name.is_some() {
        value(ui, p.trading_name.as_ref(), live, false, jump);
    }
    address(ui, &p.address, live, jump);
    rows(ui, id, |ui| {
        row(ui, tl!("VAT ID"), p.vat_id.as_ref(), live, jump);
        if p.legal_id.is_some() {
            row(ui, tl!("Register"), p.legal_id.as_ref(), live, jump);
        }
        if p.contact.name.is_some() {
            row(ui, tl!("Contact"), p.contact.name.as_ref(), live, jump);
        }
        if p.contact.email.is_some() {
            row(ui, tl!("E-mail"), p.contact.email.as_ref(), live, jump);
        }
        if p.contact.phone.is_some() {
            row(ui, tl!("Phone"), p.contact.phone.as_ref(), live, jump);
        }
        if p.electronic_address.is_some() {
            row(ui, tl!("Electronic address"), p.electronic_address.as_ref(), live, jump);
        }
    });
}

fn address(ui: &mut egui::Ui, a: &Address, live: bool, jump: &mut Option<Range<usize>>) {
    if a.is_empty() {
        missing(ui);
        return;
    }
    for line in &a.lines {
        value(ui, Some(line), live, false, jump);
    }
    ui.horizontal_wrapped(|ui| {
        for f in [a.postcode.as_ref(), a.city.as_ref(), a.subdivision.as_ref(), a.country.as_ref()].into_iter().flatten() {
            value(ui, Some(f), live, false, jump);
        }
    });
}

/// The Validate tab: the stages (and the report to copy) beside or above the findings, which fill the rest of the panel.
fn validate(ui: &mut egui::Ui, tool: &mut EInvoiceTool, key: Key, name: &str, asked: &mut Asked) {
    if tool.waiting.is_some_and(|(doc, _)| doc == key.0) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Validating… the rules of a long invoice take a few seconds."));
        });
    }
    let EInvoiceTool { report, filter, waiting, .. } = tool;
    let Some(((_, version), report)) = report.as_ref().filter(|((doc, _), _)| *doc == key.0) else {
        if waiting.is_none() {
            kit::help(
                ui,
                tl!(
                    "Validate checks the invoice stage by stage: its XML Schema, the EN 16931 rules and those of the CIUS it claims (F8 does the same)."
                ),
            );
        }
        return;
    };
    let live = *version == key.1;
    if !live {
        kit::banner(ui, Tone::Warn, tl!("The document changed since: validate again."));
        ui.add_space(kit::GAP);
    }
    let mut copy = None;
    let mut left = |ui: &mut egui::Ui| {
        stages(ui, report);
        ui.add_space(kit::GAP);
        if ui.button(tl!("Copy report")).clicked() {
            copy = Some(report_text(name, report));
        }
        not_evaluated(ui, report);
    };
    let jump = if ui.available_width() < tools::TWO_COLUMNS {
        left(ui);
        ui.add_space(kit::GAP);
        findings(ui, filter, report, live)
    } else {
        tools::columns(ui, &mut (), |ui, _| left(ui), |ui, _| findings(ui, filter, report, live)).1
    };
    asked.jump = jump;
    asked.copy = copy;
}

fn stages(ui: &mut egui::Ui, report: &Report) {
    for s in &report.stages {
        let name = stage_name(s.stage, report.detection.as_ref());
        let (tone, chip) = stage_chip(s);
        ui.horizontal(|ui| {
            let t = Tokens::get(ui.ctx());
            ui.label(egui::RichText::new(&name).font(theme::medium(12.5)).color(t.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                kit::chip(ui, tone, None, &chip, false);
            });
        });
        match &s.status {
            Status::NotRun(why) | Status::Error(why) => kit::help(ui, why),
            _ => {
                if let Some(against) = &s.against {
                    kit::help(ui, against);
                }
            }
        }
    }
    ui.add_space(kit::GAP);
    kit::help(ui, &counts_line(report));
}

fn not_evaluated(ui: &mut egui::Ui, report: &Report) {
    if report.not_evaluated_total == 0 {
        return;
    }
    ui.add_space(kit::GAP);
    let title = fmt(tl!("{count} rules not evaluated"), &[("count", &report.not_evaluated_total.to_string())]);
    egui::CollapsingHeader::new(title).id_salt("xmlcraft-einvoice-not-evaluated").default_open(false).show(ui, |ui| {
        kit::help(ui, tl!("These checks are not counted as passed or failed: the engine could not evaluate them."));
        for n in &report.not_evaluated {
            let rule = n.rule.as_deref().unwrap_or(&n.test);
            kit::help(ui, &format!("{rule}: {}", n.reason));
        }
        if report.not_evaluated_total > report.not_evaluated.len() {
            kit::help(ui, &fmt(tl!("and {more} more"), &[("more", &(report.not_evaluated_total - report.not_evaluated.len()).to_string())]));
        }
    });
}

/// The findings, filling the rest of the panel. Returns the range of the one clicked.
fn findings(ui: &mut egui::Ui, filter: &mut String, report: &Report, live: bool) -> Option<Range<usize>> {
    let response = ui.add(widgets::line(filter).hint_text(tl!("Filter by rule or text")).desired_width(f32::INFINITY));
    widgets::name_for_assistive_technology(ui, &response, tl!("Filter the findings"));
    let listed: Vec<&xmlcraft_einvoice::validate::Finding> =
        report.findings.iter().filter(|f| admits(filter, f.rule.as_deref(), &f.message)).collect();
    let summary = if report.findings_total == 0 {
        tl!("No findings.").to_owned()
    } else if listed.len() == report.findings.len() {
        fmt(tl!("{count} findings. Click one to select its element in the text."), &[("count", &report.findings_total.to_string())])
    } else {
        fmt(tl!("{shown} of {count} findings."), &[("shown", &listed.len().to_string()), ("count", &report.findings_total.to_string())])
    };
    kit::help(ui, &summary);
    let t = Tokens::get(ui.ctx());
    let syntax = crate::editor_view::SyntaxTokens::for_kind(t.kind);
    let mut jump = None;
    egui::ScrollArea::vertical().id_salt("xmlcraft-einvoice-findings").auto_shrink([false, false]).show_rows(ui, ROW, listed.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for f in listed.get(range).unwrap_or_default() {
            let (rect, response) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), egui::Sense::click());
            let rule = f.rule.as_deref().unwrap_or("—");
            let message = xmlcraft_engine::validation::plain_message(rule, &f.message);
            let label = format!("{rule}: {message}");
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, live, label.clone()));
            if live && response.hovered() {
                ui.painter().rect_filled(rect, 4.0, t.hover);
            }
            let color = match f.severity {
                Severity::Fatal => syntax.error,
                Severity::Warning => syntax.warning,
                Severity::Information => syntax.info,
            };
            // The rule id as a chip, then the message.
            let chip = ui.painter().layout_no_wrap(rule.to_owned(), theme::semibold(11.0), color);
            let chip_rect = egui::Rect::from_min_size(rect.left_center() + egui::vec2(4.0, -9.0), egui::vec2(chip.size().x + 12.0, 18.0));
            ui.painter().rect_filled(chip_rect, 9.0, color.gamma_multiply(if t.dark() { 0.22 } else { 0.12 }));
            ui.painter().galley(chip_rect.center() - chip.size() / 2.0, chip, color);
            let text_left = chip_rect.right() + 8.0;
            let text_rect = egui::Rect::from_min_max(egui::pos2(text_left, rect.top()), rect.right_bottom());
            ui.painter().with_clip_rect(text_rect).text(
                egui::pos2(text_left, rect.center().y),
                egui::Align2::LEFT_CENTER,
                shown(&message),
                theme::regular(12.0),
                if live { t.text } else { t.text_muted },
            );
            let place = fmt(tl!("Ln {line}, Col {col}"), &[("line", &f.line.to_string()), ("col", &f.column.to_string())]);
            let response = response.on_hover_text(format!("{} · {place}\n{}", severity_word(f.severity), f.message));
            if live && theme::hand(response).clicked() {
                jump = Some(f.span.clone());
            }
        }
    });
    jump
}

/// The PDF tab.
fn pdf(ui: &mut egui::Ui, tool: &mut EInvoiceTool, key: Key, facts: &DocFacts, asked: &mut Asked) {
    if ui.available_width() < tools::TWO_COLUMNS {
        egui::ScrollArea::vertical().id_salt("xmlcraft-einvoice-pdf").auto_shrink([false, false]).show(ui, |ui| {
            pdf_form(ui, tool, facts, asked);
            pdf_result(ui, tool, key);
        });
        return;
    }
    tools::columns(
        ui,
        tool,
        |ui, tool| {
            egui::ScrollArea::vertical()
                .id_salt(("xmlcraft-einvoice-pdf", 0))
                .auto_shrink([false, false])
                .show(ui, |ui| pdf_form(ui, tool, facts, asked));
        },
        |ui, tool| {
            egui::ScrollArea::vertical().id_salt(("xmlcraft-einvoice-pdf", 1)).auto_shrink([false, false]).show(ui, |ui| pdf_result(ui, tool, key));
        },
    );
}

fn pdf_form(ui: &mut egui::Ui, tool: &mut EInvoiceTool, facts: &DocFacts, asked: &mut Asked) {
    let form = &mut tool.pdf;
    kit::help(ui, tl!("Make a PDF that shows the invoice to people and carries its XML unchanged, byte for byte (ZUGFeRD / Factur-X for CII)."));
    ui.add_space(kit::GAP);
    caption(ui, tl!("Template"));
    kit::segmented(ui, &mut form.template, &[(Template::German, tl!("German")), (Template::English, tl!("English")), (Template::File, tl!("File"))]);
    if form.template == Template::File {
        ui.horizontal(|ui| {
            let name = form
                .template_file
                .as_ref()
                .and_then(|p| p.file_name())
                .map_or_else(|| tl!("No file chosen").to_owned(), |n| n.to_string_lossy().into_owned());
            ui.label(name);
            if ui.button(tl!("Choose…")).clicked() {
                asked.pick_template = true;
            }
        });
        kit::help(ui, tl!("A template in the invoice-template format (blocks, columns, fields, tables and totals with {path|filter} placeholders)."));
    }
    ui.add_space(kit::GAP);
    caption(ui, tl!("Save as"));
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 90.0).max(120.0);
        let response = ui.add(widgets::line(&mut form.out).desired_width(width));
        widgets::name_for_assistive_technology(ui, &response, tl!("The PDF to write"));
        if response.changed() {
            form.confirm = None;
        }
        let choose = ui.button(tl!("Choose…"));
        widgets::name_for_assistive_technology(ui, &choose, tl!("Choose where to save the PDF"));
        if choose.on_hover_text(tl!("Choose where to save the PDF")).clicked() {
            asked.pick_out = true;
        }
    });
    ui.add(widgets::checkbox(&mut form.open_result, tl!("Open the result as a document")));
    kit::help(ui, tl!("A PDF opens as the XML embedded in it."));
    if let Some(path) = form.confirm.clone() {
        ui.add_space(kit::GAP);
        kit::banner(ui, Tone::Warn, &fmt(tl!("{name} is there already."), &[("name", &path.display().to_string())]));
        ui.horizontal(|ui| {
            if kit::pill_sized(ui, kit::PillKind::Primary, None, tl!("Replace"), kit::TOOL_CONTROL).clicked() {
                asked.make = Some(true);
            }
            if kit::pill_sized(ui, kit::PillKind::Ghost, None, tl!("Cancel"), kit::TOOL_CONTROL).clicked() {
                form.confirm = None;
            }
        });
    }
    if let Some((name, relationship, xml_bytes, pdf_bytes)) = &facts.pdf {
        kit::section(ui, tl!("This invoice is inside a PDF"), |ui| {
            rows(ui, "xmlcraft-einvoice-inside", |ui| {
                caption(ui, tl!("PDF"));
                label_value(ui, &fmt(tl!("{name} ({bytes} bytes)"), &[("name", &facts.name), ("bytes", &pdf_bytes.to_string())]));
                ui.end_row();
                caption(ui, tl!("Embedded file"));
                label_value(ui, &fmt(tl!("{name} ({bytes} bytes)"), &[("name", name), ("bytes", &xml_bytes.to_string())]));
                ui.end_row();
                caption(ui, tl!("Relationship"));
                match relationship {
                    Some(r) => label_value(ui, r),
                    None => missing(ui),
                }
                ui.end_row();
            });
            ui.add_space(4.0);
            if ui.button(tl!("Save XML as…")).clicked() {
                asked.save_xml = true;
            }
            kit::help(ui, tl!("Showing the PDF itself (in XmlCraft or in the system's viewer) is not available yet."));
        });
    }
}

fn pdf_result(ui: &mut egui::Ui, tool: &EInvoiceTool, key: Key) {
    let form = &tool.pdf;
    if form.waiting.is_some() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(tl!("Making the PDF…"));
        });
        return;
    }
    let Some((made_from, result)) = &form.result else { return };
    ui.add_space(kit::GAP);
    match result {
        Err(why) => kit::banner(ui, Tone::Bad, why),
        Ok(made) => {
            let name = made.path.file_name().map_or_else(|| made.path.display().to_string(), |n| n.to_string_lossy().into_owned());
            kit::banner(ui, Tone::Good, &fmt(tl!("Wrote {name}"), &[("name", &name)]));
            kit::help(ui, &made.path.display().to_string());
            if made_from.0 == key.0 && made_from.1 != key.1 {
                kit::help(ui, tl!("The document changed since: make it again to carry the new text."));
            }
            ui.add_space(kit::GAP);
            rows(ui, "xmlcraft-einvoice-made", |ui| {
                let line = |ui: &mut egui::Ui, label: &str, text: &str| {
                    caption(ui, label);
                    label_value(ui, text);
                    ui.end_row();
                };
                line(ui, tl!("Pages"), &made.pages.to_string());
                line(ui, tl!("Size"), &fmt(tl!("{bytes} bytes"), &[("bytes", &made.bytes.to_string())]));
                let profile = match (&made.profile, made.hybrid) {
                    (Some(p), true) => fmt(tl!("Factur-X {profile}"), &[("profile", p)]),
                    _ => tl!("not a Factur-X hybrid").to_owned(),
                };
                line(ui, tl!("Profile"), &profile);
                line(ui, tl!("Embedded as"), &format!("{} ({})", made.file_name, made.relationship));
                line(ui, tl!("PDF/A"), if made.pdfa { tl!("says PDF/A-3b") } else { tl!("no") });
            });
            if let Some(reason) = &made.reason {
                kit::help(ui, reason);
            }
            if let Some(reason) = &made.pdfa_reason {
                kit::help(ui, reason);
            }
            for w in &made.warnings {
                kit::help(ui, &fmt(tl!("Warning: {what}"), &[("what", w)]));
            }
            ui.add_space(kit::GAP);
            kit::help(ui, tl!("Not verified with veraPDF / Mustang: check the file with a validator before relying on it."));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pdf_goes_beside_the_document_and_a_pdf_is_not_written_over() {
        assert_eq!(suggested_out(Some(Path::new("/w/rechnung.xml")), "rechnung.xml"), Path::new("/w").join("rechnung.pdf").to_string_lossy());
        assert_eq!(suggested_out(Some(Path::new("/w/r.PDF")), "r.PDF"), Path::new("/w").join("r-hybrid.pdf").to_string_lossy());
        assert_eq!(suggested_out(None, "Untitled-1"), "Untitled-1.pdf");
        assert_eq!(suggested_out(None, "a.b.xml"), "a.b.pdf");
    }

    #[test]
    fn the_filter_looks_at_the_rule_and_the_message_in_any_case() {
        assert!(admits("", None, "x"));
        assert!(admits("br-co", Some("BR-CO-15"), "x"));
        assert!(admits("TOTAL", Some("BR-CO-15"), "Invoice total amount"));
        assert!(!admits("BR-DE", Some("BR-CO-15"), "Invoice total amount"));
        assert!(admits("  ", Some("A"), ""), "white space is no filter");
    }

    #[test]
    fn a_value_is_one_line_of_bounded_length_and_an_amount_has_its_currency() {
        assert_eq!(shown("a\nb"), "a b");
        let long = shown(&"é".repeat(VALUE_CHARS + 5));
        assert!(long.ends_with('…') && long.chars().count() == VALUE_CHARS + 1);
        let amount = Field {
            text: "119.00".into(),
            range: 0..6,
            decimal: xmlcraft_einvoice::model::Decimal::parse("119.00"),
            date: None,
            unit: Some("EUR".into()),
        };
        assert_eq!(field_text(&amount), "119.00 EUR");
        let id = Field { text: "RE-1".into(), range: 0..4, decimal: None, date: None, unit: Some("0088".into()) };
        assert_eq!(field_text(&id), "RE-1", "a scheme is not a unit");
    }

    #[test]
    fn the_report_text_names_every_stage_and_finding() {
        let text = include_str!("../../../einvoice/tests/fixtures/xrechnung-ubl.xml").replace(
            "<cbc:PayableAmount currencyID=\"EUR\">119.00</cbc:PayableAmount>",
            "<cbc:PayableAmount currencyID=\"EUR\">118.00</cbc:PayableAmount>",
        );
        let report = xmlcraft_engine::einvoice::einvoice_validate(&text, "", None, 100).expect("no panic");
        let out = report_text("r.xml", &report);
        assert!(out.starts_with("E-invoice validation of r.xml"), "{out}");
        for word in ["Well-formed: passed", "XSD: passed", "EN 16931: ", "CIUS (XRechnung)", "Not valid."] {
            assert!(out.contains(word), "{word}: {out}");
        }
        assert!(report.findings.iter().all(|f| f.rule.as_deref().is_none_or(|r| out.contains(r))), "{out}");
    }
}
