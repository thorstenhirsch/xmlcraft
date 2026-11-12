//! The E-Rechnung tool window in the real shell: the rail and the command bar for an invoice, the overview of what it says, its
//! validation (F8 and the tool's button feed the same result), and the hybrid PDF made from it. Asynchronous work is waited for by
//! polling the state, never by counting frames.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
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
