//! The real shell under egui_kittest: what is on screen, and what a click does.

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use xmlcraft_ui_egui::XmlCraftApp;
use xmlcraft_ui_egui::theme::ThemePreference;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

#[test]
fn the_shell_shows_its_chrome_and_panes() {
    let h = shell();
    for label in ["Home", "Menu", "Find tools and commands", "Text", "No document open", "No messages"] {
        assert!(h.query_by_label(label).is_some(), "missing {label:?}");
    }
    // Each pane has a header; some also appear elsewhere (the status bar offers Messages, the rail names Entry Helpers).
    for pane in ["Outline", "Messages", "Entry Helpers"] {
        assert!(h.query_all_by_label(pane).count() >= 1, "missing the {pane} pane");
    }
    for section in ["ELEMENTS", "ATTRIBUTES", "ENTITIES"] {
        assert!(h.query_by_label(section).is_some(), "missing the {section} section");
    }
    assert!(h.query_by_label("XmlCraft").is_some(), "the home card names the app");
}

#[test]
fn the_theme_button_cycles_system_light_dark() {
    let mut h = shell();
    assert_eq!(h.state().theme_preference, ThemePreference::System);
    h.get_by_label("Display theme: Use system setting").click();
    h.run_steps(2);
    assert_eq!(h.state().theme_preference, ThemePreference::Light);
    h.get_by_label("Display theme: Light").click();
    h.run_steps(2);
    assert_eq!(h.state().theme_preference, ThemePreference::Dark);
    h.get_by_label("Display theme: Dark").click();
    h.run_steps(2);
    assert_eq!(h.state().theme_preference, ThemePreference::System);
}

#[test]
fn each_pane_closes_with_its_close_button() {
    let mut h = shell();
    assert!(h.state().left_open && h.state().bottom_open && h.state().right.is_some());
    // Three panes, three Close buttons; each click removes one.
    for expected in (1..=3).rev() {
        assert_eq!(h.query_all_by_label("Close").count(), expected);
        h.get_all_by_label("Close").next().expect("a Close button").click();
        h.run_steps(3);
    }
    assert!(!h.state().left_open && !h.state().bottom_open && h.state().right.is_none());
    assert!(h.query_by_label("No document open").is_none());
    assert!(h.query_by_label("No messages").is_none());
}

#[test]
fn the_rail_reopens_entry_helpers() {
    let mut h = shell();
    h.state_mut().right = None;
    h.run_steps(3);
    assert!(h.query_by_label("ELEMENTS").is_none());
    // The rail is always there; its one button is named after the pane.
    h.get_by_label("Entry Helpers").click();
    h.run_steps(3);
    assert!(h.state().right.is_some());
    assert!(h.query_by_label("ELEMENTS").is_some());
}

#[test]
fn new_and_open_are_in_the_tab_strip_and_work() {
    let mut h = shell();
    assert!(!h.get_by_label("Open").accesskit_node().is_disabled());
    h.get_by_label("New").click();
    h.run_steps(3);
    assert_eq!(h.state().tabs().iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Untitled-1"]);
    assert!(h.query_by_label("XML editor").is_some(), "the editor shows");
}

#[test]
fn a_document_tab_replaces_the_home_view() {
    let mut h = shell();
    assert!(h.query_by_label("XmlCraft").is_some());
    h.state_mut().session.open(Some("/x/invoice.xml".into()), b"<invoice/>", None).expect("opens");
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().type_text(" ");
    h.run_steps(3);
    assert!(h.query_by_label("XmlCraft").is_none(), "no home card over a document");
    assert!(h.query_by_label("invoice.xml (edited)").is_some(), "an unsaved tab says so to assistive technology");
    // The home button returns to the home view.
    h.get_by_label("Home").click();
    h.run_steps(3);
    assert_eq!(h.state().active(), None);
    assert!(h.query_by_label("XmlCraft").is_some());
}
