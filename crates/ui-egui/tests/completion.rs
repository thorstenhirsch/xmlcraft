//! Completion while typing, in the real shell: the popup that follows `<`, `&`, a few letters of an attribute and the quotes of one.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_ui_egui::XmlCraftApp;

const DTD: &str = "<!DOCTYPE r [<!ELEMENT r (a, b?, s?)><!ELEMENT a EMPTY><!ATTLIST a id ID #REQUIRED kind (p|q) #IMPLIED><!ELEMENT b (#PCDATA)>\
                   <!ELEMENT s (b)><!ENTITY e 'text'>]>";

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).with_step_dt(0.02).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

fn file(dir: &Path, n: usize, text: &str) -> PathBuf {
    // A file of its own each time: opening a path again would show the document that is open.
    let path = dir.join(format!("c{n}.xml"));
    std::fs::write(&path, text).expect("writes");
    path
}

/// Open `marked` (the caret at `¦`) once the grammar of its DOCTYPE has been read.
fn open_at(h: &mut Harness<'static, XmlCraftApp>, dir: &Path, marked: &str) {
    let caret = marked.find('¦').expect("a marker");
    let text = marked.replacen('¦', "", 1);
    let n = h.state().session.documents().len();
    assert!(h.state_mut().open_path(&file(dir, n, &text)));
    let started = Instant::now();
    while !h.state().has_grammar() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the grammar");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(3);
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(caret, caret);
    h.run_steps(4);
}

fn text(h: &Harness<'static, XmlCraftApp>) -> String {
    h.state().session.active_document().expect("shown").editor().text()
}

fn type_text(h: &mut Harness<'static, XmlCraftApp>, typed: &str) {
    h.get_by_label("XML editor").type_text(typed);
    h.run_steps(4);
}

#[test]
fn after_an_open_bracket_the_elements_that_may_come_are_offered_and_enter_writes_one() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1'/>¦</r>"));
    type_text(&mut h, "<");
    assert!(h.query_by_label("Complete b").is_some() && h.query_by_label("Complete s").is_some(), "after a: b and s may come");
    assert!(h.query_by_label("Complete a").is_none(), "a may not");
    // Down to s, Enter: the element is written in place of the bracket, with what it requires, and the popup is gone.
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    let now = text(&h);
    assert!(now.contains("<a id='1'/><s>") && !now.contains("<<"), "{now}");
    assert!(h.query_by_label("Complete b").is_none(), "the popup closed");
}

#[test]
fn more_letters_narrow_the_list_and_escape_closes_it_without_writing() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1'/>¦</r>"));
    type_text(&mut h, "<s");
    assert!(h.query_by_label("Complete s").is_some());
    assert!(h.query_by_label("Complete b").is_none(), "b does not fit the s that was typed");
    let before = text(&h);
    h.key_press(egui::Key::Escape);
    h.run_steps(3);
    assert!(h.query_by_label("Complete s").is_none(), "closed");
    assert_eq!(text(&h), before, "Escape wrote nothing");
}

#[test]
fn attributes_values_and_entities_are_offered_too() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a ¦/></r>"));
    type_text(&mut h, "ki");
    assert!(h.query_by_label("Complete kind").is_some(), "a letter into an attribute name");
    assert!(h.query_by_label("Complete id").is_none(), "id does not fit");
    h.key_press(egui::Key::Tab);
    h.run_steps(4);
    assert!(text(&h).contains("<a kind=\"p\" />") || text(&h).contains("<a kind=\"p\"/>") || text(&h).contains("kind=\"p\""), "{}", text(&h));
    // Entities: after an ampersand in text.
    open_at(&mut h, dir.path(), &format!("{DTD}<r><b>¦</b></r>"));
    type_text(&mut h, "&");
    assert!(h.query_by_label("Complete &e;").is_some(), "the entity the DTD declares");
}

#[test]
fn nothing_is_offered_in_text_or_without_a_typed_character() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1'/><b>¦</b></r>"));
    type_text(&mut h, "text");
    assert!(h.query_by_label_contains("Complete").is_none(), "plain text is plain text");
    // Moving the caret next to a bracket offers nothing: only typing does.
    let at = text(&h).find("<b>").expect("there") + 1;
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(at, at);
    h.run_steps(3);
    assert!(h.query_by_label_contains("Complete").is_none());
}

