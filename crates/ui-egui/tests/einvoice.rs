//! The E-Rechnung tool window in the real shell: the rail and the command bar for an invoice, the overview of what it says, its
//! validation (F8 and the tool's button feed the same result), and the hybrid PDF made from it. Asynchronous work is waited for by
//! polling the state, never by counting frames.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use xmlcraft_ui_egui::{Panel, XmlCraftApp};

const UBL: &str = include_str!("../../einvoice/tests/fixtures/xrechnung-ubl.xml");

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1440.0, 900.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

fn file(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path
}

/// Step the shell until `done` holds (or fail after `secs` seconds).
fn wait_for(h: &mut Harness<'static, XmlCraftApp>, secs: u64, what: &str, mut done: impl FnMut(&mut Harness<'static, XmlCraftApp>) -> bool) {
    let started = Instant::now();
    while !done(h) {
        assert!(started.elapsed() < Duration::from_secs(secs), "waiting for {what}");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn open(h: &mut Harness<'static, XmlCraftApp>, path: &Path) {
    assert!(h.state_mut().open_path(path));
    wait_for(h, 20, "the first analysis", |h| h.state().problems().is_some());
    h.run_steps(2);
}

#[test]
fn an_invoice_lights_the_rail_icon_and_offers_its_actions_and_other_xml_does_not() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "plain.xml", "<library><book/></library>"));
    assert!(!h.state_mut().panel_relevant(Panel::EInvoice), "plain XML: the rail icon is dimmed");
    assert!(h.query_by_label("Validate invoice").is_none(), "no invoice actions for plain XML");
    open(&mut h, &file(dir.path(), "rechnung.xml", UBL));
    assert!(h.state_mut().panel_relevant(Panel::EInvoice), "an invoice: the rail icon is lit");
    assert!(h.query_by_label("Validate invoice").is_some(), "the command bar offers the invoice's actions");
    assert!(!h.state().panel_visible(Panel::EInvoice), "hidden by default");
    h.get_by_label("Invoice").click();
    h.run_steps(3);
    assert!(h.state().panel_visible(Panel::EInvoice) && h.query_by_label("Close E-Rechnung").is_some(), "the tool window is up");
    assert!(h.query_by_label("→ rechnung.xml").is_some(), "it says which document it works on");
}

/// The fixture with the amount with VAT and the amount due one euro short: BR-CO-15 (and nothing else) is broken.
fn broken_total() -> String {
    UBL.replace(
        "<cbc:TaxInclusiveAmount currencyID=\"EUR\">119.00</cbc:TaxInclusiveAmount>",
        "<cbc:TaxInclusiveAmount currencyID=\"EUR\">118.00</cbc:TaxInclusiveAmount>",
    )
    .replace("<cbc:PayableAmount currencyID=\"EUR\">119.00</cbc:PayableAmount>", "<cbc:PayableAmount currencyID=\"EUR\">118.00</cbc:PayableAmount>")
}

/// Press F8 and wait until the validation of the text as it is now has arrived (the status bar says what it found).
fn validate(h: &mut Harness<'static, XmlCraftApp>) {
    h.key_press(egui::Key::F8);
    h.run_steps(2);
    assert!(h.state().status.starts_with("Validating"), "{}", h.state().status);
    wait_for(h, 120, "the validation", |h| h.state().validity().is_some() && !h.state().status.starts_with("Validating"));
    h.run_steps(3);
}

#[test]
fn f8_lists_the_rule_findings_of_an_invoice_in_problems_and_fixing_the_text_clears_them() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "broken.xml", &broken_total()));
    validate(&mut h);
    assert!(h.query_all_by_label_contains("BR-CO-15: ").next().is_some(), "Problems lists the rule: {}", h.state().status);
    assert!(h.query_all_by_label_contains("EN 16931 · Ln").next().is_some(), "with the rule set it comes from");
    // Fixed, and F8 again: the finding is gone.
    if let Some(doc) = h.state_mut().session.active_document_mut() {
        doc.editor_mut().select_all();
        doc.editor_mut().paste(UBL);
    }
    h.run_steps(2);
    validate(&mut h);
    assert!(h.query_all_by_label_contains("BR-CO-15").next().is_none(), "no finding after the fix");
    assert_eq!(h.state().validity(), Some((0, 0)), "and the invoice is valid");
}

/// The selected text of the document being shown.
fn selected(h: &Harness<'static, XmlCraftApp>) -> String {
    let doc = h.state().session.active_document().expect("shown");
    let text = doc.editor().text();
    let s = doc.editor().selections()[0];
    let (start, end) = (s.start().min(s.end()), s.start().max(s.end()));
    text.get(start..end).unwrap_or_default().to_owned()
}

