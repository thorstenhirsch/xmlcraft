//! The e-invoice validation pipeline on synthetic invoices (`fixtures/`): a valid XRechnung invoice in UBL and in CII passes every
//! stage, and each broken variant is reported with exactly the rules it breaks, at the element they are about.

use std::collections::BTreeSet;
use std::sync::Arc;

use xmlcraft_einvoice::validate::{Options, Report, Severity, Stage, Status, validate};
use xmlcraft_resolve::{Environment, FixedClock, NoBuffers, NoFiles, Policy, Resolver};
use xmlcraft_schematron::CheckState;

const UBL: &str = include_str!("fixtures/xrechnung-ubl.xml");
const CII: &str = include_str!("fixtures/xrechnung-cii.xml");

fn resolver() -> Resolver {
    let env = Environment { buffers: Arc::new(NoBuffers), files: Arc::new(NoFiles), fetcher: None, cache: None, clock: Arc::new(FixedClock(0)) };
    Resolver::new(env, Policy { offline: true, auto_download: false, ..Policy::default() }, &[])
}

fn check(text: &str) -> Report {
    validate(text, "", &resolver(), &Options::default())
}

/// The ids of the fatal findings, by stage.
fn fatal(report: &Report) -> BTreeSet<(Stage, String)> {
    report.findings.iter().filter(|f| f.severity == Severity::Fatal).map(|f| (f.stage, f.rule.clone().unwrap_or_default())).collect()
}

/// The 1-based line of the first occurrence of `needle` in `text`.
fn line_of(text: &str, needle: &str) -> usize {
    let at = text.find(needle).unwrap_or_else(|| panic!("{needle} is in the text"));
    text[..at].matches('\n').count() + 1
}

fn assert_valid(text: &str) {
    let report = check(text);
    for stage in &report.stages {
        assert_eq!(stage.status, Status::Passed, "{:?}: {:?}", stage.stage, report.findings);
        if let Some(t) = stage.totals {
            assert_eq!(t.failed, 0, "{:?}", stage.stage);
            assert!(t.passed > 0, "{:?} evaluated something", stage.stage);
        }
    }
    assert!(report.findings.is_empty(), "{:?}", report.findings);
    assert!(report.valid());
    // Nothing of these rule sets is beyond the engine on these invoices.
    assert_eq!(report.not_evaluated_total, 0, "{:?}", report.not_evaluated);
    assert_eq!(report.totals.not_evaluated, 0);
}

#[test]
fn a_valid_xrechnung_invoice_in_ubl_and_in_cii_passes_every_stage() {
    assert_valid(UBL);
    assert_valid(CII);
    let report = check(UBL);
    let against: Vec<Option<&str>> = report.stages.iter().map(|s| s.against.as_deref()).collect();
    assert_eq!(against[1], Some("XRechnung, Invoice"));
    assert!(against[3].is_some_and(|a| a.starts_with("EN 16931 1.3.17")), "{against:?}");
    assert!(against[4].is_some_and(|a| a.starts_with("XRechnung Schematron")), "{against:?}");
    let t = report.totals;
    assert_eq!(t.checks, t.passed + t.failed + t.not_fired + t.not_evaluated);
}

#[test]
fn a_wrong_total_is_br_co_15_at_the_invoice() {
    // The amount with VAT is not the amount without VAT plus the VAT (the amount due is changed with it, so BR-CO-16 still holds).
    let broken = UBL
        .replace(
            "<cbc:TaxInclusiveAmount currencyID=\"EUR\">119.00</cbc:TaxInclusiveAmount>",
            "<cbc:TaxInclusiveAmount currencyID=\"EUR\">118.00</cbc:TaxInclusiveAmount>",
        )
        .replace(
            "<cbc:PayableAmount currencyID=\"EUR\">119.00</cbc:PayableAmount>",
            "<cbc:PayableAmount currencyID=\"EUR\">118.00</cbc:PayableAmount>",
        );
    let report = check(&broken);
    assert_eq!(fatal(&report), BTreeSet::from([(Stage::En16931, "BR-CO-15".to_owned())]), "{:?}", report.findings);
    let f = report.findings.iter().find(|f| f.rule.as_deref() == Some("BR-CO-15")).expect("found");
    assert_eq!(f.line, line_of(&broken, "<ubl:Invoice"), "at the invoice element: {f:?}");
    assert!(f.test.as_deref().is_some_and(|t| t.contains("TaxInclusiveAmount")), "{f:?}");
    assert_eq!(report.stage(Stage::En16931).map(|s| &s.status), Some(&Status::Failed));
    assert_eq!(report.stage(Stage::Cius).map(|s| &s.status), Some(&Status::Passed), "the CIUS stage still runs");
    assert!(!report.valid());
    // The same in CII: the grand total is not the basis plus the VAT.
    let broken = CII
        .replace("<ram:GrandTotalAmount>119.00</ram:GrandTotalAmount>", "<ram:GrandTotalAmount>118.00</ram:GrandTotalAmount>")
        .replace("<ram:DuePayableAmount>119.00</ram:DuePayableAmount>", "<ram:DuePayableAmount>118.00</ram:DuePayableAmount>");
    let report = check(&broken);
    assert_eq!(fatal(&report), BTreeSet::from([(Stage::En16931, "BR-CO-15".to_owned())]), "{:?}", report.findings);
    let f = report.findings.iter().find(|f| f.rule.as_deref() == Some("BR-CO-15")).expect("found");
    assert_eq!(f.line, line_of(&broken, "<rsm:CrossIndustryInvoice"), "the rule is about the invoice: {f:?}");
}

