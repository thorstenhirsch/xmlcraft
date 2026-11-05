//! A YAML document in the real shell: read by YAML's rules, not by the XML ones.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_ui_egui::XmlCraftApp;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

fn file(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path
}

fn open(h: &mut Harness<'static, XmlCraftApp>, path: &Path) {
    assert!(h.state_mut().open_path(path));
    let started = Instant::now();
    while h.state().problems().is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the first analysis");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
}

#[test]
fn yaml_is_checked_as_yaml_outlined_and_offered_no_xml_tools() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "ok.yaml", "alpha:\n  beta: 1\n"));
    assert_eq!(h.state().problems(), Some((0, 0)));
    assert!(h.query_by_label("alpha").is_some() && h.query_by_label("beta 1").is_some(), "the outline names the keys");
    assert!(h.query_by_label("Schema…").is_none() && h.query_by_label("XML…").is_none());
    open(&mut h, &file(dir.path(), "bad.yml", "a: [1, 2\nb: 3\n"));
    assert!(h.state().problems().is_some_and(|(errors, _)| errors > 0));
    // The same text as a file the XML way is not YAML.
    open(&mut h, &file(dir.path(), "a.xml", "<a/>"));
    assert_eq!(h.state().problems(), Some((0, 0)));
    assert!(h.query_by_label("Schema…").is_some());
}

#[test]
fn f8_on_yaml_checks_the_syntax() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "f.yaml", "a: 1\na: 2\n"));
    h.state_mut().execute("xml.validate");
    h.run_steps(4);
    assert_eq!(h.state().problems(), Some((0, 1)), "a repeated key is a warning");
}
