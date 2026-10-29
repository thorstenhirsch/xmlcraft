//! The About window in the real shell: the version, the licence, who is credited, and the third-party notice.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_ui_egui::XmlCraftApp;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 900.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

fn open(h: &mut Harness<'static, XmlCraftApp>) {
    assert!(h.state_mut().execute("app.about"));
    h.run_steps(4);
}

#[test]
fn the_about_window_says_what_the_app_is_and_who_made_it() {
    let mut h = shell();
    open(&mut h);
    assert!(h.query_by_label(&format!("Version {}", xmlcraft_ui_egui::VERSION)).is_some(), "the version of this build");
    assert!(h.query_by_label("MIT or Apache-2.0, at your option").is_some());
    assert!(h.query_by_label_contains("Independent of Altova GmbH").is_some(), "the clean-room statement");
    assert!(h.query_by_label("CONTRIBUTORS").is_some());
    // The credits compiled into the binary name the person the project belongs to.
    assert!(h.query_all_by_label_contains("@thorstenhirsch").count() >= 1, "the owner is credited by their username");
    assert!(h.query_by_label("AI MODELS").is_some());
    assert!(h.query_by_label("THIRD-PARTY MATERIAL").is_some());
    assert!(h.query_by_label_contains("Lucide icons").is_some(), "the NOTICE of the third-party material is in the window");
}

#[test]
fn the_window_closes_with_its_button_and_with_escape() {
    let mut h = shell();
    open(&mut h);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(h.query_by_label("THIRD-PARTY MATERIAL").is_none(), "the button closes it");
    open(&mut h);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.query_by_label("THIRD-PARTY MATERIAL").is_none(), "Escape closes it");
    // It can be opened again.
    open(&mut h);
    assert!(h.query_by_label("THIRD-PARTY MATERIAL").is_some());
}

#[test]
fn it_is_in_the_help_menu_and_in_the_palette() {
    let spec = xmlcraft_engine::commands::command("app.about").expect("the command is registered");
    assert_eq!((spec.menu, spec.label), (Some("Help"), "About XmlCraft"));
    assert!(xmlcraft_engine::commands::in_menu("Help").any(|c| c.id == "app.about"));
}

#[test]
fn real_names_are_shown_only_when_someone_gave_one_and_the_viewer_asks() {
    let mut h = shell();
    let credits = r#"{"commits":3,"contributors":[
        {"login":"alice","display_name":"Al","real_name":"Alice Example","prs":1,"commits":2},
        {"login":"bob","display_name":null,"real_name":null,"prs":0,"commits":1}],"models":[]}"#;
    h.state_mut().open_about_with_credits(credits);
    h.run_steps(4);
    assert!(h.query_by_label_contains("Al (@alice)").is_some() && h.query_by_label_contains("@bob").is_some());
    assert!(h.query_by_label_contains("Alice Example").is_none(), "a real name is not shown unless asked for");
    h.get_by_label("Show the real names people gave").click();
    h.run_steps(3);
    assert!(h.query_by_label_contains("Alice Example (@alice)").is_some());
    assert!(h.query_by_label_contains("@bob").is_some(), "someone who gave none is shown by their username");
    // With nobody having given a real name there is nothing to ask for; and with nobody listed, the window says so.
    h.state_mut().open_about_with_credits(r#"{"contributors":[{"login":"bob"}]}"#);
    h.run_steps(4);
    assert!(h.query_by_label("Show the real names people gave").is_none());
    h.state_mut().open_about_with_credits("{}");
    h.run_steps(4);
    assert!(h.query_by_label("No contributors are listed in this build.").is_some());
    assert!(h.query_by_label("AI MODELS").is_none(), "no models, no heading");
}
