//! The document lifecycle in the real shell: open, edit, save, save as, close, quit and outside changes, with files on disk.

use std::path::{Path, PathBuf};

use egui::{Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_ui_egui::XmlCraftApp;

fn shell() -> Harness<'static, XmlCraftApp> {
    // These tests open text in other scripts than the bundled fonts cover; the app draws a box for them, as the test harness is told to.
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).allow_missing_glyphs().build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

fn file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("writes the fixture");
    path
}

fn open(h: &mut Harness<'static, XmlCraftApp>, path: &Path) {
    assert!(h.state_mut().open_path(path), "opens {}", path.display());
    h.run_steps(4);
}

fn text(h: &Harness<'static, XmlCraftApp>) -> String {
    h.state().session.active_document().expect("a document is shown").editor().text()
}

fn type_text(h: &mut Harness<'static, XmlCraftApp>, text: &str) {
    h.get_by_label("XML editor").type_text(text);
    h.run_steps(3);
}

#[test]
fn opening_a_file_shows_its_tab_text_and_status() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "invoice.xml", b"<invoice>\r\n  <total/>\r\n</invoice>\r\n");
    let mut h = shell();
    open(&mut h, &path);
    assert_eq!(h.state().tabs().iter().map(|t| (t.name.as_str(), t.dirty)).collect::<Vec<_>>(), [("invoice.xml", false)]);
    assert!(h.query_by_label("XML editor").is_some() && h.query_by_label("XmlCraft").is_none());
    assert_eq!(text(&h), "<invoice>\r\n  <total/>\r\n</invoice>\r\n");
    for part in ["Ln 1, Col 1", "UTF-8", "CRLF", "Spaces: 2"] {
        assert!(h.query_by_label(part).is_some(), "the status bar says {part}");
    }
    // Opening it again shows the open one.
    open(&mut h, &path);
    assert_eq!(h.state().tabs().len(), 1);
}

#[test]
fn a_file_that_cannot_be_opened_says_why() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    assert!(!h.state_mut().open_path(&dir.path().join("missing.xml")));
    h.run_steps(3);
    assert!(h.query_by_label("Couldn't open the file").is_some());
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(h.query_by_label("Couldn't open the file").is_none());
    assert!(h.state().tabs().is_empty());
}

#[test]
fn typing_and_saving_keep_the_encoding_the_byte_order_mark_and_the_line_endings() {
    let dir = tempfile::tempdir().expect("dir");
    let original = b"\xEF\xBB\xBF<a>\r\n  <b/>\r\n</a>\r\n";
    let path = file(dir.path(), "a.xml", original);
    let mut h = shell();
    open(&mut h, &path);
    // An untouched file is written back identically.
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run_steps(3);
    assert_eq!(std::fs::read(&path).expect("reads"), original);
    // Type at the start of the second line.
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(5, 5);
    type_text(&mut h, "x");
    assert!(h.state().tabs()[0].dirty);
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run_steps(3);
    assert!(!h.state().tabs()[0].dirty);
    assert_eq!(std::fs::read(&path).expect("reads"), b"\xEF\xBB\xBF<a>\r\nx  <b/>\r\n</a>\r\n");
    // A new line uses the file's line ending.
    h.key_press(Key::Enter);
    h.run_steps(3);
    assert_eq!(text(&h), "<a>\r\nx\r\n  <b/>\r\n</a>\r\n");
}

#[test]
fn a_latin_1_file_stays_latin_1() {
    let dir = tempfile::tempdir().expect("dir");
    let original = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a>caf\xE9</a>";
    let path = file(dir.path(), "latin.xml", original);
    let mut h = shell();
    open(&mut h, &path);
    assert!(text(&h).contains("café"));
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run_steps(3);
    assert_eq!(std::fs::read(&path).expect("reads"), original);
    assert!(h.query_by_label("windows-1252").is_some(), "the status bar names the encoding it reads it as");
}