#[test]
fn the_overview_shows_the_number_and_the_amount_due_and_a_click_selects_the_value() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "rechnung.xml", UBL));
    assert!(h.state_mut().execute("xml.einvoice"));
    wait_for(&mut h, 30, "the overview", |h| h.query_all_by_label("119.00 EUR").next().is_some());
    assert!(h.query_all_by_label("RE-2026-0001").next().is_some(), "the invoice number");
    assert!(h.query_all_by_label_contains("XRechnung 3.0").next().is_some(), "the standard");
    // (A pointer is over a value for a frame before it clicks it, as a hand does.)
    h.query_all_by_label("RE-2026-0001").next().expect("the number").hover();
    h.run_steps(2);
    h.query_all_by_label("RE-2026-0001").next().expect("the number").click();
    h.run_steps(3);
    assert_eq!(selected(&h), "RE-2026-0001", "the number is selected in the text");
}

#[test]
fn the_validate_tab_shows_the_stages_and_the_finding_and_a_click_selects_its_element() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "broken.xml", &broken_total()));
    assert!(h.state_mut().execute("xml.validate_invoice"), "the Validate tab is shown and the validation started");
    wait_for(&mut h, 120, "the report", |h| h.query_by_label("Copy report").is_some());
    h.run_steps(3);
    assert!(h.query_all_by_label("1 failed").next().is_some(), "the EN 16931 stage failed one check");
    assert!(h.query_all_by_label("passed").count() >= 3, "well-formed, detection, XSD and the CIUS passed");
    // Problems lists the finding too; the tool's row is the one with the rule set's chip-less label `BR-CO-15: …` and no place.
    let rows =
        h.query_all_by_label_contains("BR-CO-15: ").filter(|n| !n.accesskit_node().label().unwrap_or_default().contains("EN 16931 · Ln")).count();
    assert_eq!(rows, 1, "one row in the tool");
    let row = || "BR-CO-15: ";
    h.query_all_by_label_contains(row())
        .find(|n| !n.accesskit_node().label().unwrap_or_default().contains("EN 16931 · Ln"))
        .expect("the row")
        .hover();
    h.run_steps(2);
    h.query_all_by_label_contains(row())
        .find(|n| !n.accesskit_node().label().unwrap_or_default().contains("EN 16931 · Ln"))
        .expect("the row")
        .click();
    h.run_steps(3);
    assert!(selected(&h).starts_with("<ubl:Invoice"), "the rule is about the invoice element");
    // An edit makes the report stale, visibly.
    if let Some(doc) = h.state_mut().session.active_document_mut() {
        doc.editor_mut().place_caret(0, false);
        doc.editor_mut().type_text(" ");
    }
    h.run_steps(3);
    assert!(h.query_by_label_contains("The document changed since").is_some(), "the report says it is out of date");
}

#[test]
fn a_hybrid_pdf_carries_the_xml_byte_for_byte_and_an_existing_file_is_not_replaced_silently() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "rechnung.xml", UBL));
    assert!(h.state_mut().execute("xml.einvoice"));
    h.run_steps(2);
    h.get_by_label("PDF").click();
    h.run_steps(3);
    let out = dir.path().join("out.pdf");
    // The save panel answers with this path, as the platform's would.
    h.state_mut().pick_override = Some(vec![out.clone()]);
    h.get_by_label("Choose where to save the PDF").click();
    h.run_steps(4);
    h.get_by_label("Make hybrid PDF").click();
    wait_for(&mut h, 60, "the PDF", |h| h.query_all_by_label_contains("Not verified").next().is_some());
    let pdf = xmlcraft_pdf::Pdf::open(std::fs::read(&out).expect("the PDF was written")).expect("a PDF");
    let files = pdf.embedded_files();
    let embedded = files.first().expect("an embedded file");
    assert_eq!(pdf.embedded_data(embedded).expect("its data"), UBL.as_bytes(), "the XML is inside byte for byte");
    // Again: the file is there, so the tool asks before it replaces it.
    let before = std::fs::read(&out).expect("read");
    std::fs::write(&out, b"not a PDF any more").expect("writes");
    h.get_by_label("Make hybrid PDF").click();
    wait_for(&mut h, 10, "the question", |h| h.query_by_label("Replace").is_some());
    assert_eq!(std::fs::read(&out).expect("read"), b"not a PDF any more", "nothing was written yet");
    h.get_by_label("Replace").click();
    wait_for(&mut h, 60, "the second PDF", |_| std::fs::read(&out).is_ok_and(|b| b.starts_with(b"%PDF")));
    assert!(!before.is_empty());
}
