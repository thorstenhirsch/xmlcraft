//! Entry Helpers in the real shell: the grammar of the DTD is read on a worker thread, so these tests wait for it.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui::Modifiers;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_ui_egui::XmlCraftApp;

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).with_step_dt(0.02).build_eframe(|cc| XmlCraftApp::new(cc));
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
    h.run_steps(3);
}

/// Open a document and put the caret at `¦` of `marked`.
fn open_at(h: &mut Harness<'static, XmlCraftApp>, dir: &Path, marked: &str) {
    let caret = marked.find('¦').expect("a marker");
    let text = marked.replacen('¦', "", 1);
    // Each document is a file of its own: opening a path again would show the document that is open.
    let name = format!("doc{}.xml", h.state().session.documents().len());
    let path = file(dir, &name, &text);
    assert!(h.state_mut().open_path(&path));
    wait(h, "the grammar", |app| app.has_grammar());
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(caret, caret);
    h.run_steps(6);
}

fn text(h: &Harness<'static, XmlCraftApp>) -> String {
    h.state().session.active_document().expect("shown").editor().text()
}

/// A double click on the row `label`. Time passes first, so that an earlier click is not counted with these.
fn double_click(h: &mut Harness<'static, XmlCraftApp>, label: &str) {
    h.run_steps(30);
    // The name of an element is the label of its row here and of its row in the Outline, which follows an edit a little later (on a
    // worker): the row to click is the only one when the Outline has caught up.
    let started = Instant::now();
    while h.query_all_by_label(label).count() > 1 && started.elapsed() < Duration::from_secs(10) {
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    let at = h.get_by_label(label).rect().center();
    h.hover_at(at);
    for pressed in [true, false, true, false] {
        h.event(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
    }
    h.run_steps(3);
}

const DTD: &str = "<!DOCTYPE r [<!ELEMENT r (a, b?, s?)><!ELEMENT a EMPTY><!ATTLIST a id ID #REQUIRED kind (p|q) #IMPLIED><!ELEMENT b (#PCDATA)><!ELEMENT s (b)>\
                   <!ENTITY e 'text'>]>";

#[test]
fn the_elements_the_content_model_allows_here_are_listed_with_the_required_ones_marked() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r>¦</r>"));
    assert!(h.query_by_label("a (required)").is_some(), "a must come first");
    assert!(h.query_by_label("b").is_none(), "b may only come after a");
    // After a, b may come and nothing is required.
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1'/>¦</r>"));
    assert!(h.query_by_label("b").is_some() && h.query_by_label("a (required)").is_none());
}

#[test]
fn a_double_click_puts_an_element_with_what_it_requires_and_undo_takes_it_back() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    let marked = format!("{DTD}<r>¦</r>");
    open_at(&mut h, dir.path(), &marked);
    double_click(&mut h, "a (required)");
    assert_eq!(text(&h), format!("{DTD}<r><a id=\"p\"/></r>").replace("id=\"p\"", "id=\"\""));
    // The caret is in the value that has to be filled in.
    let doc = h.state().session.active_document().expect("shown");
    let caret = doc.editor().primary().head();
    assert_eq!(&doc.editor().text()[caret - 7..caret], "<a id=\"", "{}", doc.editor().text());
    // One undo takes it away.
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().undo();
    assert_eq!(text(&h), marked.replacen('¦', "", 1));
}

#[test]
fn the_option_decides_whether_the_children_a_declaration_requires_are_put_too() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1'/>¦</r>"));
    double_click(&mut h, "s");
    assert_eq!(text(&h), format!("{DTD}<r><a id='1'/><s><b></b></s></r>"), "the option is on at first");
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().undo();
    h.get_by_label("Insert required content").click();
    h.run_steps(3);
    double_click(&mut h, "s");
    assert_eq!(text(&h), format!("{DTD}<r><a id='1'/><s></s></r>"), "off: just the element");
}

#[test]
fn attributes_of_the_element_are_offered_and_added_to_its_start_tag() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1' ¦/></r>"));
    // An attribute that is given is greyed and does not go in twice.
    double_click(&mut h, "id (required)");
    assert!(!text(&h).contains("id='1' id"), "{}", text(&h));
    double_click(&mut h, "kind");
    assert_eq!(text(&h), format!("{DTD}<r><a id='1' kind=\"p\" /></r>"), "the first value of an enumeration is the start");
}