#[test]
fn a_character_the_encoding_cannot_hold_asks_what_to_do() {
    let dir = tempfile::tempdir().expect("dir");
    let original = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a/>";
    let path = file(dir.path(), "latin.xml", original);
    let mut h = shell();
    open(&mut h, &path);
    let end = text(&h).len();
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(end - 4, end - 4);
    type_text(&mut h, "日");
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run_steps(3);
    assert!(h.query_by_label("latin.xml has a character its encoding cannot hold").is_some());
    assert_eq!(std::fs::read(&path).expect("reads"), original, "nothing was written");
    h.get_by_label("Write character references").click();
    h.run_steps(3);
    let written = std::fs::read(&path).expect("reads");
    assert!(written.windows(8).any(|w| w == b"&#x65E5;"), "{}", String::from_utf8_lossy(&written));
    assert!(!h.state().tabs()[0].dirty);
}

#[test]
fn saving_as_utf_8_changes_the_encoding_when_a_character_does_not_fit() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "latin.xml", b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a/>");
    let mut h = shell();
    open(&mut h, &path);
    let end = text(&h).len();
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(end - 4, end - 4);
    type_text(&mut h, "日");
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run_steps(3);
    h.get_by_label("Save as UTF-8").click();
    h.run_steps(3);
    let written = std::fs::read(&path).expect("reads");
    assert!(String::from_utf8_lossy(&written).contains('日'));
    assert_eq!(h.state().session.active_document().expect("shown").encoding().encoding.name(), "UTF-8");
}

#[test]
fn a_new_document_is_saved_where_the_picker_says() {
    let dir = tempfile::tempdir().expect("dir");
    let target = dir.path().join("fresh.xml");
    let mut h = shell();
    h.state_mut().execute("file.new");
    h.run_steps(3);
    type_text(&mut h, "<fresh/>");
    h.state_mut().pick_override = Some(vec![target.clone()]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run_steps(4);
    assert_eq!(std::fs::read(&target).expect("reads"), b"<fresh/>");
    assert_eq!(h.state().tabs().iter().map(|t| (t.name.as_str(), t.dirty)).collect::<Vec<_>>(), [("fresh.xml", false)]);
    // Cancelling the picker leaves the document as it was.
    h.state_mut().execute("file.new");
    h.run_steps(3);
    type_text(&mut h, "x");
    h.state_mut().pick_override = Some(Vec::new());
    h.state_mut().execute("file.save");
    h.run_steps(4);
    assert!(h.state().tabs()[1].dirty);
}

#[test]
fn closing_a_document_with_changes_asks_first() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "a.xml", b"<a/>");
    let mut h = shell();
    open(&mut h, &path);
    type_text(&mut h, "x");
    h.state_mut().execute("file.close");
    h.run_steps(3);
    assert!(h.query_by_label("Save changes to a.xml?").is_some());
    // Cancel keeps it.
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().tabs().len(), 1);
    // Don't Save closes without writing.
    h.state_mut().execute("file.close");
    h.run_steps(3);
    h.get_by_label("Don't Save").click();
    h.run_steps(3);
    assert!(h.state().tabs().is_empty());
    assert_eq!(std::fs::read(&path).expect("reads"), b"<a/>");
    // Save writes, then closes.
    open(&mut h, &path);
    type_text(&mut h, "y");
    h.state_mut().execute("file.close");
    h.run_steps(3);
    h.get_by_label("Save").click();
    h.run_steps(3);
    assert!(h.state().tabs().is_empty());
    assert_eq!(std::fs::read(&path).expect("reads"), b"y<a/>");
}

#[test]
fn quitting_goes_through_every_document_with_changes() {
    let dir = tempfile::tempdir().expect("dir");
    let (a, b) = (file(dir.path(), "a.xml", b"<a/>"), file(dir.path(), "b.xml", b"<b/>"));
    let mut h = shell();
    open(&mut h, &a);
    type_text(&mut h, "1");
    open(&mut h, &b);
    type_text(&mut h, "2");
    h.state_mut().execute("file.quit");
    h.run_steps(3);
    // The first document with changes is asked about first.
    assert!(h.query_by_label("Save changes to a.xml?").is_some());
    h.get_by_label("Save").click();
    h.run_steps(3);
    assert_eq!(std::fs::read(&a).expect("reads"), b"1<a/>");
    assert!(h.query_by_label("Save changes to b.xml?").is_some());
    // Cancelling anywhere stops the quit and keeps what is left.
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().tabs().iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["b.xml"]);
    assert!(h.query_by_label("Save changes to b.xml?").is_none());
    assert_eq!(std::fs::read(&b).expect("reads"), b"<b/>");
}

