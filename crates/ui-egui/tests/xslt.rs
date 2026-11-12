//! The XSLT tab of the Query tool window in the real shell: a stylesheet applied, the result opened as a new document.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use xmlcraft_ui_egui::XmlCraftApp;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 900.0)).build_eframe(|cc| XmlCraftApp::new(cc));
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
    h.run_steps(2);
}

const XSL: &str = "<xsl:stylesheet version=\"3.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\"><xsl:template match=\"/\"><out><xsl:for-each select=\"//item\"><i><xsl:value-of select=\".\"/></i></xsl:for-each></out></xsl:template></xsl:stylesheet>";

fn names(h: &Harness<'static, XmlCraftApp>) -> Vec<String> {
    h.state().session.documents().iter().map(|d| d.name().to_owned()).collect()
}

#[test]
fn the_stylesheet_a_document_names_is_applied_and_the_result_opens_as_a_new_document() {
    let dir = tempfile::tempdir().expect("dir");
    file(dir.path(), "style.xsl", XSL);
    let mut h = shell();
    open(&mut h, &file(dir.path(), "d.xml", "<?xml-stylesheet type=\"text/xsl\" href=\"style.xsl\"?>\n<doc><item>a</item><item>b</item></doc>"));
    assert!(h.state_mut().execute("xml.transform"));
    h.run_steps(3);
    assert!(h.query_by_label("Close Query").is_some(), "the tool window is up, on its XSLT tab");
    assert!(h.query_by_label("→ d.xml").is_some());
    assert!(h.query_by_label("This document is the source").is_some());
    h.get_by_label("Transform").click();
    let started = Instant::now();
    while !names(&h).contains(&"d-result.xml".to_owned()) {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the result: {:?} {}", names(&h), h.state().status);
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    let shown = h.state().session.active_document().expect("shown");
    assert_eq!((shown.name(), shown.editor().text().as_str()), ("d-result.xml", "<out><i>a</i><i>b</i></out>"));
    assert!(h.state().status.contains("d-result.xml"), "{}", h.state().status);
    assert!(names(&h).contains(&"d.xml".to_owned()), "the source is untouched");
}

#[test]
fn another_open_document_can_be_the_stylesheet() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "style.xsl", XSL));
    open(&mut h, &file(dir.path(), "d.xml", "<doc><item>x</item></doc>"));
    assert!(h.state_mut().execute("xml.transform"));
    h.run_steps(3);
    h.get_by_label("Transform").click();
    let started = Instant::now();
    while !names(&h).contains(&"d-result.xml".to_owned()) {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the result");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_stylesheet_that_is_not_one_is_said_in_the_window_which_stays() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "bad.xml", "<not-a-stylesheet/>"));
    open(&mut h, &file(dir.path(), "e.xml", "<doc/>"));
    assert!(h.state_mut().execute("xml.transform"));
    h.run_steps(3);
    h.get_by_label("Transform").click();
    let started = Instant::now();
    while h.query_by_label_contains("cannot be used").is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the message");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(h.query_by_label("Close Query").is_some(), "the panel stays so that another stylesheet can be chosen");
    assert_eq!(names(&h), ["bad.xml", "e.xml"], "no document was made");
}

#[test]
fn a_stylesheet_in_front_is_the_stylesheet_and_the_source_is_chosen_from_the_open_documents() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "d.xml", "<doc><item>x</item><item>y</item></doc>"));
    open(&mut h, &file(dir.path(), "style.xsl", XSL));
    assert!(h.state_mut().execute("xml.transform"));
    h.run_steps(3);
    assert!(h.query_by_label("→ style.xsl").is_some());
    assert!(h.query_by_label("This document is the stylesheet").is_some(), "the roles follow the document in front");
    h.get_by_label("Transform").click();
    let started = Instant::now();
    while !names(&h).contains(&"d-result.xml".to_owned()) {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the result: {:?} {}", names(&h), h.state().status);
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    let shown = h.state().session.active_document().expect("shown");
    assert_eq!((shown.name(), shown.editor().text().as_str()), ("d-result.xml", "<out><i>x</i><i>y</i></out>"), "named after the source");
    assert!(names(&h).contains(&"style.xsl".to_owned()) && names(&h).contains(&"d.xml".to_owned()), "both are untouched");
}

#[test]
fn nothing_to_transform_with_says_so_and_a_json_document_is_not_transformed() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "d.xml", "<doc/>"));
    assert!(h.state_mut().execute("xml.transform"));
    h.run_steps(3);
    assert!(h.query_by_label("Close Query").is_some());
    assert!(h.query_by_label_contains("names no stylesheet").is_some(), "the panel says what is missing");
    assert!(h.get_by_label("Transform").accesskit_node().is_disabled(), "and the action waits for a stylesheet");
    // The tool follows the document: a stylesheet opened later is on offer at once.
    open(&mut h, &file(dir.path(), "s.xsl", XSL));
    h.state_mut().session.set_active(Some(0));
    h.run_steps(3);
    assert!(h.query_by_label_contains("names no stylesheet").is_none(), "the list is recomputed from the open documents");
    open(&mut h, &file(dir.path(), "a.json", "{}"));
    h.run_steps(3);
    assert!(h.query_by_label_contains("Query needs an XML document").is_some());
}
