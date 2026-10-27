//! The control channel driving the real shell (egui_kittest): what an agent sees and what its requests do.

use std::sync::{Arc, Mutex};

use egui_kittest::Harness;
use serde_json::{Value, json};
use xmlcraft_ui_egui::theme::ThemePreference;
use xmlcraft_ui_egui::{ControlClient, ControlReply, XmlCraftApp};

fn shell() -> (Harness<'static, XmlCraftApp>, ControlClient) {
    let slot: Arc<Mutex<Option<ControlClient>>> = Arc::new(Mutex::new(None));
    let put = slot.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(move |cc| {
        let mut app = XmlCraftApp::new(cc);
        *put.lock().expect("lock") = Some(app.attach_control(&cc.egui_ctx));
        app
    });
    h.run_steps(4);
    let client = slot.lock().expect("lock").take().expect("the app attached the control channel");
    (h, client)
}

/// Send a request and run frames until it is answered.
fn ask(h: &mut Harness<'static, XmlCraftApp>, client: &ControlClient, method: &str, params: Value) -> ControlReply {
    let rx = client.send(method, params);
    for _ in 0..30 {
        h.run_steps(1);
        if let Ok(reply) = rx.try_recv() {
            return reply;
        }
    }
    Err(format!("{method}: no answer after 30 frames"))
}

