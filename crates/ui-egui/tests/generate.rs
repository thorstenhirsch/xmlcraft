//! Generate Schema and Generate Sample XML in the real shell: the work is done on worker threads, so these tests wait for it.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::Modifiers;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_ui_egui::XmlCraftApp;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 900.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

fn file(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path
}

fn wait(h: &mut Harness<'static, XmlCraftApp>, what: &str, done: impl Fn(&Harness<'static, XmlCraftApp>) -> bool) {
    let started = Instant::now();
    while !done(h) {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for {what}");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
}

fn open(h: &mut Harness<'static, XmlCraftApp>, path: &Path) {
    assert!(h.state_mut().open_path(path));
    wait(h, "the first analysis", |h| h.state().problems().is_some());
}

fn names(h: &Harness<'static, XmlCraftApp>) -> Vec<String> {
    h.state().session.documents().iter().map(|d| d.name().to_owned()).collect()
}

fn text_of(h: &Harness<'static, XmlCraftApp>, name: &str) -> String {
    h.state().session.documents().iter().find(|d| d.name() == name).map(|d| d.editor().text()).unwrap_or_default()
}

const ORDER: &str = "<order id=\"7\"><item sku=\"a\">Tea</item><item sku=\"b\">Coffee</item></order>";
const SCHEMA: &str = "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"><xs:element name=\"order\"><xs:complexType><xs:sequence><xs:element name=\"id\" type=\"xs:int\"/><xs:element name=\"item\" type=\"xs:string\" maxOccurs=\"unbounded\"/></xs:sequence></xs:complexType></xs:element><xs:element name=\"note\" type=\"xs:string\"/></xs:schema>";

#[test]
fn generate_schema_makes_a_new_document_from_the_one_being_shown() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "order.xml", ORDER));
    assert!(h.state_mut().execute("xml.generate_schema"));
    h.run_steps(3);
    assert!(h.query_by_label("Generate Schema").is_some(), "the form is up");
    h.get_by_label("Generate").click();
    wait(&mut h, "the schema", |h| names(h).contains(&"schema.xsd".to_owned()));
    let schema = text_of(&h, "schema.xsd");
    assert!(schema.contains("<xs:element name=\"order\">") && schema.contains("name=\"id\" use=\"required\" type=\"xs:integer\""), "{schema}");
    assert!(h.query_by_label("Generate Schema").is_none(), "the form is gone");
    let shown = h.state().session.active_document().expect("shown");
    assert_eq!(shown.name(), "schema.xsd", "the new document is the one shown");
    assert!(shown.is_dirty() && shown.path().is_none(), "it is not a file yet, so closing it asks");
    assert!(h.state().status.contains("schema.xsd"), "{}", h.state().status);
}

#[test]
fn the_options_of_the_form_are_the_options_of_the_schema() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "order.xml", ORDER));
    assert!(h.state_mut().execute("xml.generate_schema"));
    h.run_steps(3);
    h.get_by_label("DTD").click();
    h.run_steps(2);
    h.get_by_label("Generate").click();
    wait(&mut h, "the DTD", |h| names(h).contains(&"schema.dtd".to_owned()));
    let dtd = text_of(&h, "schema.dtd");
    assert!(dtd.contains("<!ELEMENT order (item+)>") && dtd.contains("<!ATTLIST order"), "{dtd}");
    // Another layout.
    h.get_by_label("order.xml").click();
    h.run_steps(2);
    assert!(h.state_mut().execute("xml.generate_schema"));
    h.run_steps(3);
    h.get_by_label("Venetian blind").click();
    h.run_steps(2);
    h.get_by_label("Generate").click();
    wait(&mut h, "the schema", |h| names(h).iter().filter(|n| *n == "schema.xsd").count() == 1);
    assert!(text_of(&h, "schema.xsd").contains("<xs:complexType name=\"orderType\">"), "{}", text_of(&h, "schema.xsd"));
}

#[test]
fn several_open_documents_can_be_learned_from_together() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "a.xml", "<r><a/></r>"));
    open(&mut h, &file(dir.path(), "b.xml", "<r><a/><b/></r>"));
    assert!(h.state_mut().execute("xml.generate_schema"));
    h.run_steps(3);
    assert!(h.query_by_label("Learn from").is_some(), "with more than one document, they are listed");
    // The one that is shown is checked; add the other (its name is also on a tab, so ask for the check box).
    h.get_by_role_and_label(egui::Role::CheckBox, "a.xml").click();
    h.run_steps(2);
    h.get_by_label("Generate").click();
    wait(&mut h, "the schema", |h| names(h).contains(&"schema.xsd".to_owned()));
    let schema = text_of(&h, "schema.xsd");
    assert!(schema.contains("<xs:element name=\"b\" minOccurs=\"0\">"), "b is in only one of the documents: {schema}");
}

