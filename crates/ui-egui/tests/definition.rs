//! F12 in the real shell: where an XML Schema name is defined, in the same document or in an included one.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use xmlcraft_ui_egui::XmlCraftApp;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.state_mut().set_validate_delay(Duration::from_secs(3600));
    h.run_steps(4);
    h
}

fn file(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path
}

fn wait(h: &mut Harness<'static, XmlCraftApp>, what: &str, done: impl Fn(&XmlCraftApp) -> bool) {
    let started = Instant::now();
    while !done(h.state()) {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for {what}");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
}

fn open(h: &mut Harness<'static, XmlCraftApp>, path: &Path) {
    assert!(h.state_mut().open_path(path));
    wait(h, "the first analysis", |app| app.problems().is_some());
}

fn place(h: &mut Harness<'static, XmlCraftApp>, offset: usize) {
    h.state_mut().session.active_document_mut().expect("a document").editor_mut().set_selection(offset, offset);
}

#[test]
fn f12_selects_the_type_the_name_refers_to() {
    let dir = tempfile::tempdir().expect("dir");
    let text = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:t" xmlns:t="urn:t">
  <xs:element name="order" type="t:OrderType"/>
  <xs:complexType name="OrderType"><xs:sequence/></xs:complexType>
</xs:schema>"#;
    let mut h = shell();
    open(&mut h, &file(dir.path(), "order.xsd", text));
    place(&mut h, text.find("t:OrderType").expect("the name") + 2);
    assert!(h.state_mut().execute("xml.go_to_definition"));
    let start = text.rfind("OrderType").expect("the definition");
    wait(&mut h, "the definition", |app| {
        app.session.active_document().is_some_and(|doc| {
            let selection = doc.editor().primary();
            (selection.start(), selection.end()) == (start, start + "OrderType".len())
        })
    });
    assert!(h.state().status.contains("Went to the definition"), "{}", h.state().status);
}

#[test]
fn f12_opens_the_included_document_the_definition_is_in() {
    let dir = tempfile::tempdir().expect("dir");
    let types = file(
        dir.path(),
        "types.xsd",
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:t"><xs:complexType name="OrderType"><xs:sequence/></xs:complexType></xs:schema>"#,
    );
    let root = file(
        dir.path(),
        "root.xsd",
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:t" xmlns:t="urn:t"><xs:include schemaLocation="types.xsd"/><xs:element name="order" type="t:OrderType"/></xs:schema>"#,
    );
    let root_text = std::fs::read_to_string(&root).expect("the root");
    let types_text = std::fs::read_to_string(&types).expect("the types");
    let mut h = shell();
    open(&mut h, &root);
    place(&mut h, root_text.find("t:OrderType").expect("the name") + 2);
    assert!(h.state_mut().execute("xml.go_to_definition"));
    let start = types_text.find("OrderType").expect("the definition");
    wait(&mut h, "the included document", |app| {
        app.session.active_document().is_some_and(|doc| {
            let selection = doc.editor().primary();
            doc.name() == "types.xsd" && (selection.start(), selection.end()) == (start, start + "OrderType".len())
        })
    });
    assert!(h.state().status.contains("Went to the definition in types.xsd"), "{}", h.state().status);
}

#[test]
fn f12_says_when_the_name_has_no_definition() {
    let dir = tempfile::tempdir().expect("dir");
    let text = r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:element name="a" type="missing"/></xs:schema>"#;
    let mut h = shell();
    open(&mut h, &file(dir.path(), "missing.xsd", text));
    place(&mut h, text.find("missing").expect("the name") + 2);
    assert!(h.state_mut().execute("xml.go_to_definition"));
    wait(&mut h, "the answer", |app| app.status == "No definition here");
}

#[test]
fn f12_does_not_select_a_range_in_an_open_document_that_changed_after_it_was_asked() {
    let dir = tempfile::tempdir().expect("dir");
    let types = file(
        dir.path(),
        "types.xsd",
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:t"><xs:complexType name="OrderType"><xs:sequence/></xs:complexType></xs:schema>"#,
    );
    let root = file(
        dir.path(),
        "root.xsd",
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:t" xmlns:t="urn:t"><xs:include schemaLocation="types.xsd"/><xs:element name="order" type="t:OrderType"/></xs:schema>"#,
    );
    let root_text = std::fs::read_to_string(&root).expect("the root");
    let mut h = shell();
    open(&mut h, &types);
    open(&mut h, &root);
    place(&mut h, root_text.find("t:OrderType").expect("the name") + 2);
    assert!(h.state_mut().execute("xml.go_to_definition"));
    // The included document is edited before the answer is taken in: what was found is about the text it had.
    let index = h.state().session.documents().iter().position(|d| d.name() == "types.xsd").expect("types is open");
    let doc = h.state_mut().session.document_mut(index).expect("types");
    doc.editor_mut().set_selection(0, 0);
    doc.editor_mut().type_text("<!-- moved -->");
    wait(&mut h, "the answer", |app| app.status.contains("changed meanwhile"));
    assert_eq!(h.state().session.active_document().map(|d| d.name().to_owned()), Some("root.xsd".to_owned()), "nothing else is shown");
}
