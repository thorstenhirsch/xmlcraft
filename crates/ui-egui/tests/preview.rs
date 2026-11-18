//! The Preview tool window in the real shell: the rail and the command bar for the documents a browser shows, the page worked out for
//! an HTML document, an XML document through the stylesheet it names and plain XML, the permissions a person gives one document, and
//! the system-browser copy. A test window has no native handle, so the browser view itself is never made here (on macOS and Windows
//! the attempt fails and the tool falls back, as it does on Linux); its decisions are tested in `preview_host.rs`. Asynchronous work is
//! waited for by polling the state, never by counting frames.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_engine::preview::Shown;
use xmlcraft_ui_egui::{Panel, XmlCraftApp};

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1440.0, 900.0)).with_step_dt(0.05).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
}

fn file(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path
}

fn wait_for(h: &mut Harness<'static, XmlCraftApp>, what: &str, mut done: impl FnMut(&mut Harness<'static, XmlCraftApp>) -> bool) {
    let started = Instant::now();
    while !done(h) {
        assert!(started.elapsed() < Duration::from_secs(30), "waiting for {what}");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
}

fn open(h: &mut Harness<'static, XmlCraftApp>, path: &Path) {
    assert!(h.state_mut().open_path(path));
    wait_for(h, "the first analysis", |h| h.state().problems().is_some());
    // (The command bar looks at the text once it has stood still.)
    h.run_steps(20);
}

/// Wait until the Preview shows the page as `shown`, and return its title and warnings.
fn page(h: &mut Harness<'static, XmlCraftApp>, shown: Shown) -> (Option<String>, Vec<String>) {
    wait_for(h, "the page", |h| h.state().preview_page().is_some_and(|p| p.is_ok_and(|p| p.shown == shown)));
    match h.state().preview_page() {
        Some(Ok(page)) => (page.title.clone(), page.warnings.clone()),
        other => panic!("{other:?}"),
    }
}

const XSL: &str = r#"<xsl:stylesheet version="3.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:output method="html"/><xsl:template match="/"><html><head><title>Books</title></head><body><xsl:for-each select="//book"><p><xsl:value-of select="."/></p></xsl:for-each></body></html></xsl:template></xsl:stylesheet>"#;

#[test]
fn a_page_lights_the_rail_icon_and_offers_preview_and_plain_xml_does_not() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "plain.xml", "<library><book/></library>"));
    assert!(!h.state_mut().panel_relevant(Panel::Preview), "plain XML: the rail icon is dimmed");
    assert!(h.query_by_label("Preview").is_none(), "no Preview action for plain XML");
    open(&mut h, &file(dir.path(), "styled.xml", "<?xml-stylesheet type=\"text/xsl\" href=\"s.xsl\"?>\n<books/>"));
    assert!(h.state_mut().panel_relevant(Panel::Preview), "XML that names a stylesheet: lit");
    assert!(h.query_by_label("Preview").is_some(), "and the command bar offers Preview");
    open(&mut h, &file(dir.path(), "image.svg", "<svg xmlns=\"http://www.w3.org/2000/svg\"/>"));
    assert!(h.state_mut().panel_relevant(Panel::Preview), "an SVG image: lit");
    open(&mut h, &file(dir.path(), "index.html", "<!DOCTYPE html><title>Home</title><p>Hello"));
    assert!(h.state_mut().panel_relevant(Panel::Preview), "a page: lit");
    assert!(!h.state().panel_visible(Panel::Preview), "hidden by default");
    h.get_by_label("Preview").click();
    h.run_steps(3);
    assert!(h.state().panel_visible(Panel::Preview) && h.query_by_label("Close Preview").is_some(), "the tool window is up");
    assert!(h.query_by_label("→ index.html").is_some(), "it says which document it shows");
    let (title, warnings) = page(&mut h, Shown::Html);
    assert_eq!((title.as_deref(), warnings.len()), (Some("Home"), 0));
    assert!(!h.state().preview_view_shown(), "a test window has no browser view");
    assert!(h.query_by_label("The page opens in your browser").is_some(), "the tool falls back to the system browser and says so");
}