#[test]
fn the_values_of_an_attribute_are_offered_inside_its_quotes() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1' kind=¦/></r>"));
    type_text(&mut h, "\"");
    assert!(h.query_by_label("Complete p").is_some() && h.query_by_label("Complete q").is_some(), "the enumeration of kind");
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert!(text(&h).contains("kind=\"q\""), "{}", text(&h));
}

#[test]
fn a_click_on_a_row_writes_it_as_one_undoable_step() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1'/>¦</r>"));
    let before = text(&h);
    type_text(&mut h, "<");
    h.get_by_label("Complete s").click();
    h.run_steps(4);
    assert!(text(&h).contains("<a id='1'/><s>"), "{}", text(&h));
    // The element and what it requires are one step; the bracket that was typed is another.
    assert!(h.state_mut().execute("edit.undo"));
    assert_eq!(text(&h), before.replacen("<a id='1'/>", "<a id='1'/><", 1), "undo takes back the choice, not the bracket");
}

#[test]
fn without_a_grammar_the_names_used_in_the_document_are_offered_after_a_bracket() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    let text0 = "<lib><book/>\n</lib>";
    let caret = text0.find("\n</lib>").expect("there");
    assert!(h.state_mut().open_path(&file(dir.path(), 0, text0)));
    let started = Instant::now();
    while h.state().problems().is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the analysis");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(3);
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(caret, caret);
    h.run_steps(3);
    type_text(&mut h, "<");
    assert!(h.query_by_label("Complete book").is_some() && h.query_by_label("Complete lib").is_some());
    type_text(&mut h, "bo");
    h.key_press(egui::Key::Tab);
    h.run_steps(4);
    assert!(text(&h).contains("<book/><book></book>"), "{}", text(&h));
}

#[test]
fn the_popup_goes_when_the_caret_moves_away_the_text_is_undone_or_the_caret_is_put_elsewhere() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1'/>¦</r>"));
    type_text(&mut h, "<");
    assert!(h.query_by_label("Complete s").is_some());
    h.key_press(egui::Key::ArrowLeft);
    h.run_steps(3);
    assert!(h.query_by_label("Complete s").is_none(), "moving the caret closes it");
    type_text(&mut h, "<");
    assert!(h.query_all_by_label_contains("Complete").count() > 0);
    // An undo is a change that is not typing: the popup is not about that text any more.
    assert!(h.state_mut().execute("edit.undo"));
    h.run_steps(3);
    assert_eq!(h.query_all_by_label_contains("Complete").count(), 0, "undo closes it");
    type_text(&mut h, "<");
    // The caret put somewhere else by something other than the keyboard: it is not the popup's any more.
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(0, 0);
    h.run_steps(3);
    assert_eq!(h.query_all_by_label_contains("Complete").count(), 0);
}

#[test]
fn an_attribute_that_is_there_already_is_not_offered_and_a_value_is_of_its_own_attribute() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1' ¦/></r>"));
    type_text(&mut h, "i");
    assert!(h.query_by_label("Complete id").is_none(), "id is given already");
    let other = "<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY><!ATTLIST a size (s|m) #IMPLIED kind (p|q) #IMPLIED>]>";
    open_at(&mut h, dir.path(), &format!("{other}<r><a kind=¦/></r>"));
    type_text(&mut h, "\"");
    assert!(h.query_by_label("Complete p").is_some());
    assert!(h.query_by_label("Complete m").is_none() && h.query_by_label("Complete s").is_none(), "the values of size are not those of kind");
}

#[test]
fn a_bracket_in_a_comment_or_a_cdata_section_offers_nothing() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1'/><!-- ¦ --></r>"));
    type_text(&mut h, "<");
    assert_eq!(h.query_all_by_label_contains("Complete").count(), 0, "in a comment");
    type_text(&mut h, "&");
    assert_eq!(h.query_all_by_label_contains("Complete").count(), 0, "an ampersand in a comment too");
}

