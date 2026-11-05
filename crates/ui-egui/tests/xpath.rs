//! The XPath window in the real shell: an expression typed, evaluated on the worker, the result listed, a node selected in the text.

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

/// Type an expression and press Evaluate; wait until the worker has answered.
fn evaluate(h: &mut Harness<'static, XmlCraftApp>, expression: &str, expect: &str) {
    h.get_by_label("The XPath expression").focus();
    h.run_steps(1);
    h.get_by_label("The XPath expression").type_text(expression);
    h.run_steps(2);
    h.get_by_label("Evaluate").click();
    let started = Instant::now();
    while h.query_by_label_contains(expect).is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for {expect:?}");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
}

const ORDER: &str = "<order xmlns:p=\"urn:p\"><p:item n=\"1\">Tea</p:item><p:item n=\"2\">Milk</p:item></order>";

#[test]
fn an_expression_is_evaluated_and_its_items_are_listed() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "order.xml", ORDER));
    assert!(h.state_mut().execute("xml.xpath"));
    h.run_steps(3);
    assert!(h.query_by_label("Evaluate XPath").is_some(), "the window is up");
    evaluate(&mut h, "count(//p:item)", "xs:integer");
    assert!(h.query_by_label_contains("xs:integer   2").is_some(), "the prefix of the root element is known to the expression");
}

#[test]
fn a_node_is_selected_in_the_text_when_clicked() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "order.xml", ORDER));
    assert!(h.state_mut().execute("xml.xpath"));
    h.run_steps(3);
    evaluate(&mut h, "//p:item[@n = 2]", "element Q{urn:p}item");
    // (A pointer is over a row for a frame before it clicks it, as a hand does.)
    h.get_by_label_contains("element Q{urn:p}item").hover();
    h.run_steps(2);
    h.get_by_label_contains("element Q{urn:p}item").click();
    h.run_steps(3);
    let doc = h.state().session.active_document().expect("shown");
    let text = doc.editor().text();
    let selection = doc.editor().selections()[0];
    let (start, end) = (selection.start().min(selection.end()), selection.start().max(selection.end()));
    assert_eq!(&text[start..end], "<p:item n=\"2\">Milk</p:item>");
    assert!(h.query_by_label("Evaluate XPath").is_none(), "the window closes");
}

#[test]
fn an_error_shows_its_code_and_the_expression_is_kept_for_next_time() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "order.xml", ORDER));
    assert!(h.state_mut().execute("xml.xpath"));
    h.run_steps(3);
    evaluate(&mut h, "//item[", "XPST0003");
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    h.run_steps(2);
    assert!(h.query_by_label("Evaluate XPath").is_none());
    assert!(h.state_mut().execute("xml.xpath"));
    h.run_steps(3);
    let field = h.get_by_label("The XPath expression");
    assert_eq!(field.accesskit_node().value().as_deref(), Some("//item["), "the last expression is back");
}

#[test]
fn a_json_document_is_not_evaluated() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.json", "{\"a\": 1}"));
    assert!(h.state_mut().execute("xml.xpath"));
    h.run_steps(3);
    assert!(h.query_by_label("Evaluate XPath").is_none(), "no window for a JSON document");
    assert!(h.state().status.contains("XML"), "{}", h.state().status);
}