#[test]
fn open_in_browser_writes_the_page_with_the_policy_follows_it_and_removes_it_at_the_end() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "index.html", "<!DOCTYPE html><title>One</title><p>Hello"));
    assert!(h.state_mut().execute("xml.preview"));
    page(&mut h, Shown::Html);
    h.get_by_label("Open in Browser").click();
    h.run_steps(2);
    let (url, copy) = h.state().preview_browser_copy();
    let copy = copy.expect("a copy was written");
    assert!(url.as_deref().is_some_and(|u| u.starts_with("file://") && u.ends_with("index-0.html") || u.contains("index-")), "{url:?}");
    let written = std::fs::read_to_string(&copy).expect("reads");
    assert!(written.starts_with("<!DOCTYPE html><meta http-equiv=\"Content-Security-Policy\" content=\"default-src &#39;none&#39;"), "{written}");
    assert!(written.contains("<title>One</title>"), "the page itself follows unchanged");
    // The text changes: the copy follows (the browser shows it when reloaded).
    if let Some(doc) = h.state_mut().session.active_document_mut() {
        doc.editor_mut().select_all();
        doc.editor_mut().paste("<!DOCTYPE html><title>Two</title><p>Bye");
    }
    page(&mut h, Shown::Html);
    wait_for(&mut h, "the new title", |h| h.state().preview_page().is_some_and(|p| p.is_ok_and(|p| p.title.as_deref() == Some("Two"))));
    assert!(std::fs::read_to_string(&copy).expect("reads").contains("<title>Two</title>"));
    // Scripts allowed for this document: the copy's policy says so.
    h.get_by_label("Scripts of the page").click();
    h.run_steps(2);
    assert!(std::fs::read_to_string(&copy).expect("reads").contains("script-src &#39;unsafe-inline&#39;"), "the copy is written again with it");
    let folder = copy.parent().expect("its folder").to_path_buf();
    h.state_mut().close_preview();
    assert!(!copy.exists() && !folder.exists(), "the copy and its folder are gone when the app closes");
}

#[test]
fn an_xml_document_is_shown_through_the_stylesheet_it_names_and_follows_the_stylesheet_being_edited() {
    let dir = tempfile::tempdir().expect("dir");
    file(dir.path(), "b.xsl", XSL);
    let doc = file(dir.path(), "books.xml", "<?xml-stylesheet type=\"text/xsl\" href=\"b.xsl\"?>\n<books><book>One</book></books>");
    let mut h = shell();
    open(&mut h, &doc);
    assert!(h.state_mut().execute("xml.preview"));
    let (title, _) = page(&mut h, Shown::Transformed);
    assert_eq!(title.as_deref(), Some("Books"));
    // The stylesheet opened and changed in the editor: the page of the document follows what is being edited, not the file.
    assert!(h.state_mut().open_path(&dir.path().join("b.xsl")));
    h.run_steps(4);
    if let Some(sheet) = h.state_mut().session.active_document_mut() {
        sheet.editor_mut().select_all();
        sheet.editor_mut().paste(&XSL.replace("<title>Books</title>", "<title>Library</title>"));
    }
    h.state_mut().session.set_active(Some(0));
    wait_for(&mut h, "the page of the edited stylesheet", |h| {
        h.state().preview_page().is_some_and(|p| p.is_ok_and(|p| p.shown == Shown::Transformed && p.title.as_deref() == Some("Library")))
    });
    // Open as Document: the page's HTML in a new tab, the document itself unchanged.
    h.get_by_label("Open as Document").click();
    h.run_steps(3);
    let shown = h.state().session.active_document().expect("a new tab");
    assert_eq!(shown.name(), "books-preview.html");
    assert!(shown.editor().text().contains("<title>Library</title><") && shown.editor().text().contains("<p>One</p>"));
    assert!(std::fs::read_to_string(&doc).expect("reads").starts_with("<?xml-stylesheet"), "the document is not changed");
}

#[test]
fn plain_xml_is_a_tree_a_missing_stylesheet_is_said_and_data_is_not_previewed() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "gone.xml", "<?xml-stylesheet type=\"text/xsl\" href=\"missing.xsl\"?>\n<r><x a=\"1\">t</x></r>"));
    assert!(h.state_mut().execute("xml.preview"));
    let (title, warnings) = page(&mut h, Shown::Tree);
    assert_eq!(title.as_deref(), Some("r"));
    assert!(warnings.first().is_some_and(|w| w.contains("missing.xsl") && w.contains("could not be read")), "{warnings:?}");
    assert!(h.query_all_by_label_contains("could not be read").next().is_some(), "the panel says so");
    open(&mut h, &file(dir.path(), "data.json", "{\"a\": 1}"));
    assert!(h.query_by_label("Preview shows HTML and XML").is_some());
}
