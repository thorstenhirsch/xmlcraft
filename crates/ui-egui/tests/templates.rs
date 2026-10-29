//! New from Template and the buttons of the home view, in the real shell.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_engine::templates::{TEMPLATES, template};
use xmlcraft_ui_egui::XmlCraftApp;

fn shell() -> Harness<'static, XmlCraftApp> {
    // A frame is 20 ms, as a double click needs (the default step is a quarter of a second).
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 900.0)).with_step_dt(0.02).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

fn documents(h: &Harness<'static, XmlCraftApp>) -> Vec<(String, String)> {
    h.state().session.documents().iter().map(|d| (d.name().to_owned(), d.editor().text())).collect()
}

fn open_dialog(h: &mut Harness<'static, XmlCraftApp>) {
    assert!(h.state_mut().execute("file.new_from_template"));
    h.run_steps(4);
}

#[test]
fn the_dialog_lists_every_template_and_the_one_chosen_becomes_a_new_document_with_its_name() {
    let mut h = shell();
    open_dialog(&mut h);
    for t in TEMPLATES {
        assert!(h.query_by_label(t.title).is_some(), "{} is offered", t.title);
    }
    h.get_by_label("XML Schema").click();
    h.run_steps(2);
    h.get_by_label("Create").click();
    h.run_steps(4);
    assert!(h.query_by_label("New from Template").is_none(), "the dialog is gone");
    let xsd = template("xsd").expect("a template");
    assert_eq!(documents(&h), [(xsd.name.to_owned(), xsd.text.to_owned())], "a document that is not a file yet, with the text and the name");
    assert!(h.state().session.active_document().is_some_and(|d| d.path().is_none()));
}

#[test]
fn enter_creates_the_first_and_a_double_click_creates_the_one_it_is_on() {
    let mut h = shell();
    open_dialog(&mut h);
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    let xml = template("xml").expect("a template");
    assert_eq!(documents(&h), [(xml.name.to_owned(), xml.text.to_owned())], "the first is chosen when nothing was");
    open_dialog(&mut h);
    // Time passes first, so that no earlier click is counted with these.
    h.run_steps(30);
    let at = h.get_by_label("DTD").rect().center();
    h.hover_at(at);
    for pressed in [true, false, true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
    }
    h.run_steps(4);
    let dtd = template("dtd").expect("a template");
    assert_eq!(documents(&h).last(), Some(&(dtd.name.to_owned(), dtd.text.to_owned())));
    assert_eq!(documents(&h).len(), 2);
}

#[test]
fn cancel_and_escape_make_nothing() {
    let mut h = shell();
    open_dialog(&mut h);
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(h.query_by_label("New from Template").is_none() && documents(&h).is_empty());
    open_dialog(&mut h);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.query_by_label("New from Template").is_none() && documents(&h).is_empty());
}

#[test]
fn a_template_that_does_not_exist_makes_nothing() {
    let mut h = shell();
    assert!(!h.state_mut().new_document_from_template("nope"));
    assert!(documents(&h).is_empty());
    assert!(h.state_mut().new_document_from_template("catalog"));
    assert_eq!(documents(&h).len(), 1);
}

#[test]
fn the_command_is_in_the_file_menu_with_its_shortcut() {
    let spec = xmlcraft_engine::commands::command("file.new_from_template").expect("registered");
    assert_eq!((spec.menu, spec.label), (Some("File"), "New from Template…"));
    assert!(spec.shortcut.is_some());
}

#[test]
fn the_home_view_has_the_ways_to_begin() {
    let mut h = shell();
    for label in ["New", "New from Template…", "Open…"] {
        assert!(h.query_all_by_label(label).count() >= 1, "{label} is on the home view");
    }
    // The button of the home view, not the one of the tab strip: the card's.
    let buttons = h.query_all_by_label("New from Template…").count();
    assert!(buttons >= 1);
    h.query_all_by_label("New from Template…").last().expect("a button").click();
    h.run_steps(4);
    assert!(h.query_by_label("New from Template").is_some(), "it opens the dialog");
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    h.query_all_by_label("New").last().expect("a button").click();
    h.run_steps(4);
    assert_eq!(documents(&h).len(), 1, "New makes an untitled document");
    assert!(documents(&h)[0].0.starts_with("Untitled"));
}

#[test]
fn the_open_button_of_the_home_view_asks_for_a_file() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("a.xml");
    std::fs::write(&path, "<a/>").expect("writes");
    let mut h = shell();
    h.state_mut().pick_override = Some(vec![path]);
    h.query_all_by_label("Open…").last().expect("a button").click();
    h.run_steps(8);
    assert_eq!(documents(&h), [("a.xml".to_owned(), "<a/>".to_owned())]);
}