#[test]
fn a_document_that_is_not_well_formed_is_said_in_the_form_and_nothing_is_made() {
    let mut h = shell();
    assert!(!h.state_mut().execute("xml.generate_schema"), "nothing is open");
    h.state_mut().session.new_document();
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().load("<a><b></a>");
    h.run_steps(3);
    assert!(h.state_mut().execute("xml.generate_schema"));
    h.run_steps(3);
    h.get_by_label("Generate").click();
    wait(&mut h, "the answer", |h| h.query_by_label_contains("is not well-formed").is_some());
    assert!(h.query_by_label("Generate Schema").is_some(), "the form stays, for another try");
    assert_eq!(names(&h).len(), 1, "nothing was made");
}

#[test]
fn cancel_and_escape_close_the_form_without_making_anything() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "order.xml", ORDER));
    assert!(h.state_mut().execute("xml.generate_schema"));
    h.run_steps(3);
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(h.query_by_label("Generate Schema").is_none());
    assert!(h.state_mut().execute("xml.generate_schema"));
    h.run_steps(3);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.query_by_label("Generate Schema").is_none());
    assert_eq!(names(&h), vec!["order.xml".to_owned()]);
}

#[test]
fn generate_sample_reads_the_schema_and_makes_a_document_that_names_its_roots() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "order.xsd", SCHEMA));
    assert!(h.state_mut().execute("xml.generate_sample"));
    wait(&mut h, "the schema to be read", |h| h.query_by_label("Reading the schema…").is_none());
    assert!(h.query_by_label("Generate Sample XML").is_some());
    // The elements it can start with are offered.
    h.get_by_label("The element to start with").click();
    h.run_steps(3);
    assert!(h.query_by_label("order").is_some() && h.query_by_label("note").is_some());
    h.get_by_label("note").click();
    h.run_steps(2);
    h.get_by_label("Generate").click();
    wait(&mut h, "the sample", |h| names(h).contains(&"sample.xml".to_owned()));
    let sample = text_of(&h, "sample.xml");
    assert!(sample.contains("<note>string</note>") && !sample.contains("<order"), "{sample}");
    assert!(h.query_by_label("Generate Sample XML").is_none());
}

#[test]
fn the_options_of_generate_sample_shape_the_document() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "order.xsd", SCHEMA));
    assert!(h.state_mut().execute("xml.generate_sample"));
    wait(&mut h, "the schema to be read", |h| h.query_by_label("Reading the schema…").is_none());
    h.get_by_label("the name of its element").click();
    h.run_steps(2);
    h.get_by_label("Say where the schema is (xsi:schemaLocation)").click();
    h.run_steps(2);
    h.get_by_label("Generate").click();
    wait(&mut h, "the sample", |h| names(h).contains(&"sample.xml".to_owned()));
    let sample = text_of(&h, "sample.xml");
    assert!(sample.contains("<item>item</item>") && sample.contains("xsi:noNamespaceSchemaLocation=\"order.xsd\""), "{sample}");
}

#[test]
fn generate_sample_from_a_dtd_and_from_something_that_is_no_grammar() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "lib.dtd", "<!ELEMENT lib (book+)>\n<!ELEMENT book (#PCDATA)>\n"));
    assert!(h.state_mut().execute("xml.generate_sample"));
    wait(&mut h, "the DTD to be read", |h| h.query_by_label("Reading the schema…").is_none());
    h.get_by_label("Generate").click();
    wait(&mut h, "the sample", |h| names(h).contains(&"sample.xml".to_owned()));
    assert!(text_of(&h, "sample.xml").contains("<lib>") && text_of(&h, "sample.xml").contains("<book>"), "{}", text_of(&h, "sample.xml"));
    // A document that is neither is said so, and the button stays off.
    open(&mut h, &file(dir.path(), "plain.xml", "<a><b/></a>"));
    assert!(h.state_mut().execute("xml.generate_sample"));
    wait(&mut h, "the answer", |h| h.query_by_label_contains("error").is_some());
    assert!(h.query_by_label("Generate Sample XML").is_some(), "the form stays");
    h.get_by_label("Cancel").click();
    h.run_steps(2);
    let before = names(&h).len();
    h.key_press_modifiers(Modifiers::NONE, egui::Key::F8);
    h.run_steps(2);
    assert_eq!(names(&h).len(), before, "nothing was made");
}

#[test]
fn a_schema_that_is_open_and_being_edited_is_what_the_sample_is_made_from() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    let path = file(dir.path(), "order.xsd", SCHEMA);
    open(&mut h, &path);
    let editor = h.state_mut().session.active_document_mut().expect("shown").editor_mut();
    let at = editor.text().find("name=\"note\"").expect("the element");
    assert!(editor.apply_edits(&[xmlcraft_text::Edit { range: at + 6..at + 10, text: "memo".to_owned() }]));
    assert!(h.state_mut().execute("xml.generate_sample"));
    wait(&mut h, "the schema to be read", |h| h.query_by_label("Reading the schema…").is_none());
    h.get_by_label("The element to start with").click();
    h.run_steps(3);
    assert!(h.query_by_label("memo").is_some() && h.query_by_label("note").is_none(), "the unsaved text is what was read");
}
