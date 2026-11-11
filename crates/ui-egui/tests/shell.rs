//! The real shell under egui_kittest: what is on screen, and what a click does.

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use xmlcraft_ui_egui::theme::ThemePreference;
use xmlcraft_ui_egui::{Panel, XmlCraftApp};

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

/// A shell with a document open: the panes, the rail and the header are about a document, and the home view has none of them.
fn shell_with_document() -> Harness<'static, XmlCraftApp> {
    let mut h = shell();
    h.state_mut().session.open(Some("/x/library.xml".into()), b"<library><book/></library>", None).expect("opens");
    h.run_steps(6);
    h
}

#[test]
fn the_shell_shows_its_chrome_and_the_home_view_has_the_whole_window() {
    let h = shell();
    for label in ["Home", "Menu", "Find tools and commands", "XmlCraft", "New document", "New from Template…", "Open file…"] {
        assert!(h.query_by_label(label).is_some(), "missing {label:?}");
    }
    // The panes and the header are about a document.
    for label in ["Outline", "Problems", "Entry Helpers", "Validate"] {
        assert!(h.query_by_label(label).is_none(), "{label:?} is for a document");
    }
    // The rail is there, with the document's tool windows disabled and the others not.
    for label in ["Toggle Outline", "Toggle Entry Helpers", "Toggle Problems"] {
        assert!(h.get_by_label(label).accesskit_node().is_disabled(), "{label:?} is for a document");
        assert_ne!(h.get_by_label(label).accesskit_node().toggled(), Some(egui::accesskit::Toggled::True), "{label:?} is not shown here");
    }
    assert!(!h.get_by_label("Toggle Search").accesskit_node().is_disabled());
    assert!(!h.get_by_label("Open Preferences").accesskit_node().is_disabled());
}

#[test]
fn a_document_has_its_panes_a_rail_to_toggle_them_and_the_actions_that_matter() {
    let h = shell_with_document();
    for label in ["Validate", "Check", "Format", "Assign Schema…", "Schema…", "Toggle Outline", "Toggle Entry Helpers", "Toggle Problems"] {
        assert!(h.query_by_label(label).is_some(), "missing {label:?}");
    }
    // Each pane has a header; some also appear elsewhere.
    for pane in ["Outline", "Problems", "Entry Helpers"] {
        assert!(h.query_all_by_label(pane).count() >= 1, "missing the {pane} pane");
    }
    assert!(h.query_by_label("No DTD or schema").is_some(), "Entry Helpers says what it is for");
    // The header says what the document is: its element path, and where it stands.
    assert!(h.query_by_label("Select element library").is_some(), "the path of the caret");
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
    let mut h = shell_with_document();
    assert!(h.state().panel_visible(Panel::Outline) && h.state().panel_visible(Panel::Problems) && h.state().panel_visible(Panel::EntryHelpers));
    // Three panes, three Close buttons, each named after its pane; each click removes one.
    for label in ["Close Outline", "Close Entry Helpers", "Close Problems"] {
        h.get_by_label(label).click();
        h.run_steps(3);
        assert!(h.query_by_label(label).is_none(), "{label}");
    }
    assert!(!h.state().panel_visible(Panel::Outline) && !h.state().panel_visible(Panel::Problems) && !h.state().panel_visible(Panel::EntryHelpers));
    assert!(h.query_by_label("No DTD or schema").is_none());
}

#[test]
fn the_rail_toggles_the_panes() {
    let mut h = shell_with_document();
    h.state_mut().set_panel_visible(Panel::EntryHelpers, false);
    h.run_steps(3);
    assert!(h.query_by_label("No DTD or schema").is_none());
    // A rail button toggles the pane it is named after.
    h.get_by_label("Toggle Entry Helpers").click();
    h.run_steps(3);
    assert!(h.state().panel_visible(Panel::EntryHelpers));
    assert!(h.query_by_label("No DTD or schema").is_some());
    h.get_by_label("Toggle Entry Helpers").click();
    h.run_steps(3);
    assert!(!h.state().panel_visible(Panel::EntryHelpers), "and again closes it");
    h.get_by_label("Toggle Problems").click();
    h.run_steps(3);
    assert!(!h.state().panel_visible(Panel::Problems));
    let outline = h.state().panel_visible(Panel::Outline);
    h.get_by_label("Toggle Outline").click();
    h.run_steps(3);
    assert_eq!(h.state().panel_visible(Panel::Outline), !outline, "the Outline toggles too");
}

#[test]
fn new_opens_an_untitled_document_and_open_is_enabled() {
    let mut h = shell();
    assert!(!h.get_by_label("Open").accesskit_node().is_disabled());
    // The tab strip's button is the first; the home view has one of its own.
    h.query_all_by_label("New").next().expect("the button of the tab strip").click();
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