#[test]
fn a_missing_buyer_reference_is_br_de_15_of_xrechnung() {
    let broken = UBL.replace("  <cbc:BuyerReference>04011000-12345-67</cbc:BuyerReference>\n", "");
    let report = check(&broken);
    assert_eq!(fatal(&report), BTreeSet::from([(Stage::Cius, "BR-DE-15".to_owned())]), "{:?}", report.findings);
    let f = report.findings.iter().find(|f| f.rule.as_deref() == Some("BR-DE-15")).expect("found");
    assert_eq!(f.line, line_of(&broken, "<ubl:Invoice"), "{f:?}");
    let broken = CII.replace("      <ram:BuyerReference>04011000-12345-67</ram:BuyerReference>\n", "");
    let report = check(&broken);
    assert_eq!(fatal(&report), BTreeSet::from([(Stage::Cius, "BR-DE-15".to_owned())]), "{:?}", report.findings);
}

#[test]
fn a_missing_seller_vat_identifier_breaks_the_rules_that_need_it() {
    let start = UBL.find("      <cac:PartyTaxScheme>").expect("the seller's VAT id");
    let end = UBL[start..].find("</cac:PartyTaxScheme>\n").map(|e| start + e + "</cac:PartyTaxScheme>\n".len()).expect("its end");
    let broken = format!("{}{}", &UBL[..start], &UBL[end..]);
    let report = check(&broken);
    assert_eq!(
        fatal(&report),
        BTreeSet::from([(Stage::En16931, "BR-S-02".to_owned()), (Stage::Cius, "BR-DE-16".to_owned())]),
        "{:?}",
        report.findings
    );
    // BR-S-02 is about the invoice (whose lines are standard rated).
    let f = report.findings.iter().find(|f| f.rule.as_deref() == Some("BR-S-02")).expect("found");
    assert!(f.path.as_deref().is_some_and(|p| p.ends_with("Invoice[1]")), "{f:?}");
}

#[test]
fn a_missing_payment_instruction_is_br_de_1_and_a_wrong_type_code_is_found_at_its_element() {
    let start = UBL.find("  <cac:PaymentMeans>").expect("payment means");
    let end = UBL.find("  </cac:PaymentMeans>\n").map(|e| e + "  </cac:PaymentMeans>\n".len()).expect("its end");
    let broken = format!("{}{}", &UBL[..start], &UBL[end..]);
    let report = check(&broken);
    assert_eq!(fatal(&report), BTreeSet::from([(Stage::Cius, "BR-DE-1".to_owned())]), "{:?}", report.findings);
    let broken = UBL.replace("<cbc:InvoiceTypeCode>380</cbc:InvoiceTypeCode>", "<cbc:InvoiceTypeCode>999</cbc:InvoiceTypeCode>");
    let report = check(&broken);
    let ids = fatal(&report);
    assert!(ids.contains(&(Stage::En16931, "BR-CL-01".to_owned())), "{ids:?}");
    let f = report.findings.iter().find(|f| f.rule.as_deref() == Some("BR-CL-01")).expect("found");
    assert_eq!((f.line, &broken[f.span.clone()]), (line_of(&broken, "<cbc:InvoiceTypeCode>"), "<cbc:InvoiceTypeCode>999</cbc:InvoiceTypeCode>"));
}