#[test]
fn an_entity_is_put_as_a_reference() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1'/><b>¦</b></r>"));
    double_click(&mut h, "&e;");
    assert_eq!(text(&h), format!("{DTD}<r><a id='1'/><b>&e;</b></r>"));
}

#[test]
fn a_document_without_a_doctype_says_so() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    let path = file(dir.path(), "plain.xml", "<r><a/></r>");
    assert!(h.state_mut().open_path(&path));
    h.run_steps(8);
    assert!(h.query_by_label("No DTD or schema").is_some());
}

#[test]
fn a_doctype_too_big_to_read_says_so() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    let doctype = format!("<!DOCTYPE r [<!--{}--><!ELEMENT r ANY>]>", " ".repeat(xmlcraft_engine::schema::MAX_DOCTYPE_BYTES));
    let path = file(dir.path(), "big.xml", &format!("{doctype}<r/>"));
    assert!(h.state_mut().open_path(&path));
    let started = Instant::now();
    while h.query_by_label("No DTD to show").is_none() {
        assert!(started.elapsed() < Duration::from_secs(20), "waiting for the answer");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(h.query_by_label("Reading the DTD…").is_none());
}

fn edit(h: &mut Harness<'static, XmlCraftApp>, range: std::ops::Range<usize>, with: &str) {
    let doc = h.state_mut().session.active_document_mut().expect("shown");
    assert!(doc.editor_mut().apply_edits(&[xmlcraft_text::Edit::replace(range, with)]));
    h.run_steps(8);
}

#[test]
fn the_choices_follow_the_text_when_the_caret_stays_where_it_is() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1'/>¦</r>"));
    assert!(h.query_by_label("b").is_some() && h.query_by_label("a (required)").is_none());
    // The same number of bytes before the caret, and `a` is not there any more.
    let at = DTD.len() + "<r>".len();
    edit(&mut h, at..at + "<a id='1'/>".len(), "<!--1234-->");
    assert_eq!(h.state().session.active_document().expect("shown").editor().primary().head(), at + 11);
    assert!(h.query_by_label("a (required)").is_some() && h.query_by_label("b").is_none());
}

#[test]
fn what_is_selected_is_replaced_by_what_is_put() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><!--c-->¦</r>"));
    let caret = h.state().session.active_document().expect("shown").editor().primary().head();
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(caret - "<!--c-->".len(), caret);
    h.run_steps(6);
    double_click(&mut h, "a (required)");
    assert_eq!(text(&h), format!("{DTD}<r><a id=\"\"/></r>"));
}

#[test]
fn taking_the_doctype_away_takes_the_choices_away() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r>¦</r>"));
    assert!(h.query_by_label("a (required)").is_some());
    edit(&mut h, 0..DTD.len(), "");
    // The grammar goes when the worker has read the text again; on a busy machine that is not within the first frames.
    wait(&mut h, "the grammar to go", |app| !app.has_grammar());
    assert!(h.query_by_label("No DTD or schema").is_some() && h.query_by_label("a (required)").is_none());
    assert!(!h.state().has_grammar());
}

// --- an XML Schema -------------------------------------------------------------------------------------------------------------------

const XSD: &str = "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\" targetNamespace=\"urn:o\" elementFormDefault=\"qualified\">\
    <xs:element name=\"order\"><xs:complexType><xs:sequence>\
      <xs:element name=\"customer\" type=\"xs:string\"/>\
      <xs:element name=\"state\"><xs:simpleType><xs:restriction base=\"xs:string\"><xs:enumeration value=\"open\"/><xs:enumeration value=\"closed\"/><xs:enumeration value=\"r&amp;d\"/></xs:restriction></xs:simpleType></xs:element>\
      <xs:element name=\"item\" minOccurs=\"0\" maxOccurs=\"unbounded\"><xs:complexType><xs:sequence><xs:element name=\"note\" type=\"xs:string\"/></xs:sequence>\
        <xs:attribute name=\"sku\" type=\"xs:string\" use=\"required\"/>\
        <xs:attribute name=\"kind\"><xs:simpleType><xs:restriction base=\"xs:string\"><xs:enumeration value=\"pen\"/><xs:enumeration value=\"ink\"/></xs:restriction></xs:simpleType></xs:attribute></xs:complexType></xs:element>\
    </xs:sequence></xs:complexType></xs:element></xs:schema>";

const XSI: &str = "xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"";

