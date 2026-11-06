//! Completion in an XSLT stylesheet in the real shell: the `xsl:` elements that fit the place, the attributes, the values and the names of
//! the stylesheet, and function names in an expression, offered while typing and written when chosen.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_ui_egui::XmlCraftApp;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).with_step_dt(0.02).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

fn file(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path
}

/// Open `marked` (the caret at `¦`) once it has been analysed.
fn open_at(h: &mut Harness<'static, XmlCraftApp>, dir: &Path, name: &str, marked: &str) {
    let caret = marked.find('¦').unwrap_or(0);
    assert!(h.state_mut().open_path(&file(dir, name, &marked.replacen('¦', "", 1))));
    let started = Instant::now();
    while h.state().problems().is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the analysis");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(caret, caret);
    h.run_steps(4);
}

fn text(h: &Harness<'static, XmlCraftApp>) -> String {
    h.state().session.active_document().expect("shown").editor().text()
}

fn type_text(h: &mut Harness<'static, XmlCraftApp>, typed: &str) {
    h.get_by_label("XML editor").type_text(typed);
    h.run_steps(4);
}

const HEAD: &str = "<xsl:stylesheet version=\"3.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\">\n";

#[test]
fn instructions_attributes_names_and_functions_are_completed_in_a_stylesheet() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    // An instruction in a template: only those that start with what was typed.
    open_at(&mut h, dir.path(), "a.xsl", &format!("{HEAD}<xsl:template name=\"t\" match=\"/\">\n¦\n</xsl:template>\n</xsl:stylesheet>\n"));
    type_text(&mut h, "<xsl:ca");
    assert!(h.query_by_label("Complete xsl:call-template").is_some(), "offered");
    assert!(h.query_by_label("Complete xsl:template").is_none(), "a declaration does not stand in a template");
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert!(text(&h).contains("<xsl:call-template name=\"\"></xsl:call-template>"), "{}", text(&h));
    // The caret is inside the quotes of `name`, where the names of the stylesheet are offered.
    type_text(&mut h, "t");
    assert!(h.query_by_label("Complete t").is_some(), "the name of the template defined above");
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert!(text(&h).contains("<xsl:call-template name=\"t\"></xsl:call-template>"), "{}", text(&h));
    // An attribute of the element being written.
    open_at(&mut h, dir.path(), "b.xsl", &format!("{HEAD}<xsl:template match=\"/\">\n<xsl:sort ¦"));
    type_text(&mut h, "ord");
    assert!(h.query_by_label("Complete order").is_some());
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert!(text(&h).ends_with("<xsl:sort order=\"\""), "{}", text(&h));
    // A function in an expression.
    open_at(&mut h, dir.path(), "c.xsl", &format!("{HEAD}<xsl:template match=\"/\"><xsl:value-of select=\"¦\"/>"));
    type_text(&mut h, "string-j");
    assert!(h.query_by_label("Complete string-join").is_some());
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert!(text(&h).contains("select=\"string-join()\""), "{}", text(&h));
}

#[test]
fn a_document_that_is_no_stylesheet_is_completed_as_before() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), "plain.xml", "<root><item/>\n¦\n</root>\n");
    type_text(&mut h, "<it");
    assert!(h.query_by_label("Complete item").is_some(), "the names this document uses");
    assert!(h.query_by_label_contains("xsl:").is_none());
}
