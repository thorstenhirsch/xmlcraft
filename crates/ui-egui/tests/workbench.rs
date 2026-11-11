//! The Workbench under egui_kittest: the panels docked around the editor, a move to the other side, and the arrangement kept across
//! a restart (the preferences saved and read back).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use craft_ui::docking::{Action, Side, Target};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::{Value, json};
use xmlcraft_ui_egui::{ControlClient, Network, Panel, XmlCraftApp};

/// What eframe keeps between runs, in memory.
#[derive(Default)]
struct Memory(HashMap<String, String>);

impl eframe::Storage for Memory {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }

    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.to_owned(), value);
    }

    fn remove_string(&mut self, key: &str) {
        self.0.remove(key);
    }

    fn flush(&mut self) {}
}

fn shell(saved: Option<String>) -> (Harness<'static, XmlCraftApp>, ControlClient) {
    let slot: Arc<Mutex<Option<ControlClient>>> = Arc::new(Mutex::new(None));
    let put = slot.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(move |cc| {
        let mut app = XmlCraftApp::with_saved_preferences(cc, Network::offline(), saved.as_deref());
        *put.lock().expect("lock") = Some(app.attach_control(&cc.egui_ctx));
        app
    });
    h.run_steps(4);
    h.state_mut().session.open(Some("/x/library.xml".into()), b"<library><book/></library>", None).expect("opens");
    h.run_steps(6);
    let client = slot.lock().expect("lock").take().expect("the app attached the control channel");
    (h, client)
}

/// Where each shown panel is, as `ui.state` reports it.
fn layout(h: &mut Harness<'static, XmlCraftApp>, client: &ControlClient) -> Value {
    let rx = client.send("ui.state", Value::Null);
    for _ in 0..30 {
        h.run_steps(1);
        if let Ok(reply) = rx.try_recv() {
            return reply.expect("ui.state answers")["layout"].clone();
        }
    }
    panic!("ui.state: no answer after 30 frames")
}

#[test]
fn the_panels_dock_around_the_editor_move_to_the_right_and_stay_there_after_a_restart() {
    let (mut h, c) = shell(None);
    for panel in [Panel::Outline, Panel::EntryHelpers, Panel::Problems] {
        h.state_mut().set_panel_visible(panel, true);
    }
    h.run_steps(4);
    assert_eq!(layout(&mut h, &c), json!({ "editor": "center", "outline": "left", "entry_helpers": "left", "problems": "bottom" }));
    // Each panel has its tab and its Close button; the editor has neither.
    for label in ["Outline", "Entry Helpers", "Problems"] {
        assert!(h.query_all_by_label(label).count() >= 1, "the {label} tab");
    }
    for label in ["Close Outline", "Close Entry Helpers", "Close Problems"] {
        assert!(h.query_by_label(label).is_some(), "{label}");
    }
    assert!(h.query_by_label("Close Editor").is_none(), "the editor stays");
    assert!(h.query_by_label("No DTD or schema").is_some(), "the Entry Helpers draw");

    h.state_mut()
        .apply_layout(vec![Action::MovePanel { panel: Panel::EntryHelpers, to: Target::NewDock { side: Side::End } }])
        .expect("Entry Helpers move to the end dock");
    h.run_steps(4);
    assert_eq!(layout(&mut h, &c)["entry_helpers"], "right");
    assert_eq!(layout(&mut h, &c)["outline"], "left");
    assert!(h.query_by_label("No DTD or schema").is_some(), "and still draw there");

    // A restart: what the app saves is what the next one reads.
    let mut storage = Memory::default();
    eframe::App::save(h.state_mut(), &mut storage);
    let saved = eframe::Storage::get_string(&storage, XmlCraftApp::STORAGE_KEY);
    assert!(saved.is_some(), "the preferences are saved");
    let (mut again, c2) = shell(saved);
    assert_eq!(layout(&mut again, &c2), json!({ "editor": "center", "outline": "left", "entry_helpers": "right", "problems": "bottom" }));
}
