//! The validation of an electronic invoice against what ships with XmlCraft (`xmlcraft_einvoice::validate`), for the tool and the
//! window: offline by construction (the resolver knows the bundle and nothing else), within a time budget, behind the last-resort guard.

use std::sync::Arc;

use xmlcraft_einvoice::validate::{Options, Report, Stage, validate};
use xmlcraft_resolve::{Environment, FixedClock, NoBuffers, NoFiles, Policy, Resolver};

/// How long the rules of one validation may take before the checks not reached are reported as not evaluated.
pub const TIME_BUDGET_SECS: u64 = 60;

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
}