#[test]
fn a_file_changed_elsewhere_is_reloaded_when_clean_and_asked_about_when_edited() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "a.xml", b"<a/>");
    let mut h = shell();
    open(&mut h, &path);
    std::fs::write(&path, b"<changed-elsewhere/>").expect("writes");
    h.state_mut().check_disk_changes();
    h.run_steps(3);
    assert_eq!(text(&h), "<changed-elsewhere/>", "no edits: reloaded quietly");
    // With edits it asks, and Keep leaves the text alone.
    type_text(&mut h, "x");
    std::fs::write(&path, b"<changed-again-longer/>").expect("writes");
    h.state_mut().check_disk_changes();
    h.run_steps(3);
    assert!(h.query_by_label("a.xml changed on disk").is_some());
    h.get_by_label("Keep my version").click();
    h.run_steps(3);
    assert_eq!(text(&h), "x<changed-elsewhere/>");
    // Saving over a file that changed since asks before it replaces it.
    std::fs::write(&path, b"<a-third-change/>").expect("writes");
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run_steps(3);
    assert!(h.query_by_label("a.xml changed on disk").is_some());
    h.get_by_label("Overwrite").click();
    h.run_steps(3);
    assert_eq!(std::fs::read(&path).expect("reads"), b"x<changed-elsewhere/>");
}

#[test]
fn a_file_with_invalid_bytes_opens_but_will_not_be_saved_over() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "bad.xml", b"<a>\xFF</a>");
    let mut h = shell();
    open(&mut h, &path);
    assert!(h.state().session.active_document().expect("shown").is_lossy());
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run_steps(3);
    assert!(h.query_by_label("bad.xml cannot be saved over").is_some());
    assert_eq!(std::fs::read(&path).expect("reads"), b"<a>\xFF</a>", "the original bytes are untouched");
    // Save As a copy works.
    let copy = dir.path().join("copy.xml");
    h.state_mut().pick_override = Some(vec![copy.clone()]);
    h.get_by_label("Save As…").click();
    h.run_steps(4);
    assert!(String::from_utf8_lossy(&std::fs::read(&copy).expect("reads")).contains("<a>"));
    assert_eq!(std::fs::read(&path).expect("reads"), b"<a>\xFF</a>");
}

#[test]
fn reverting_throws_the_changes_away_after_asking() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "a.xml", b"<a/>");
    let mut h = shell();
    open(&mut h, &path);
    type_text(&mut h, "x");
    h.state_mut().execute("file.revert");
    h.run_steps(3);
    assert!(h.query_by_label("Revert a.xml?").is_some());
    h.get_by_label("Revert").click();
    h.run_steps(3);
    assert_eq!(text(&h), "<a/>");
    assert!(!h.state().tabs()[0].dirty);
}

#[test]
fn undo_and_redo_through_the_menu_commands() {
    let mut h = shell();
    h.state_mut().execute("file.new");
    h.run_steps(3);
    type_text(&mut h, "abc");
    assert!(h.state_mut().execute("edit.undo"));
    h.run_steps(2);
    assert_eq!(text(&h), "");
    assert!(h.state_mut().execute("edit.redo"));
    h.run_steps(2);
    assert_eq!(text(&h), "abc");
    assert!(h.state_mut().execute("edit.select_all"));
    h.run_steps(2);
    assert_eq!(h.state().session.active_document().expect("shown").editor().primary().sel.end(), 3);
}

