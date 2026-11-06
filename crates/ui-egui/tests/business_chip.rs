//! The header chip that names an e-invoice or an XBRL instance, in the real shell.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_ui_egui::XmlCraftApp;

const INVOICE: &str = r#"<Invoice xmlns="urn:oasis:names:specification:ubl:schema:xsd:Invoice-2" xmlns:cbc="urn:oasis:names:specification:ubl:schema:xsd:CommonBasicComponents-2"><cbc:CustomizationID>urn:cen.eu:en16931:2017#compliant#urn:xeinkauf.de:kosit:xrechnung_3.0</cbc:CustomizationID><cbc:ID>1</cbc:ID></Invoice>"#;
const XBRL: &str = r#"<xbrli:xbrl xmlns:xbrli="http://www.xbrl.org/2003/instance" xmlns:link="http://www.xbrl.org/2003/linkbase" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:iso4217="http://www.xbrl.org/2003/iso4217" xmlns:eg="http://example.com/eg"><link:schemaRef xlink:type="simple" xlink:href="eg.xsd"/><xbrli:context id="c"><xbrli:entity><xbrli:identifier scheme="http://example.com">x</xbrli:identifier></xbrli:entity><xbrli:period><xbrli:instant>2025-12-31</xbrli:instant></xbrli:period></xbrli:context><xbrli:unit id="u"><xbrli:measure>iso4217:EUR</xbrli:measure></xbrli:unit><eg:Revenue contextRef="c" unitRef="u" decimals="0">10</eg:Revenue></xbrli:xbrl>"#;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

fn open(h: &mut Harness<'static, XmlCraftApp>, dir: &Path, name: &str, text: &str) {
    let path: PathBuf = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    assert!(h.state_mut().open_path(&path));
    let started = Instant::now();
    while h.state().problems().is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the analysis");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(3);
}

#[test]
fn an_e_invoice_and_an_xbrl_instance_are_named_in_the_header_and_other_documents_are_not() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, dir.path(), "plain.xml", "<a><b/></a>");
    assert!(h.query_by_label_contains("XRechnung").is_none() && h.query_by_label_contains("XBRL").is_none());
    open(&mut h, dir.path(), "invoice.xml", INVOICE);
    assert!(h.query_by_label_contains("XRechnung").is_some(), "the invoice is named");
    open(&mut h, dir.path(), "report.xbrl", XBRL);
    assert!(h.query_by_label_contains("XBRL · 1 fact").is_some(), "the instance says how many facts it has");
    assert!(h.query_by_label_contains("XRechnung").is_none(), "the chip is the shown document's");
}

#[test]
fn the_chip_follows_the_text_once_it_has_stood_still() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, dir.path(), "x.xml", INVOICE);
    assert!(h.query_by_label_contains("XRechnung").is_some());
    {
        let doc = h.state_mut().session.active_document_mut().expect("shown");
        doc.editor_mut().select_all();
        doc.editor_mut().type_text("<a/>");
    }
    let started = Instant::now();
    while h.query_by_label_contains("XRechnung").is_some() {
        assert!(started.elapsed() < Duration::from_secs(10), "the chip went on naming an invoice");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(20));
    }
}