/// An instance document of the schema, with its content `inside` (the caret `¦` may be in it).
fn instance(inside: &str) -> String {
    format!("<order xmlns=\"urn:o\" {XSI} xsi:schemaLocation=\"urn:o order.xsd\">{inside}</order>")
}

/// Replace the whole text of the document being shown.
fn replace_text(h: &mut Harness<'static, XmlCraftApp>, with: &str) {
    let len = text(h).len();
    edit(h, 0..len, with);
}

fn with_schema(dir: &Path) {
    file(dir, "order.xsd", XSD);
}

#[test]
fn what_a_schema_allows_here_is_listed_with_the_required_ones_marked() {
    let dir = tempfile::tempdir().expect("dir");
    with_schema(dir.path());
    let mut h = shell();
    open_at(&mut h, dir.path(), &instance("¦"));
    assert!(h.query_by_label("customer (required)").is_some(), "customer must come first");
    assert!(h.query_by_label("state").is_none(), "state only after customer");
    open_at(&mut h, dir.path(), &instance("<customer/><state/>¦"));
    assert!(h.query_by_label("item").is_some() && h.query_by_label("customer (required)").is_none());
}

#[test]
fn a_double_click_puts_the_element_with_what_the_schema_requires_and_undo_takes_it_back() {
    let dir = tempfile::tempdir().expect("dir");
    with_schema(dir.path());
    let mut h = shell();
    let marked = instance("<customer/><state/>¦");
    open_at(&mut h, dir.path(), &marked);
    double_click(&mut h, "item");
    assert_eq!(text(&h), instance("<customer/><state/><item sku=\"\"><note></note></item>"), "the required attribute and child are in");
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().undo();
    assert_eq!(text(&h), marked.replacen('¦', "", 1));
    // With the option off, only the element and what it needs for itself.
    h.get_by_label("Insert required content").click();
    h.run_steps(3);
    double_click(&mut h, "item");
    assert_eq!(text(&h), instance("<customer/><state/><item sku=\"\"></item>"));
}

#[test]
fn the_attributes_of_the_type_are_added_to_the_start_tag_with_the_first_value_it_lists() {
    let dir = tempfile::tempdir().expect("dir");
    with_schema(dir.path());
    let mut h = shell();
    open_at(&mut h, dir.path(), &instance("<customer/><state/><item sku='1' ¦/>"));
    double_click(&mut h, "sku (required)");
    assert!(!text(&h).contains("sku='1' sku"), "an attribute that is given does not go in twice: {}", text(&h));
    double_click(&mut h, "kind");
    assert!(text(&h).contains("<item sku='1' kind=\"pen\" />"), "{}", text(&h));
}

#[test]
fn the_values_of_the_type_of_an_element_are_listed_and_put_as_text() {
    let dir = tempfile::tempdir().expect("dir");
    with_schema(dir.path());
    let mut h = shell();
    open_at(&mut h, dir.path(), &instance("<customer/><state>¦</state>"));
    assert!(h.query_by_label("open").is_some() && h.query_by_label("closed").is_some());
    double_click(&mut h, "closed");
    assert_eq!(text(&h), instance("<customer/><state>closed</state>"));
    // A value with markup characters is put as text.
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().undo();
    h.run_steps(8);
    double_click(&mut h, "r&d");
    assert_eq!(text(&h), instance("<customer/><state>r&amp;d</state>"));
}

#[test]
fn the_schema_is_not_loaded_again_for_what_is_typed_in_the_document() {
    let dir = tempfile::tempdir().expect("dir");
    with_schema(dir.path());
    let mut h = shell();
    open_at(&mut h, dir.path(), &instance("<customer/>¦"));
    assert_eq!(h.state().schema_requests(), 1);
    let at = text(&h).find("<customer/>").expect("a place");
    edit(&mut h, at..at, "<!-- typed -->");
    edit(&mut h, at..at, "<!-- more -->");
    assert_eq!(h.state().schema_requests(), 1, "the edits of the document itself do not change what the schema is made of");
    // Another document that is open and is edited does.
    let other = file(dir.path(), "other.xsd", XSD);
    assert!(h.state_mut().open_path(&other));
    h.run_steps(4);
    replace_text(&mut h, XSD);
    let instance_index = h.state().session.documents().iter().position(|d| d.path().is_some_and(|p| p.ends_with("doc0.xml"))).expect("open");
    h.state_mut().session.set_active(Some(instance_index));
    h.run_steps(6);
    assert!(h.state().schema_requests() >= 2, "the schema may be one of the documents that are open");
}

