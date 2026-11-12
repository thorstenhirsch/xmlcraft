//! The XPath tab of the Query tool window in the real shell: an expression typed, evaluated on the worker, the result listed, a node selected in the text.

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
    assert!(h.query_by_label("Close Query").is_some(), "the tool window is up");
    assert!(h.query_by_label("→ order.xml").is_some(), "it says which document it works on");
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
    assert!(h.query_by_label("Close Query").is_some(), "the panel stays open");
    assert!(h.query_by_label_contains("1 items").is_some(), "with its result, for the next click");
}

#[test]
fn an_error_shows_its_code_and_escape_gives_the_keyboard_back_to_the_editor_while_the_panel_stays() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "order.xml", ORDER));
    assert!(h.state_mut().execute("xml.xpath"));
    h.run_steps(3);
    evaluate(&mut h, "//item[", "XPST0003");
    // (The keyboard is in the expression field again, as it is for someone who evaluates with Ctrl/Cmd+Enter.)
    h.get_by_label("The XPath expression").focus();
    h.run_steps(2);
    assert!(!h.state().editor_focused());
    h.key_press(egui::Key::Escape);
    h.run_steps(5);
    assert!(h.query_by_label("Close Query").is_some(), "Escape does not close the panel");
    assert!(h.state().editor_focused(), "the editor has the keyboard again");
    // The panel is closed with its x; hidden, it keeps the expression.
    h.get_by_label("Close Query").click();
    h.run_steps(3);
    assert!(h.query_by_label("Close Query").is_none());
    assert!(h.state_mut().execute("xml.xpath"));
    h.run_steps(3);
    let field = h.get_by_label("The XPath expression");
    assert_eq!(field.accesskit_node().value().as_deref(), Some("//item["), "the expression is still there");
}

#[test]
fn a_json_document_is_not_evaluated_and_the_panel_says_what_it_needs() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.json", "{\"a\": 1}"));
    assert!(h.state_mut().execute("xml.xpath"));
    h.run_steps(3);
    assert!(h.query_by_label("Close Query").is_some());
    assert!(h.query_by_label("The XPath expression").is_none(), "no expression field for a JSON document");
    assert!(h.query_by_label_contains("Query needs an XML document").is_some());
    // The tool follows the document: back on an XML document it works again.
    open(&mut h, &file(dir.path(), "order.xml", ORDER));
    h.run_steps(2);
    assert!(h.query_by_label("The XPath expression").is_some());
    assert!(h.query_by_label("→ order.xml").is_some());
}

#[test]
fn the_result_belongs_to_the_document_it_was_asked_on() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "order.xml", ORDER));
    open(&mut h, &file(dir.path(), "other.xml", "<o><p:item xmlns:p=\"urn:p\">x</p:item></o>"));
    assert!(h.state_mut().execute("xml.xpath"));
    h.run_steps(3);
    assert!(h.query_by_label("→ other.xml").is_some());
    evaluate(&mut h, "count(//*)", "xs:integer   2");
    // Another document is shown: its panel has no result yet, and the first one's is back when its tab is.
    h.state_mut().session.set_active(Some(0));
    h.run_steps(3);
    assert!(h.query_by_label("→ order.xml").is_some(), "the tool follows the shown document");
    assert!(h.query_by_label_contains("xs:integer").is_none(), "another document's result is not shown here");
    h.state_mut().session.set_active(Some(1));
    h.run_steps(3);
    assert!(h.query_by_label_contains("xs:integer   2").is_some());
}

#[test]
fn the_steps_of_a_path_are_shown_with_what_each_leaves_when_asked_for() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "order.xml", ORDER));
    assert!(h.state_mut().execute("xml.xpath"));
    h.run_steps(3);
    h.get_by_label_contains("Show the steps").click_accesskit();
    h.run_steps(2);
    evaluate(&mut h, "/order/p:item[@n = 2]", "element Q{urn:p}item");
    assert!(h.query_by_label_contains("/order  →  1 items").is_some(), "the first step leaves the root element");
    assert!(h.query_by_label_contains("/p:item[@n = 2]  →  1 items").is_some(), "the last step leaves the one item");
}
