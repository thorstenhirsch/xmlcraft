//! The Profile and Debug modes of the Query tool's XPath tab in the real shell: a profile listed and a row marked, a debugging session
//! stepped to an item, its context node selected in the text, and the session stopped.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use xmlcraft_ui_egui::XmlCraftApp;

const SHOP: &str = "<shop><item><price>20</price><name>a</name></item><item><price>5</price><name>b</name></item><item><price>30</price><name>c</name></item></shop>";

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

fn open_query(h: &mut Harness<'static, XmlCraftApp>, path: &Path) {
    assert!(h.state_mut().open_path(path));
    let started = Instant::now();
    while h.state().problems().is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for the first analysis");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(h.state_mut().execute("xml.xpath"));
    h.run_steps(3);
}

fn type_expression(h: &mut Harness<'static, XmlCraftApp>, expression: &str) {
    h.get_by_label("The XPath expression").focus();
    h.run_steps(1);
    h.get_by_label("The XPath expression").type_text(expression);
    h.run_steps(2);
}

/// Run frames until a label contains `text`.
fn wait_for(h: &mut Harness<'static, XmlCraftApp>, text: &str) {
    let started = Instant::now();
    while h.query_by_label_contains(text).is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for {text:?}");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_profile_lists_the_hit_counts_and_a_click_marks_the_part_in_the_expression() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_query(&mut h, &file(dir.path(), "shop.xml", SHOP));
    h.get_by_label("Profile").click();
    h.run_steps(2);
    type_expression(&mut h, "//item[price > 10]/name");
    h.get_by_label("Profile the expression").click();
    wait_for(&mut h, "price > 10  3 hits");
    assert!(h.query_by_label_contains("name  2 hits").is_some(), "the last step ran for the two items that passed");
    assert!(h.query_by_label_contains("2 items.").is_some(), "with the result");
    // The tree lists every part, the hot spots only those that cost something: both have the predicate.
    h.get_by_label("Tree").click();
    h.run_steps(2);
    assert!(h.query_by_label_contains("//item[price > 10]/name  1 hits").is_some(), "the tree starts with the whole expression");
    h.get_by_label_contains("price > 10  3 hits").hover();
    h.run_steps(2);
    h.get_by_label_contains("price > 10  3 hits").click();
    h.run_steps(3);
    assert!(h.query_by_label("Marked: price > 10").is_some(), "the part is marked in the expression");
    assert_eq!(
        h.get_by_label_contains("price > 10  3 hits").accesskit_node().toggled(),
        Some(egui::accesskit::Toggled::True),
        "the row says it is chosen"
    );
}

#[test]
fn a_debugging_session_steps_shows_the_context_node_and_stops() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_query(&mut h, &file(dir.path(), "shop.xml", SHOP));
    h.get_by_label("Debug").click();
    h.run_steps(2);
    type_expression(&mut h, "//item[price > 10]/name");
    h.get_by_label("Start debugging").click();
    wait_for(&mut h, "Paused, evaluating //item[price > 10]/name");
    assert!(h.query_by_label_contains("item 1 of 1: document").is_some(), "the context is the document");
    // Step into until the predicate takes its first item.
    let started = Instant::now();
    while h.query_by_label_contains("Paused, next item of price > 10").is_none() {
        assert!(started.elapsed() < Duration::from_secs(20), "stepping to the predicate");
        h.get_by_label("Step into (F11)").click();
        let before = Instant::now();
        // (Each step is answered by the worker: wait for the next pause.)
        loop {
            h.run_steps(1);
            std::thread::sleep(Duration::from_millis(5));
            if h.query_by_label_contains("Paused,").is_some() || before.elapsed() > Duration::from_secs(5) {
                break;
            }
        }
    }
    let row = "item 1 of 3: element item  /shop[1]/item[1]";
    wait_for(&mut h, row);
    h.get_by_label_contains(row).hover();
    h.run_steps(2);
    h.get_by_label_contains(row).click();
    h.run_steps(3);
    let doc = h.state().session.active_document().expect("shown");
    let text = doc.editor().text();
    let selection = doc.editor().selections()[0];
    let (start, end) = (selection.start().min(selection.end()), selection.start().max(selection.end()));
    assert_eq!(&text[start..end], "<item><price>20</price><name>a</name></item>", "the context node is selected in the text");
    assert!(h.query_by_label("Close Query").is_some(), "the panel stays open");
    h.get_by_label("Stop").click();
    wait_for(&mut h, "Stopped.");
    assert!(h.query_by_label("Start debugging").is_some(), "a new session can be started");
}

#[test]
fn leaving_the_debug_mode_ends_the_session() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open_query(&mut h, &file(dir.path(), "shop.xml", SHOP));
    h.get_by_label("Debug").click();
    h.run_steps(2);
    type_expression(&mut h, "//item[price > 10]/name");
    h.get_by_label("Start debugging").click();
    wait_for(&mut h, "Paused,");
    assert!(h.query_by_label("Stop debugging").is_some(), "the footer stops a running session");
    h.get_by_label("Result").click();
    h.run_steps(3);
    h.get_by_label("Debug").click();
    h.run_steps(3);
    wait_for(&mut h, "Stopped.");
    assert!(h.query_by_label("Start debugging").is_some());
}