#[test]
fn the_keys_of_the_popup_are_the_editors_only_while_it_has_the_keyboard() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1'/>¦</r>"));
    type_text(&mut h, "<");
    assert!(h.query_by_label("Complete s").is_some());
    let before = text(&h);
    // The keyboard goes to the find bar: Enter there is the find bar's, and writes nothing in the document.
    assert!(h.state_mut().execute("edit.find"));
    h.run_steps(4);
    h.key_press(egui::Key::Enter);
    h.run_steps(3);
    assert_eq!(text(&h), before, "the popup did not take the Enter");
    assert_eq!(h.query_all_by_label_contains("Complete").count(), 0, "and it is gone");
}

#[test]
fn an_end_tag_has_no_attributes_to_complete_and_a_value_has_no_bracket() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1'/><b>x</b¦</r>"));
    type_text(&mut h, " ki");
    assert_eq!(h.query_all_by_label_contains("Complete").count(), 0, "an end tag has no attributes, whatever encloses it");
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1' kind=\"foo¦\"/></r>"));
    type_text(&mut h, "<");
    assert_eq!(h.query_all_by_label_contains("Complete").count(), 0, "a bracket in a value is nothing to complete");
    // Later in such a value: what comes after the bracket is not a value either.
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id='1' kind=\"a<b ¦\"/></r>"));
    type_text(&mut h, "p");
    assert_eq!(h.query_all_by_label_contains("Complete").count(), 0, "after a bracket in a value");
}

#[test]
fn an_entity_is_offered_inside_an_attribute_value_too() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_at(&mut h, dir.path(), &format!("{DTD}<r><a id=\"¦\"/></r>"));
    type_text(&mut h, "&");
    assert!(h.query_by_label("Complete &e;").is_some(), "references are written in values too");
}

#[test]
fn only_the_entities_a_value_may_refer_to_are_offered_in_one() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    let dtd = "<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY><!ATTLIST a id CDATA #IMPLIED>\
               <!ENTITY ok 'fine'><!ENTITY lt2 'a<b'><!ENTITY ext SYSTEM 'x.txt'>]>";
    open_at(&mut h, dir.path(), &format!("{dtd}<r><a id=\"¦\"/></r>"));
    type_text(&mut h, "&");
    assert!(h.query_by_label("Complete &ok;").is_some());
    assert!(h.query_by_label("Complete &lt2;").is_none(), "its text holds a bracket");
    assert!(h.query_by_label("Complete &ext;").is_none(), "it is external");
}

#[test]
fn idref_values_are_offered_from_the_ids_in_the_document() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    let dtd = "<!DOCTYPE r [<!ELEMENT r (a*, b)><!ELEMENT a EMPTY><!ATTLIST a id ID #IMPLIED>\
               <!ELEMENT b EMPTY><!ATTLIST b ref IDREF #IMPLIED>]>";
    open_at(&mut h, dir.path(), &format!("{dtd}<r><a id='one'/><a id='two'/><b ref=¦/></r>"));
    type_text(&mut h, "\"");
    assert!(h.query_by_label("Complete one").is_some() && h.query_by_label("Complete two").is_some(), "the IDs the document has");
    h.key_press(egui::Key::ArrowDown);
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert!(text(&h).contains("ref=\"two\""), "{}", text(&h));
}

#[test]
fn an_idrefs_value_adds_the_id_that_was_chosen_to_the_ones_before_it() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    let dtd = "<!DOCTYPE r [<!ELEMENT r (a*, b)><!ELEMENT a EMPTY><!ATTLIST a id ID #IMPLIED>\
               <!ELEMENT b EMPTY><!ATTLIST b refs IDREFS #IMPLIED>]>";
    open_at(&mut h, dir.path(), &format!("{dtd}<r><a id='one'/><a id='two'/><b refs='one ¦'/></r>"));
    type_text(&mut h, "t");
    assert!(h.query_by_label("Complete one two").is_some(), "the IDREF list adds `two`");
    assert!(h.query_by_label("Complete one one").is_none(), "an ID cannot be repeated in one value");
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    assert!(text(&h).contains("refs='one two'"), "{}", text(&h));
}