#[test]
fn a_schema_that_is_open_and_edited_is_what_the_choices_come_from() {
    let dir = tempfile::tempdir().expect("dir");
    let xsd = file(dir.path(), "order.xsd", XSD);
    let mut h = shell();
    // The schema is open, and renamed in the editor, not saved.
    assert!(h.state_mut().open_path(&xsd));
    h.run_steps(4);
    let renamed = XSD.replace("name=\"customer\"", "name=\"buyer\"");
    replace_text(&mut h, &renamed);
    // The instance, opened next, is answered from the schema as it is in the editor.
    open_at(&mut h, dir.path(), &instance("¦"));
    assert!(h.query_by_label("buyer (required)").is_some() && h.query_by_label("customer (required)").is_none());
    // The schema is edited again while the instance is shown elsewhere: back in the instance, the choices are the new ones.
    let schema_index = h.state().session.documents().iter().position(|d| d.path().is_some_and(|p| p.ends_with("order.xsd"))).expect("open");
    let instance_index = h.state().session.active().expect("shown");
    h.state_mut().session.set_active(Some(schema_index));
    h.run_steps(4);
    replace_text(&mut h, &XSD.replace("name=\"customer\"", "name=\"client\""));
    h.state_mut().session.set_active(Some(instance_index));
    h.run_steps(4);
    // The schema that was held until now is the old one: the new one is asked for, and the answer is waited for by what it says.
    let started = Instant::now();
    while h.query_by_label("client (required)").is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "the edit of the other document makes a new request");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(h.query_by_label("buyer (required)").is_none());
}

#[test]
fn a_schema_that_cannot_be_read_says_why() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    let path = file(dir.path(), "lost.xml", &instance(""));
    assert!(h.state_mut().open_path(&path));
    let started = Instant::now();
    while h.query_by_label("No schema to show").is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the answer");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(h.query_by_label_contains("order.xsd").is_some(), "it says which schema could not be loaded");
}

#[test]
fn taking_the_schema_away_takes_the_choices_away() {
    let dir = tempfile::tempdir().expect("dir");
    with_schema(dir.path());
    let mut h = shell();
    open_at(&mut h, dir.path(), &instance("¦"));
    assert!(h.query_by_label("customer (required)").is_some());
    let text_now = text(&h);
    let end = text_now.find('>').expect("the end of the start tag") + 1;
    edit(&mut h, 0..end, "<order>");
    wait(&mut h, "the schema to go", |app| !app.has_grammar());
    assert!(h.query_by_label("No DTD or schema").is_some() && h.query_by_label("customer (required)").is_none());
}

#[test]
fn a_doctype_is_what_the_choices_come_from_when_a_schema_is_named_too() {
    let dir = tempfile::tempdir().expect("dir");
    with_schema(dir.path());
    let mut h = shell();
    let both = format!(
        "<!DOCTYPE order [<!ELEMENT order (z)><!ELEMENT z EMPTY>]><order xmlns=\"urn:o\" {XSI} xsi:schemaLocation=\"urn:o order.xsd\">¦</order>"
    );
    open_at(&mut h, dir.path(), &both);
    assert!(
        h.query_by_label("z (required)").is_some() && h.query_by_label("customer (required)").is_none(),
        "the DOCTYPE is what a document is checked against"
    );
    assert_eq!(h.state().schema_requests(), 0, "the schema is not even loaded");
}

#[test]
fn an_attribute_in_a_namespace_brings_the_declaration_of_its_prefix() {
    let dir = tempfile::tempdir().expect("dir");
    file(
        dir.path(),
        "q.xsd",
        "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\" targetNamespace=\"urn:q\" attributeFormDefault=\"qualified\" elementFormDefault=\"qualified\">\
           <xs:element name=\"r\"><xs:complexType><xs:attribute name=\"a\" type=\"xs:string\"/></xs:complexType></xs:element></xs:schema>",
    );
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("<r xmlns=\"urn:q\" {XSI} xsi:schemaLocation=\"urn:q q.xsd\" ¦/>"));
    double_click(&mut h, "ns1:a");
    assert!(text(&h).contains("<r xmlns=\"urn:q\""), "{}", text(&h));
    assert!(text(&h).contains(" xmlns:ns1=\"urn:q\" ns1:a=\"\" />"), "{}", text(&h));
}