#[test]
fn a_schema_problem_does_not_stop_the_rules_and_a_broken_document_stops_everything() {
    let broken = UBL.replace("<cbc:ID>RE-2026-0001</cbc:ID>", "<cbc:ID>RE-2026-0001</cbc:ID><cbc:Bogus/>");
    let report = check(&broken);
    assert_eq!(report.stage(Stage::Schema).map(|s| &s.status), Some(&Status::Failed));
    assert_eq!(report.stage(Stage::En16931).map(|s| &s.status), Some(&Status::Passed), "{:?}", report.findings);
    let f = report.findings.iter().find(|f| f.stage == Stage::Schema).expect("a schema finding");
    assert_eq!(f.line, line_of(&broken, "<cbc:Bogus/>"));
    let report = check(&UBL.replace("</ubl:Invoice>", ""));
    assert_eq!(report.stage(Stage::WellFormedness).map(|s| &s.status), Some(&Status::Failed));
    assert!(report.stages.iter().skip(1).all(|s| matches!(s.status, Status::NotRun(_))), "{:?}", report.stages);
    assert!(report.findings.iter().all(|f| f.stage == Stage::WellFormedness) && !report.findings.is_empty());
}

#[test]
fn what_does_not_apply_is_said_and_not_run() {
    // Peppol BIS: recognised, EN 16931 runs, its own rules are not bundled.
    let peppol = UBL.replace(
        "urn:cen.eu:en16931:2017#compliant#urn:xeinkauf.de:kosit:xrechnung_3.0",
        "urn:cen.eu:en16931:2017#compliant#urn:fdc:peppol.eu:2017:poacc:billing:3.0",
    );
    let report = check(&peppol);
    assert_eq!(report.stage(Stage::En16931).map(|s| &s.status), Some(&Status::Passed));
    assert!(matches!(report.stage(Stage::Cius).map(|s| &s.status), Some(Status::NotRun(why)) if why.contains("Peppol")), "{:?}", report.stages);
    // An older XRechnung is not checked against the 3.0 rules.
    let old = UBL.replace("kosit:xrechnung_3.0", "kosit:xrechnung_2.3");
    assert!(matches!(check(&old).stage(Stage::Cius).map(|s| &s.status), Some(Status::NotRun(why)) if why.contains("2.3")));
    // FatturaPA: recognised; its schema is not bundled.
    let fattura = "<p:FatturaElettronica versione=\"FPR12\" xmlns:p=\"http://ivaservizi.agenziaentrate.gov.it/docs/xsd/fatture/v1.2\"/>";
    let report = check(fattura);
    assert_eq!(report.stage(Stage::Detection).map(|s| &s.status), Some(&Status::Passed));
    assert!(matches!(report.stage(Stage::Schema).map(|s| &s.status), Some(Status::NotRun(why)) if why.contains("FatturaPA")));
    assert!(report.valid(), "nothing that ran failed");
    // Other XML is no invoice.
    let report = check("<a/>");
    assert_eq!(report.stage(Stage::Detection).map(|s| &s.status), Some(&Status::Failed));
    assert!(!report.valid());
    // Stages that are not asked for do not run.
    let only = validate(UBL, "", &resolver(), &Options { stages: Some(vec![Stage::Schema]), ..Options::default() });
    assert!(matches!(only.stage(Stage::En16931).map(|s| &s.status), Some(Status::NotRun(why)) if why == "not asked for"));
}

