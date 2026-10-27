//! Accessibility-tree snapshots of the real shell: the widgets a screen reader sees (role, label, state), as text, without
//! positions. A change that renames a control, drops a label or breaks a toggle shows up as a diff in review.
//!
//! `UPDATE_SNAPSHOTS=1 cargo test -p xmlcraft-ui-egui --test snapshots` rewrites the files in `tests/snapshots/`.

use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::NodeT;
use xmlcraft_ui_egui::XmlCraftApp;
use xmlcraft_ui_egui::theme::ThemePreference;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.state_mut().theme_preference = ThemePreference::Light;
    h.run_steps(5);
    h
}

fn dump(node: &egui_kittest::Node<'_>, depth: usize, out: &mut String) {
    let a = node.accesskit_node();
    // Text runs repeat their label and break lines where the font's metrics say: they would make a snapshot differ by platform.
    if a.role() == Role::TextRun {
        return;
    }
    let label = a.label();
    let value = a.value();
    let plain = matches!(a.role(), Role::GenericContainer | Role::Unknown | Role::Group | Role::Window | Role::Pane | Role::Label)
        && label.is_none()
        && value.is_none();
    let next = if plain {
        depth
    } else {
        out.push_str(&"  ".repeat(depth));
        out.push_str(&format!("{:?}", a.role()));
        if let Some(label) = label {
            out.push_str(&format!(" {label:?}"));
        }
        if let Some(value) = value {
            out.push_str(&format!(" value={value:?}"));
        }
        if a.is_disabled() {
            out.push_str(" [disabled]");
        }
        if let Some(t) = a.toggled() {
            out.push_str(&format!(" [toggled={}]", format!("{t:?}").to_lowercase()));
        }
        if a.is_selected() == Some(true) {
            out.push_str(" [selected]");
        }
        out.push('\n');
        depth + 1
    };
    for child in node.children() {
        dump(&child, next, out);
    }
}

fn tree_text(h: &Harness<'_, XmlCraftApp>) -> String {
    let mut out = String::new();
    dump(&h.root(), 0, &mut out);
    out
}

fn check(name: &str, actual: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots").join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&path, actual).expect("write the snapshot");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("{} is missing: run with UPDATE_SNAPSHOTS=1", path.display()));
    assert_eq!(actual.replace("\r\n", "\n"), expected.replace("\r\n", "\n"), "the {name} tree changed (UPDATE_SNAPSHOTS=1 to accept)");
}

#[test]
fn the_home_shell() {
    check("shell_home", &tree_text(&shell()));
}

#[test]
fn a_document_tab_with_unsaved_changes() {
    let mut h = shell();
    let session = &mut h.state_mut().session;
    session.open(Some("/x/invoice.xml".into()), b"<invoice/>", None).expect("opens");
    session.active_document_mut().expect("shown").editor_mut().type_text(" ");
    session.open(Some("/x/catalog.xsd".into()), b"<schema/>", None).expect("opens");
    session.set_active(Some(0));
    // The outline and the status badge come from the background analysis: wait for it, or the tree would show "Reading the document…".
    let started = std::time::Instant::now();
    while h.state().problems().is_none() {
        assert!(started.elapsed() < std::time::Duration::from_secs(10), "waiting for the analysis");
        h.run_steps(1);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run_steps(3);
    check("shell_tabs", &tree_text(&h));
}

#[test]
fn the_command_palette_open() {
    let mut h = shell();
    assert!(h.state_mut().execute("view.palette"));
    h.run_steps(4);
    check("palette_open", &tree_text(&h));
}

#[test]
fn every_pane_closed() {
    let mut h = shell();
    for id in ["view.outline", "view.entry_helpers", "view.messages"] {
        assert!(h.state_mut().execute(id));
    }
    h.run_steps(4);
    check("panes_closed", &tree_text(&h));
}
