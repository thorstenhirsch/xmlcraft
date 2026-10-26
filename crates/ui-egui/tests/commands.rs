//! Commands, shortcuts and the palette in the real shell (egui_kittest).

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_engine::commands::COMMANDS;
use xmlcraft_ui_egui::XmlCraftApp;
use xmlcraft_ui_egui::theme::ThemePreference;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

#[test]
fn unknown_commands_do_nothing_and_say_so() {
    let mut h = shell();
    assert!(!h.state_mut().execute("no.such.command"));
    assert!(!h.state_mut().execute(""));
}

#[test]
fn toggles_flip_and_choices_select() {
    let mut h = shell();
    for (id, read) in [
        ("view.outline", (|a: &XmlCraftApp| a.left_open) as fn(&XmlCraftApp) -> bool),
        ("view.messages", |a| a.bottom_open),
        ("view.entry_helpers", |a| a.right.is_some()),
    ] {
        let before = read(h.state());
        assert_eq!(h.state().command_checked(id), Some(before), "{id} reports its state");
        assert!(h.state_mut().execute(id));
        assert_eq!(read(h.state()), !before, "{id} flips");
        assert!(h.state_mut().execute(id));
        assert_eq!(read(h.state()), before, "{id} flips back");
    }
    for (id, want) in
        [("view.theme.dark", ThemePreference::Dark), ("view.theme.light", ThemePreference::Light), ("view.theme.system", ThemePreference::System)]
    {
        assert!(h.state_mut().execute(id));
        assert_eq!(h.state().theme_preference, want);
        assert_eq!(h.state().command_checked(id), Some(true));
    }
    assert_eq!(h.state().command_checked("file.quit"), None, "an action has no checked state");
}

#[test]
fn quitting_asks_the_window_to_close() {
    let mut h = shell();
    assert!(h.state_mut().execute("file.quit"));
    // The window command goes out with the next frame; the app keeps running until the window says so.
    h.run_steps(2);
}

#[test]
fn the_palette_shortcut_opens_and_escape_closes_it() {
    let mut h = shell();
    assert!(!h.state().palette_open);
    h.key_down_modifiers(Modifiers::COMMAND, Key::K);
    h.key_up_modifiers(Modifiers::COMMAND, Key::K);
    h.run_steps(3);
    assert!(h.state().palette_open, "the palette shortcut opens it");
    assert!(h.query_by_label("Quit").is_some(), "with an empty query every command is listed");
    h.key_down(Key::Escape);
    h.key_up(Key::Escape);
    h.run_steps(3);
    assert!(!h.state().palette_open);
}

#[test]
fn typing_in_the_palette_narrows_it_and_enter_runs_the_first_hit() {
    let mut h = shell();
    assert!(h.state_mut().execute("view.palette"));
    h.run_steps(3);
    h.event(egui::Event::Text("dark".into()));
    h.run_steps(3);
    assert_eq!(h.state().palette_query, "dark");
    assert!(h.query_by_label("Dark theme").is_some());
    assert!(h.query_by_label("Quit").is_none(), "commands that do not match are gone");
    h.key_down(Key::Enter);
    h.key_up(Key::Enter);
    h.run_steps(3);
    assert_eq!(h.state().theme_preference, ThemePreference::Dark, "Enter ran the best match");
    assert!(!h.state().palette_open, "and closed the palette");
}

#[test]
fn clicking_a_hit_runs_it() {
    let mut h = shell();
    h.state_mut().execute("view.palette");
    h.run_steps(3);
    h.get_by_label("Light theme").click();
    h.run_steps(3);
    assert_eq!(h.state().theme_preference, ThemePreference::Light);
    assert!(!h.state().palette_open);
}

#[test]
fn a_query_with_no_match_says_so() {
    let mut h = shell();
    h.state_mut().execute("view.palette");
    h.run_steps(3);
    h.event(egui::Event::Text("zzzzqq".into()));
    h.run_steps(3);
    assert!(h.query_by_label("No matching commands").is_some());
    // Enter with nothing to run closes nothing and changes nothing.
    h.key_down(Key::Enter);
    h.key_up(Key::Enter);
    h.run_steps(2);
    assert_eq!(h.state().theme_preference, ThemePreference::System);
}

#[test]
fn every_shortcut_in_the_registry_is_wired_to_a_command_that_runs() {
    // A command with a shortcut must be one the frontend implements, or the shortcut would silently do nothing.
    let mut h = shell();
    for c in COMMANDS.iter().filter(|c| c.shortcut.is_some() && c.id != "file.quit") {
        assert!(h.state_mut().execute(c.id), "{} has a shortcut but the frontend does not run it", c.id);
        h.state_mut().palette_open = false;
    }
}