#[test]
fn the_findings_are_capped_and_counted_and_a_check_that_was_not_evaluated_is_never_a_failure() {
    // Every amount in an unknown currency: many findings of BR-CL-03.
    let broken = UBL.replace("currencyID=\"EUR\"", "currencyID=\"XXQ\"");
    let report = validate(&broken, "", &resolver(), &Options { max_findings: 3, ..Options::default() });
    assert_eq!(report.findings.len(), 3);
    assert!(report.findings_total > 3, "{}", report.findings_total);
    // A budget that runs out at once: the rules are not evaluated, and nothing is reported as failed or passed for them.
    let stop = || true;
    let report = validate(&broken, "", &resolver(), &Options { stop: Some(&stop), ..Options::default() });
    for stage in [Stage::En16931, Stage::Cius] {
        let t = report.stage(stage).and_then(|s| s.totals).expect("totals");
        assert_eq!((t.passed, t.failed), (0, 0), "{stage:?}");
        assert!(t.not_evaluated > 0);
    }
    assert!(report.findings.iter().all(|f| f.stage != Stage::En16931 && f.stage != Stage::Cius));
    assert!(report.not_evaluated.iter().all(|n| n.reason.contains("time budget")), "{:?}", report.not_evaluated.first());
    let _ = CheckState::Passed;
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig { cases: 24, ..Default::default() })]

    /// A hostile invoice (the valid one cut anywhere, or with text put anywhere) is answered without a panic.
    #[test]
    fn a_hostile_invoice_is_answered_without_a_panic(cut in 0usize..6000, insert in ".{0,12}", syntax in 0usize..2) {
        let doc = if syntax == 0 { UBL } else { CII };
        let at = (0..=cut.min(doc.len())).rev().find(|i| doc.is_char_boundary(*i)).unwrap_or(0);
        let _ = check(&doc[..at]);
        let _ = check(&format!("{}{insert}{}", &doc[..at], &doc[at..]));
    }
}

#[test]
fn deeply_nested_and_huge_invoices_are_answered() {
    let deep = UBL.replace("<cbc:Name>Beratung</cbc:Name>", &format!("<cbc:Name>{}x{}</cbc:Name>", "<a>".repeat(5000), "</a>".repeat(5000)));
    let _ = check(&deep);
    let many_lines: String = (0..300)
        .map(|i| {
            UBL[UBL.find("  <cac:InvoiceLine>").expect("a line")..UBL.find("</ubl:Invoice>").expect("end")]
                .replace("<cbc:ID>1</cbc:ID>", &format!("<cbc:ID>{i}</cbc:ID>"))
        })
        .collect();
    let big = format!("{}{many_lines}</ubl:Invoice>", &UBL[..UBL.find("  <cac:InvoiceLine>").expect("a line")]);
    let started = std::time::Instant::now();
    let report = check(&big);
    eprintln!("300 lines: {:?}, {} findings", started.elapsed(), report.findings_total);
    assert!(report.stage(Stage::En16931).is_some_and(|s| s.status == Status::Failed), "the totals no longer add up");
}

/// A rule that compares `number()` of an amount the invoice lacks must not pass: `number()` of nothing is NaN, and every comparison
/// with NaN is false except `!=` (XPath 3.1 §3.7). The published XPath engine answered `NaN ge 0` with true, so such a rule passed on
/// an invoice without the amount (vendor/README.md, `xee-interpreter`). The bundled EN 16931 and XRechnung rules convert with
/// `xs:decimal`, which fails on a missing value instead; this is the shape of a user's or a CIUS's own rule.
#[test]
fn a_rule_comparing_a_missing_amount_does_not_pass() {
    use xmlcraft_schematron::Schema;
    let rules = r#"<schema xmlns="http://purl.oclc.org/dsdl/schematron" queryBinding="xslt2">
        <ns prefix="ram" uri="urn:un:unece:uncefact:data:standard:ReusableAggregateBusinessInformationEntity:100"/>
        <pattern><rule context="ram:SpecifiedTradeSettlementHeaderMonetarySummation">
          <assert id="R-GE" test="number(ram:DuePayableAmount) ge 0">The amount due shall not be negative.</assert>
          <assert id="R-GT" test="number(ram:DuePayableAmount) &gt; -1">The amount due shall be more than -1.</assert>
          <assert id="R-LE" test="0 &lt;= number(ram:DuePayableAmount)">The amount due shall be at least 0.</assert>
          <assert id="R-NE" test="number(ram:DuePayableAmount) != 0">The amount due shall not be zero.</assert>
        </rule></pattern></schema>"#;
    let schema = Schema::parse(rules, &|_| None).expect("reads");
    let ids = |text: &str| -> BTreeSet<String> {
        let report = schema.validate(text, None).expect("validates");
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        report.findings.iter().filter_map(|f| f.id.clone()).collect()
    };
    assert!(ids(CII).is_empty(), "the amount is there: every rule holds");
    let without = CII.replace("<ram:DuePayableAmount>119.00</ram:DuePayableAmount>", "");
    assert_ne!(without, CII);
    let expected: BTreeSet<String> = ["R-GE", "R-GT", "R-LE"].map(str::to_owned).into();
    assert_eq!(ids(&without), expected, "NaN compares false, except `!=`");
}