#[test]
fn dismissing_the_reload_question_does_not_accept_the_other_programs_change() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "a.xml", b"<a/>");
    let mut h = shell();
    open(&mut h, &path);
    type_text(&mut h, "x");
    std::fs::write(&path, b"<changed-elsewhere/>").expect("writes");
    h.state_mut().check_disk_changes();
    h.run_steps(3);
    assert!(h.query_by_label("a.xml changed on disk").is_some());
    // Escape is not a decision: the text stays, and the next save still asks before replacing what the other program wrote.
    h.key_press(Key::Escape);
    h.run_steps(3);
    assert!(h.query_by_label("a.xml changed on disk").is_none(), "the question is closed");
    assert_eq!(text(&h), "x<a/>");
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run_steps(3);
    assert!(h.query_by_label("a.xml changed on disk").is_some(), "saving asks again");
    assert_eq!(std::fs::read(&path).expect("reads"), b"<changed-elsewhere/>", "nothing was overwritten");
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(std::fs::read(&path).expect("reads"), b"<changed-elsewhere/>");
}

#[test]
fn save_as_onto_the_documents_own_file_still_checks_for_outside_changes() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "a.xml", b"<a/>");
    let mut h = shell();
    open(&mut h, &path);
    type_text(&mut h, "x");
    std::fs::write(&path, b"<changed-elsewhere/>").expect("writes");
    let own = h.state().session.active_document().and_then(|d| d.path()).map(Path::to_path_buf).expect("has a path");
    h.state_mut().pick_override = Some(vec![own]);
    h.state_mut().execute("file.save_as");
    h.run_steps(4);
    assert!(h.query_by_label("a.xml changed on disk").is_some(), "asked, not overwritten");
    assert_eq!(std::fs::read(&path).expect("reads"), b"<changed-elsewhere/>");
}

#[test]
fn closing_a_file_with_invalid_bytes_through_save_as_still_closes_it() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "bad.xml", b"<a>\xFF</a>");
    let mut h = shell();
    open(&mut h, &path);
    type_text(&mut h, "x");
    h.state_mut().execute("file.close");
    h.run_steps(3);
    h.get_by_label("Save").click();
    h.run_steps(3);
    assert!(h.query_by_label("bad.xml cannot be saved over").is_some());
    let copy = dir.path().join("copy.xml");
    h.state_mut().pick_override = Some(vec![copy.clone()]);
    h.get_by_label("Save As…").click();
    h.run_steps(4);
    assert!(copy.exists(), "the copy was written");
    assert!(h.state().tabs().is_empty(), "and the close that asked for the save went on");
    assert_eq!(std::fs::read(&path).expect("reads"), b"<a>\xFF</a>");
}

#[test]
fn quitting_while_another_question_is_open_goes_on_once_it_is_answered() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "a.xml", b"<a/>");
    let mut h = shell();
    open(&mut h, &path);
    type_text(&mut h, "x");
    assert!(!h.state_mut().open_path(&dir.path().join("missing.xml")));
    h.run_steps(3);
    assert!(h.query_by_label("Couldn't open the file").is_some());
    h.state_mut().execute("file.quit");
    h.run_steps(3);
    h.get_by_label("OK").click();
    h.run_steps(3);
    assert!(h.query_by_label("Save changes to a.xml?").is_some(), "the quit was not forgotten");
}

#[test]
fn several_files_that_cannot_be_opened_are_reported_together() {
    let dir = tempfile::tempdir().expect("dir");
    let good = file(dir.path(), "good.xml", b"<a/>");
    let (one, two) = (dir.path().join("one.xml"), dir.path().join("two.xml"));
    let mut h = shell();
    h.state_mut().open_paths([one.as_path(), good.as_path(), two.as_path()]);
    h.run_steps(3);
    assert_eq!(h.state().tabs().len(), 1, "the good one opened");
    assert!(h.query_by_label("Couldn't open some of the files").is_some());
    for name in ["one.xml", "two.xml"] {
        assert!(h.query_all_by_label_contains(name).next().is_some(), "{name} is named in the message");
    }
}

#[test]
fn the_status_bar_keeps_what_a_command_says() {
    let dir = tempfile::tempdir().expect("dir");
    let path = file(dir.path(), "a.xml", b"<a/>");
    let mut h = shell();
    open(&mut h, &path);
    type_text(&mut h, "x");
    assert!(h.state_mut().execute("file.save"));
    assert_eq!(h.state().status, "Saved a.xml");
}

