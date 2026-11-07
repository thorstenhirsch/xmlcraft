//! The tab strip in the real shell: many documents keep the buttons beside the tabs in reach and the shown tab in view, and a tab can be
//! dragged to another place.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_ui_egui::XmlCraftApp;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

fn open(h: &mut Harness<'static, XmlCraftApp>, names: &[&str]) {
    for name in names {
        h.state_mut().session.open(Some(format!("/x/{name}").into()), b"<a/>", None).expect("opens");
    }
    h.run_steps(5);
}

fn order(h: &Harness<'static, XmlCraftApp>) -> Vec<String> {
    h.state().session.documents().iter().map(|d| d.name().to_owned()).collect()
}

#[test]
fn many_documents_leave_new_and_open_in_reach_and_the_shown_tab_in_view() {
    let mut h = shell();
    let names: Vec<String> = (0..40).map(|i| format!("document-number-{i}.xml")).collect();
    open(&mut h, &names.iter().map(String::as_str).collect::<Vec<_>>());
    let width = 1280.0;
    assert!(h.get_by_label("New").rect().right() < width && h.get_by_label("Open").rect().right() < width, "the buttons did not go off the window");
    let shown = h.get_by_label_contains("document-number-39.xml").rect();
    assert!(shown.left() >= 0.0 && shown.right() <= width, "the shown tab is scrolled into view: {shown:?}");
    // Showing another one brings that one into view.
    h.state_mut().session.set_active(Some(0));
    h.run_steps(6);
    let first = h.get_by_label_contains("document-number-0.xml").rect();
    assert!(first.left() >= 0.0 && first.right() <= width, "{first:?}");
}

#[test]
fn a_tab_dragged_along_the_strip_goes_where_it_is_let_go() {
    let mut h = shell();
    open(&mut h, &["a.xml", "b.xml", "c.xml"]);
    let a = h.get_by_label_contains("a.xml").rect().center();
    let c = h.get_by_label_contains("c.xml").rect().center();
    h.hover_at(a);
    h.run_steps(2);
    h.drag_at(a);
    h.run_steps(2);
    for step in 1..=6 {
        h.hover_at(egui::pos2(a.x + (c.x - a.x) * step as f32 / 6.0 + 4.0, a.y));
        h.run_steps(1);
    }
    h.drop_at(egui::pos2(c.x + 4.0, c.y));
    h.run_steps(4);
    assert_eq!(order(&h), ["b.xml", "c.xml", "a.xml"]);
    // The one that was shown is still shown.
    assert_eq!(h.state().session.active_document().map(|d| d.name().to_owned()).as_deref(), Some("c.xml"));
    // A plain click is not a drag: it shows the tab and moves nothing.
    let b = h.get_by_label_contains("b.xml").rect().center();
    h.hover_at(b);
    h.run_steps(2);
    h.get_by_label_contains("b.xml").click_accesskit();
    h.run_steps(3);
    assert_eq!(order(&h), ["b.xml", "c.xml", "a.xml"]);
}

#[test]
fn a_tab_menu_closes_the_others_but_leaves_unsaved_work_open() {
    let mut h = shell();
    open(&mut h, &["a.xml", "b.xml", "c.xml"]);
    h.state_mut().session.set_active(Some(1));
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().type_text("x");
    h.run_steps(2);
    h.get_by_label_contains("a.xml").click_secondary();
    h.run_steps(3);
    h.get_by_label("Close Others").click();
    h.run_steps(3);
    assert_eq!(order(&h), ["a.xml", "b.xml"], "c.xml closed, b.xml still has unsaved changes");
    assert_eq!(h.state().session.active_document().map(|d| d.name().to_owned()).as_deref(), Some("a.xml"));
    assert!(h.state().status.contains("Left 1 other tabs open"), "{}", h.state().status);
}

#[test]
fn a_tab_menu_closes_tabs_to_the_right_and_copies_the_path() {
    let mut h = shell();
    open(&mut h, &["a.xml", "b.xml", "c.xml"]);
    h.get_by_label_contains("a.xml").click_secondary();
    h.run_steps(3);
    h.get_by_label("Close Tabs to the Right").click();
    h.run_steps(3);
    assert_eq!(order(&h), ["a.xml"]);
    h.get_by_label_contains("a.xml").click_secondary();
    h.run_steps(3);
    h.get_by_label("Copy Path").click();
    h.run_steps(2);
    assert_eq!(h.state().status, "Copied /x/a.xml");
}
