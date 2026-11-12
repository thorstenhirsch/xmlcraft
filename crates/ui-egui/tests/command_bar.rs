//! The command bar in the real shell: Validate and Format for every document, then the actions of the kind of document in front.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use xmlcraft_ui_egui::XmlCraftApp;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

fn file(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path
}

fn open(h: &mut Harness<'static, XmlCraftApp>, path: &Path) {
    assert!(h.state_mut().open_path(path));
    let started = Instant::now();
    while h.state().problems().is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the first analysis");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(3);
}

const XSLT: &str = r#"<xsl:stylesheet version="1.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:template match="/"/></xsl:stylesheet>"#;
const XSD: &str = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="n"/></xs:schema>"#;
const WSDL: &str = r#"<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/" targetNamespace="urn:t"/>"#;
const OPENAPI: &str =
    "openapi: 3.1.0\ninfo: {title: T, version: '1'}\nservers: [{url: 'http://localhost:9'}]\npaths:\n  /ping:\n    get: {operationId: ping}\n";

#[test]
fn every_document_has_validate_and_format_and_the_fixed_check_and_assign_are_gone() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.xml", "<a/>"));
    assert!(h.query_by_label("Validate").is_some() && h.query_by_label("Format").is_some());
    assert!(!h.get_by_label("Format").accesskit_node().is_disabled());
    assert!(h.query_by_label("Check").is_none(), "Check is a schema's action now, and in the Schema menu");
    assert!(h.query_by_label("Assign Schema…").is_none(), "Assign is in Schema… and the header chip");
}

#[test]
fn a_stylesheet_offers_run_which_transforms() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    // (Run transforms with the Query tool that Transform with XSLT… shows: another open document is what it is run with.)
    open(&mut h, &file(dir.path(), "in.xml", "<a/>"));
    open(&mut h, &file(dir.path(), "s.xsl", XSLT));
    assert!(h.query_by_label("Run").is_some() && h.query_by_label("XPath").is_some());
    assert!(h.query_by_label("Schema…").is_none(), "a stylesheet is not offered the XML schema menu");
    h.get_by_label("Run").click();
    h.run_steps(4);
    assert!(h.query_by_label("Close Query").is_some(), "Run shows the Query tool on its XSLT tab");
    assert!(h.query_by_label("This document is the stylesheet").is_some());
}

#[test]
fn a_schema_offers_a_sample_and_a_check() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "s.xsd", XSD));
    assert!(h.query_by_label("Sample XML").is_some() && h.query_by_label("Check").is_some());
    assert!(h.query_by_label("XPath").is_none() && h.query_by_label("Schema…").is_none());
}

#[test]
fn a_wsdl_offers_a_soap_request_and_an_openapi_description_a_try() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "s.wsdl", WSDL));
    assert!(h.query_by_label("SOAP Request").is_some() && h.query_by_label("Schema…").is_none());
    open(&mut h, &file(dir.path(), "api.yaml", OPENAPI));
    assert!(h.query_by_label("Try Operation").is_some() && h.query_by_label("Convert").is_some());
    open(&mut h, &file(dir.path(), "plain.yaml", "a: 1\n"));
    assert!(h.query_by_label("Try Operation").is_none() && h.query_by_label("Convert").is_some());
}

#[test]
fn an_html_page_has_no_kind_actions_and_format_says_why_not() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "p.html", "<!DOCTYPE html><p>x</p>"));
    assert!(h.get_by_label("Format").accesskit_node().is_disabled());
    for label in ["XPath", "Schema…", "Convert", "Run", "Sample XML"] {
        assert!(h.query_by_label(label).is_none(), "{label}");
    }
}
