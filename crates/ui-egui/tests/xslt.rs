//! The Transform window in the real shell: a stylesheet applied, the result opened as a new document.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
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
    assert!(h.query_by_label("Transform with XSLT").is_some(), "the window is up");
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
    assert!(h.query_by_label("Transform with XSLT").is_some(), "the window stays so that another stylesheet can be chosen");
    assert_eq!(names(&h), ["bad.xml", "e.xml"], "no document was made");
}

#[test]
fn nothing_to_transform_with_says_so_and_a_json_document_is_not_transformed() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "d.xml", "<doc/>"));
    assert!(h.state_mut().execute("xml.transform"));
    h.run_steps(3);
    assert!(h.query_by_label("Transform with XSLT").is_none());
    assert!(h.state().status.contains("no stylesheet"), "{}", h.state().status);
    open(&mut h, &file(dir.path(), "a.json", "{}"));
    assert!(h.state_mut().execute("xml.transform"));
    h.run_steps(3);
    assert!(h.query_by_label("Transform with XSLT").is_none());
    assert!(h.state().status.contains("XML"), "{}", h.state().status);
}
