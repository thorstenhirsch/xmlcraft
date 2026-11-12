//! E-Rechnung, the tool window of electronic invoices (XRechnung, ZUGFeRD / Factur-X, Peppol BIS, plain UBL and CII, FatturaPA). It
//! follows the document being shown and has three tabs: **Overview** (what the invoice says), **Validate** (the stages and the rule
//! findings) and **PDF** (a hybrid PDF made from the invoice; the PDF an invoice was opened from).

use crate::i18n::tl;
use crate::kit;
use crate::tools;
use crate::{Panel, XmlCraftApp};

/// Which part of the tool is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Tab {
    #[default]
    Overview,
    Validate,
    Pdf,
}

/// The state of the E-Rechnung tool window.
#[derive(Debug, Default)]
pub(crate) struct EInvoiceTool {
    tab: Tab,
    /// The validation (F8 or the button) being waited for: the document and the version of its text.
    waiting: Option<(u64, u64)>,
    /// The last validation report of an invoice, and the document and version it was made from.
    report: Option<((u64, u64), Box<xmlcraft_einvoice::validate::Report>)>,
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

    /// F8 asked for a validation that checks an invoice with its rules: the tool waits for it.
    pub(crate) fn invoice_validation_asked(&mut self, key: (u64, u64)) {
        self.einvoice.waiting = Some(key);
    }

    /// A validation of document `id` at `version` arrived, with the report of its invoice rules when it is an invoice. Called for every
    /// validation, whatever is shown.
    pub(crate) fn invoice_validated(&mut self, id: u64, version: u64, report: Option<Box<xmlcraft_einvoice::validate::Report>>) {
        if self.einvoice.waiting.is_some_and(|(doc, v)| doc == id && version >= v) {
            self.einvoice.waiting = None;
        }
        if let Some(report) = report {
            self.einvoice.report = Some(((id, version), report));
            self.workbench.set_attention(Panel::EInvoice);
        }
    }
}

/// The body of the E-Rechnung panel.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut XmlCraftApp) {
    let bound = tools::bound(app);
    let invoice = app.active_bar_kind(ui.ctx()) == Some(crate::bar_actions::BarKind::Invoice);
    kit::tool_body(ui, false, |ui| {
        let Some(bound) = bound else {
            tools::needs(
                ui,
                tl!("E-Rechnung needs an invoice"),
                tl!("Open an XRechnung, ZUGFeRD / Factur-X, Peppol, UBL or CII invoice, or a PDF that carries one."),
            );
            return;
        };
        tools::bound_line(ui, &bound.name);
        if !invoice {
            tools::needs(
                ui,
                tl!("This document is no e-invoice"),
                tl!("E-Rechnung works on UBL and CII invoices and credit notes, FatturaPA, and PDFs that carry one."),
            );
            return;
        }
        ui.add_space(kit::GAP);
        kit::segmented(ui, &mut app.einvoice.tab, &[(Tab::Overview, tl!("Overview")), (Tab::Validate, tl!("Validate")), (Tab::Pdf, tl!("PDF"))]);
        if app.einvoice.tab == Tab::Validate
            && let Some((_, report)) = app.einvoice.report.as_ref().filter(|((doc, _), _)| *doc == bound.id)
        {
            ui.add_space(kit::GAP);
            kit::help(ui, &crate::i18n::fmt(tl!("{count} findings."), &[("count", &report.findings_total.to_string())]));
        }
    });
}