#[test]
fn a_character_reference_is_not_written_into_a_comment() {
    let dir = tempfile::tempdir().expect("dir");
    let original = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><a><!----></a>";
    let path = file(dir.path(), "latin.xml", original);
    let mut h = shell();
    open(&mut h, &path);
    let at = text(&h).find("-->").expect("the comment");
    h.state_mut().session.active_document_mut().expect("shown").editor_mut().set_selection(at, at);
    type_text(&mut h, "日");
    h.key_press_modifiers(Modifiers::COMMAND, Key::S);
    h.run_steps(3);
    h.get_by_label("Write character references").click();
    h.run_steps(3);
    assert!(h.query_by_label("A character reference won't do here").is_some());
    assert_eq!(std::fs::read(&path).expect("reads"), original, "nothing was written");
    assert!(text(&h).contains("<!--日-->"), "and the text was not touched: {}", text(&h));
}

#[test]
fn pretty_print_and_minify_are_commands_that_undo_in_one_step() {
    let mut h = shell();
    h.state_mut().execute("file.new");
    h.run_steps(3);
    type_text(&mut h, "<a><b><c/></b><d/></a>");
    assert!(h.state_mut().execute("xml.pretty_print"));
    h.run_steps(2);
    assert_eq!(text(&h), "<a>\n  <b>\n    <c/>\n  </b>\n  <d/>\n</a>");
    assert_eq!(h.state().status, "Pretty-printed Untitled-1");
    assert!(h.state_mut().execute("xml.minify"));
    h.run_steps(2);
    assert_eq!(text(&h), "<a><b><c/></b><d/></a>");
    assert!(h.state_mut().execute("xml.minify"));
    assert_eq!(h.state().status, "Untitled-1 needs no changes");
    assert!(h.state_mut().execute("edit.undo"));
    h.run_steps(2);
    assert_eq!(text(&h), "<a>\n  <b>\n    <c/>\n  </b>\n  <d/>\n</a>");
}

#[test]
fn a_document_that_is_not_well_formed_is_not_reformatted() {
    let mut h = shell();
    h.state_mut().execute("file.new");
    h.run_steps(3);
    type_text(&mut h, "<a><b></a>");
    h.state_mut().execute("xml.pretty_print");
    h.run_steps(3);
    assert!(h.query_by_label("The document is not well-formed").is_some());
    assert_eq!(text(&h), "<a><b></a>", "left as it was");
}

#[test]
fn the_home_view_lists_the_files_opened_lately_and_a_click_opens_one() {
    let dir = tempfile::tempdir().expect("dir");
    let (a, b) = (file(dir.path(), "alpha.xml", b"<alpha/>"), file(dir.path(), "beta.xml", b"<beta/>"));
    let mut h = shell();
    assert!(h.query_by_label("Recent").is_none(), "nothing has been opened yet");
    open(&mut h, &a);
    open(&mut h, &b);
    // Close both: the home view is shown, with the newest first.
    h.state_mut().execute("file.close");
    h.state_mut().execute("file.close");
    h.run_steps(4);
    assert!(h.state().tabs().is_empty());
    assert!(h.query_by_label("Recent").is_some());
    let names: Vec<String> =
        h.state().recent_files().iter().map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).collect();
    assert_eq!(names, ["beta.xml", "alpha.xml"]);
    h.get_by_label_contains("alpha.xml").click();
    h.run_steps(4);
    assert_eq!(h.state().tabs().iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["alpha.xml"]);
    // A file that is gone is reported, and dropped from the list.
    h.state_mut().execute("file.close");
    h.run_steps(3);
    std::fs::remove_file(&b).expect("removes");
    h.get_by_label_contains("beta.xml").click();
    h.run_steps(4);
    assert!(h.query_by_label("Couldn't open the file").is_some(), "the reason is shown");
    assert!(h.state().tabs().is_empty(), "it could not be opened");
    assert!(!h.state().recent_files().iter().any(|p| p.ends_with("beta.xml")), "and it is not listed any more");
    // The command empties the list.
    h.state_mut().execute("file.clear_recent");
    h.run_steps(3);
    assert!(h.state().recent_files().is_empty());
    assert!(h.query_by_label("Recent").is_none());
}
