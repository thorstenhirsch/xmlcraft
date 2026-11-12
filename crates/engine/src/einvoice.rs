//! The validation of an electronic invoice against what ships with XmlCraft (`xmlcraft_einvoice::validate`), for the tool and the
//! window: offline by construction (the resolver knows the bundle and nothing else), within a time budget, behind the last-resort guard.

use std::sync::Arc;

use xmlcraft_einvoice::validate::{Options, Report, Stage, validate};
use xmlcraft_resolve::{Environment, FixedClock, NoBuffers, NoFiles, Policy, Resolver};

/// How long the rules of one validation may take before the checks not reached are reported as not evaluated.
pub const TIME_BUDGET_SECS: u64 = 60;

/// The namespaces an electronic invoice's root element is in: a text that names none of them is not read ([`detect_einvoice`]).
const INVOICE_NAMESPACES: [&str; 5] = [
    "urn:oasis:names:specification:ubl:schema:xsd:",
    "urn:un:unece:uncefact:data:standard:CrossIndustryInvoice:100",
    "urn:ferd:CrossIndustryDocument:invoice:1p0",
    "ivaservizi.agenziaentrate.gov.it/docs/xsd/fatture/",
    "www.fatturapa.gov.it/export/fatturazione/sdi/fatturapa/",
];

/// Whether a recognised document is an electronic invoice the E-Rechnung tools work on: an invoice or a credit note in UBL, in
/// UN/CEFACT CII (ZUGFeRD 1.0's too) or in FatturaPA, whatever standard it claims. Orders and the other business documents are not.
pub fn is_einvoice(d: &xmlcraft_einvoice::Detection) -> bool {
    use xmlcraft_einvoice::Syntax;
    matches!(d.syntax, Syntax::Ubl | Syntax::Cii | Syntax::CiiLegacy | Syntax::FatturaPa) && matches!(d.document.as_str(), "Invoice" | "Credit note")
}

/// What `text` is when it is an electronic invoice ([`is_einvoice`]), else none. Cheap to refuse: a text that names none of the
/// namespaces of the invoice syntaxes is not parsed. An escaped panic is no invoice.
pub fn detect_einvoice(text: &str) -> Option<xmlcraft_einvoice::Detection> {
    if !INVOICE_NAMESPACES.iter().any(|ns| text.contains(ns)) {
        return None;
    }
    crate::guard(|| xmlcraft_einvoice::detect(text)).ok().flatten().filter(is_einvoice)
}

/// A resolver that finds the bundled schemas and nothing else: no files, no open documents, no network.
pub fn bundled_only_resolver() -> Resolver {
    let env = Environment { buffers: Arc::new(NoBuffers), files: Arc::new(NoFiles), fetcher: None, cache: None, clock: Arc::new(FixedClock(0)) };
    Resolver::new(env, Policy { offline: true, auto_download: false, use_bundled: true, ..Policy::default() }, &[])
}

/// Validate the e-invoice `text` (at `location`; empty when it has none) with the `stages` asked for (all when `None`), listing at most
/// `max_findings`. An escaped panic is an error, not a crash.
pub fn einvoice_validate(text: &str, location: &str, stages: Option<Vec<Stage>>, max_findings: usize) -> Result<Report, String> {
    crate::guard(|| {
        let resolver = bundled_only_resolver();
        #[cfg(not(target_arch = "wasm32"))]
        let started = std::time::Instant::now();
        #[cfg(not(target_arch = "wasm32"))]
        let stop = move || started.elapsed() > std::time::Duration::from_secs(TIME_BUDGET_SECS);
        #[cfg(not(target_arch = "wasm32"))]
        let stop: Option<&dyn Fn() -> bool> = Some(&stop);
        // (There is no clock on the web: the rules run to the end there.)
        #[cfg(target_arch = "wasm32")]
        let stop: Option<&dyn Fn() -> bool> = None;
        validate(text, location, &resolver, &Options { stages, max_findings, stop })
    })
}

/// The neutral invoice model of `text` (`xmlcraft_einvoice::model`), or none when it is no UBL or CII invoice. An escaped panic is an error.
pub fn einvoice_model(text: &str) -> Result<Option<xmlcraft_einvoice::model::Invoice>, String> {
    crate::guard(|| xmlcraft_einvoice::model::read(text))
}

/// The visual PDF of the invoice `text`, embedding `bytes` (its original bytes) unchanged (`xmlcraft_invoice_pdf::render`). An escaped panic
/// is an error.
pub fn einvoice_render_pdf(
    text: &str,
    bytes: &[u8],
    options: &xmlcraft_invoice_pdf::Options<'_>,
) -> Result<Result<xmlcraft_invoice_pdf::Rendered, xmlcraft_invoice_pdf::RenderError>, String> {
    crate::guard(|| xmlcraft_invoice_pdf::render(text, bytes, options))
}

#[cfg(test)]
mod tests {
    use super::*;
    use xmlcraft_einvoice::validate::Status;

    #[test]
    fn a_ubl_invoice_is_validated_offline_stage_by_stage() {
        let invoice = include_str!("../../einvoice/tests/fixtures/xrechnung-ubl.xml");
        let report = einvoice_validate(invoice, "", None, 100).expect("no panic");
        let statuses: Vec<(Stage, &Status)> = report.stages.iter().map(|s| (s.stage, &s.status)).collect();
        assert_eq!(statuses.len(), 5, "{statuses:?}");
        assert!(report.stages.iter().all(|s| s.status == Status::Passed), "{statuses:?} {:?}", report.findings);
        assert!(report.valid());
    }

    #[test]
    fn a_document_that_is_no_invoice_or_no_xml_is_answered() {
        for text in ["", "<a/>", "<a>", "not xml", "<Invoice xmlns=\"urn:oasis:names:specification:ubl:schema:xsd:Invoice-2\"/>"] {
            let report = einvoice_validate(text, "", None, 10).expect("no panic");
            assert!(!report.valid(), "{text:?}");
        }
    }

    #[test]
    fn invoices_and_credit_notes_are_e_invoices_and_orders_and_other_xml_are_not() {
        let ubl = include_str!("../../einvoice/tests/fixtures/xrechnung-ubl.xml");
        let cii = include_str!("../../einvoice/tests/fixtures/xrechnung-cii.xml");
        assert!(detect_einvoice(ubl).is_some() && detect_einvoice(cii).is_some());
        let fattura = "<p:FatturaElettronica versione=\"FPR12\" xmlns:p=\"http://ivaservizi.agenziaentrate.gov.it/docs/xsd/fatture/v1.2\"/>";
        assert!(detect_einvoice(fattura).is_some());
        assert!(detect_einvoice("<CreditNote xmlns=\"urn:oasis:names:specification:ubl:schema:xsd:CreditNote-2\"/>").is_some());
        for text in [
            "",
            "<a/>",
            "<Order xmlns=\"urn:oasis:names:specification:ubl:schema:xsd:Order-2\"/>",
            "not xml <",
            "urn:oasis:names:specification:ubl:schema:xsd:",
        ] {
            assert!(detect_einvoice(text).is_none(), "{text:?}");
        }
    }
}
