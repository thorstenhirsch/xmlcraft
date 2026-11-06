//! HTML documents in the real shell: read with the rules of HTML (the problems of the markup, not of XML), completed from the table of
//! elements and attributes.

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

fn wait(h: &mut Harness<'static, XmlCraftApp>, what: &str, done: impl Fn(&XmlCraftApp) -> bool) {
    let started = Instant::now();
    while !done(h.state()) {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for {what}");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
}

/// Open `marked` (the caret at `¦`) once it has been analysed.
fn open_at(h: &mut Harness<'static, XmlCraftApp>, dir: &Path, name: &str, marked: &str) {
    let caret = marked.find('¦').unwrap_or(0);
    assert!(h.state_mut().open_path(&file(dir, name, &marked.replacen('¦', "", 1))));
    wait(h, "the analysis", |app| app.problems().is_some());
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

#[test]
fn the_problems_of_html_are_those_of_html_not_of_xml() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    // Void elements, unquoted attributes and left-out end tags are fine in HTML; the span that is not closed before its parent ends is not.
    let page = "<!DOCTYPE html>\n<html>\n<body>\n<p>one<br>two\n<ul><li>a<li>b</ul>\n<img src=x alt=y>\n<div><span>x</div>\n</body>\n</html>\n";
    open_at(&mut h, dir.path(), "page.html", page);
    assert_eq!(h.state().problems(), Some((0, 1)), "one warning: the span");
    assert!(!h.state().underlined().is_empty());
    // The same text as .xml has errors, as XML.
    open_at(&mut h, dir.path(), "page.xml", page);
    assert!(h.state().problems().is_some_and(|(errors, _)| errors > 0), "{:?}", h.state().problems());
}

#[test]
fn elements_attributes_and_values_are_completed_from_what_html_has() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), "d.html", "<body>\n¦\n</body>\n");
    type_text(&mut h, "<di");
    assert!(h.query_by_label("Complete div").is_some() && h.query_by_label("Complete dialog").is_some());
    assert!(h.query_by_label("Complete span").is_none(), "span does not start with di");
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert!(text(&h).contains("<body>\n<div></div>\n</body>"), "{}", text(&h));
    // An attribute of the element being written.
    open_at(&mut h, dir.path(), "e.html", "¦");
    type_text(&mut h, "<a hr");
    assert!(h.query_by_label("Complete href").is_some() && h.query_by_label("Complete hreflang").is_some());
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert_eq!(text(&h), "<a href=\"\"");
    // And a value inside the quotes.
    open_at(&mut h, dir.path(), "f.html", "<input type=\"¦\">");
    type_text(&mut h, "ch");
    assert!(h.query_by_label("Complete checkbox").is_some());
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert_eq!(text(&h), "<input type=\"checkbox\">");
}
