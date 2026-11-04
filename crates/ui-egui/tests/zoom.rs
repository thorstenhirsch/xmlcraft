//! View ▸ Zoom In, Zoom Out and Actual Size: the editor's text gets larger and smaller, and the choice is kept.

use egui_kittest::Harness;
use xmlcraft_ui_egui::XmlCraftApp;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h.state_mut().session.open(Some("/x/z.xml".into()), b"<a>\n  <b/>\n</a>", None).expect("opens");
    h.run_steps(4);
    h
}

fn row_height(h: &Harness<'static, XmlCraftApp>) -> f32 {
    h.state().editor_geometry().expect("the editor was drawn").row_height
}

#[test]
fn zooming_in_and_out_changes_the_size_of_the_text_and_actual_size_brings_it_back() {
    let mut h = shell();
    let normal = row_height(&h);
    assert!(h.state_mut().execute("view.zoom_in"));
    assert!(h.state_mut().execute("view.zoom_in"));
    h.run_steps(3);
    assert!((h.state().editor_zoom() - 1.2).abs() < 1e-4);
    assert!(row_height(&h) > normal, "larger text, taller rows");
    assert!(h.state().status.contains("Zoom 120%"), "{}", h.state().status);
    assert!(h.state_mut().execute("view.zoom_out"));
    assert!(h.state_mut().execute("view.zoom_out"));
    assert!(h.state_mut().execute("view.zoom_out"));
    h.run_steps(3);
    assert!(row_height(&h) < normal, "smaller text, shorter rows");
    assert!(h.state_mut().execute("view.zoom_reset"));
    h.run_steps(3);
    assert_eq!(h.state().editor_zoom(), 1.0);
    assert!((row_height(&h) - normal).abs() < 0.01);
}

#[test]
fn zooming_stops_at_the_ends() {
    let mut h = shell();
    for _ in 0..60 {
        assert!(h.state_mut().execute("view.zoom_in"));
    }
    assert_eq!(h.state().editor_zoom(), xmlcraft_ui_egui::zoom::MAX);
    for _ in 0..100 {
        assert!(h.state_mut().execute("view.zoom_out"));
    }
    assert_eq!(h.state().editor_zoom(), xmlcraft_ui_egui::zoom::MIN);
    h.run_steps(3);
}