#[test]
fn state_describes_the_app_and_its_window() {
    let (mut h, c) = shell();
    let state = ask(&mut h, &c, "ui.state", Value::Null).expect("state");
    assert_eq!(state["app"], "XmlCraft");
    assert_eq!(state["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(state["theme_preference"], "system");
    assert_eq!(state["panes"], json!({"outline": true, "entry_helpers": true, "messages": true}));
    assert_eq!(state["palette"]["open"], false);
    assert_eq!(state["window"]["width"], 1280.0);
    assert_eq!(state["tabs"], json!([]));
}

#[test]
fn inspect_lists_widgets_with_labels_and_can_filter() {
    let (mut h, c) = shell();
    let all = ask(&mut h, &c, "ui.inspect", json!({})).expect("inspect");
    assert!(all["count"].as_u64().is_some_and(|n| n > 10), "{all}");
    let menu = ask(&mut h, &c, "ui.inspect", json!({"query": "menu", "role": "button"})).expect("inspect");
    let widgets = menu["widgets"].as_array().expect("a list");
    assert!(widgets.iter().any(|w| w["label"] == "Menu" && w["clickable"] == true && w["enabled"] == true), "{menu}");
    assert!(widgets.iter().all(|w| w["rect"].as_array().is_some_and(|r| r.len() == 4)));
    let limited = ask(&mut h, &c, "ui.inspect", json!({"limit": 3})).expect("inspect");
    assert_eq!(limited["widgets"].as_array().map(Vec::len), Some(3));
    assert_eq!(limited["truncated"], true);
}

#[test]
fn clicking_by_label_does_what_the_user_would() {
    let (mut h, c) = shell();
    let r = ask(&mut h, &c, "ui.click", json!({"label": "Display theme: Use system setting"})).expect("click");
    assert_eq!(r["label"], "Display theme: Use system setting");
    h.run_steps(2);
    assert_eq!(h.state().theme_preference, ThemePreference::Light);
    // A disabled widget cannot be clicked, and an agent is told why.
    let err = ask(&mut h, &c, "ui.click", json!({"label": "Open"})).expect_err("disabled");
    assert!(err.contains("no enabled clickable widget"), "{err}");
    let err = ask(&mut h, &c, "ui.click", json!({"label": "Nothing like this"})).expect_err("missing");
    assert!(err.contains("ui.inspect"), "{err}");
    let err = ask(&mut h, &c, "ui.click", json!({})).expect_err("no target");
    assert!(err.contains("pass id, label, or x and y"), "{err}");
}

#[test]
fn clicking_by_id_and_by_coordinates_works() {
    let (mut h, c) = shell();
    let found = ask(&mut h, &c, "ui.inspect", json!({"query": "Display theme"})).expect("inspect");
    let id = found["widgets"][0]["id"].as_str().expect("an id").to_owned();
    ask(&mut h, &c, "ui.click", json!({"id": id})).expect("click by id");
    h.run_steps(2);
    assert_eq!(h.state().theme_preference, ThemePreference::Light);
    assert!(ask(&mut h, &c, "ui.click", json!({"id": "123456789"})).expect_err("stale").contains("no widget with id"));
    let rect = found["widgets"][0]["rect"].as_array().expect("rect");
    let (x, y) = (
        (rect[0].as_f64().unwrap_or(0.0) + rect[2].as_f64().unwrap_or(0.0)) / 2.0,
        (rect[1].as_f64().unwrap_or(0.0) + rect[3].as_f64().unwrap_or(0.0)) / 2.0,
    );
    ask(&mut h, &c, "ui.click", json!({"x": x, "y": y})).expect("click at a point");
    h.run_steps(3);
    assert_eq!(h.state().theme_preference, ThemePreference::Dark, "the pointer click on the same button cycled the theme again");
}

#[test]
fn commands_run_by_id_and_are_listed() {
    let (mut h, c) = shell();
    let list = ask(&mut h, &c, "ui.commands", Value::Null).expect("commands");
    let commands = list["commands"].as_array().expect("a list");
    assert!(commands.iter().any(|c| c["id"] == "view.palette" && c["enabled"] == true && c["shortcut"] == "Ctrl+K"));
    assert!(commands.iter().any(|c| c["id"] == "view.messages" && c["checked"] == true));
    ask(&mut h, &c, "ui.command", json!({"id": "view.theme.dark"})).expect("runs");
    assert_eq!(h.state().theme_preference, ThemePreference::Dark);
    let err = ask(&mut h, &c, "ui.command", json!({"id": "no.such"})).expect_err("unknown");
    assert!(err.contains("unknown command"), "{err}");
}

#[test]
fn typing_and_keys_reach_the_focused_widget() {
    let (mut h, c) = shell();
    ask(&mut h, &c, "ui.command", json!({"id": "view.palette"})).expect("opens");
    h.run_steps(3);
    ask(&mut h, &c, "ui.type", json!({"text": "light"})).expect("types");
    assert_eq!(h.state().palette_query, "light");
    ask(&mut h, &c, "ui.key", json!({"key": "Enter"})).expect("presses Enter");
    h.run_steps(3);
    assert_eq!(h.state().theme_preference, ThemePreference::Light);
    // A shortcut sent as a key press with modifiers.
    ask(&mut h, &c, "ui.key", json!({"key": "K", "modifiers": ["command"]})).expect("presses the palette shortcut");
    h.run_steps(3);
    assert!(h.state().palette_open);
    let err = ask(&mut h, &c, "ui.key", json!({"key": "NoSuchKey"})).expect_err("unknown key");
    assert!(err.contains("unknown key"), "{err}");
    assert!(ask(&mut h, &c, "ui.key", json!({"key": "A", "modifiers": ["hyper"]})).expect_err("unknown modifier").contains("modifier"));
}

#[test]
fn quitting_asks_first_when_something_would_be_lost() {
    let (mut h, c) = shell();
    h.state_mut().tabs = vec![xmlcraft_ui_egui::TabInfo { name: "a.xml".into(), dirty: true }];
    let r = ask(&mut h, &c, "ui.quit", json!({})).expect("answers");
    assert_eq!(r, json!({"quitting": false, "needs_confirmation": true}), "an agent cannot discard unsaved work by quitting");
    h.state_mut().tabs.clear();
    let r = ask(&mut h, &c, "ui.quit", json!({})).expect("answers");
    assert_eq!(r, json!({"quitting": true, "needs_confirmation": false}));
}

#[test]
fn resizing_is_bounded_and_unknown_methods_say_what_exists() {
    let (mut h, c) = shell();
    assert!(ask(&mut h, &c, "ui.resize", json!({"width": 100, "height": 700})).expect_err("too small").contains("between 320 and 8192"));
    assert!(ask(&mut h, &c, "ui.resize", json!({"width": 1e9, "height": 700})).is_err());
    assert!(
        ask(&mut h, &c, "ui.resize", json!({"width": 900, "height": 700, "extra": 1})).expect_err("unknown parameter").contains("unknown parameter")
    );
    assert!(ask(&mut h, &c, "ui.resize", json!({"width": 900, "height": 700})).is_ok());
    let err = ask(&mut h, &c, "ui.nope", json!({})).expect_err("unknown");
    assert!(err.contains("ui.state") && err.contains("ui.screenshot"), "{err}");
    assert!(ask(&mut h, &c, "ui.state", json!({"x": 1})).expect_err("takes none").contains("takes no parameters"));
    assert!(ask(&mut h, &c, "ui.screenshot", json!({"region": [0, 0, 1]})).expect_err("bad region").contains("region"));
}

#[test]
fn a_request_after_the_app_is_gone_is_answered_not_lost() {
    let (h, c) = shell();
    drop(h);
    let rx = c.send("ui.state", Value::Null);
    assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(2)), Ok(Err("the app has closed".to_owned())));
}
