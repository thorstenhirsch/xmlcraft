//! The XQuery tab of the Query tool window in the real shell, and XQuery documents: a query run on the worker with the document as
//! its context, a node of the result selected in the text, an error with its place, an `.xq` document run from the command bar and
//! its problems while typing.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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

fn wait_for(h: &mut Harness<'static, XmlCraftApp>, label: &str) {
    let started = Instant::now();
    while h.query_by_label_contains(label).is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for {label:?}");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn run_query(h: &mut Harness<'static, XmlCraftApp>, query: &str, expect: &str) {
    h.get_by_label("The XQuery query").focus();
    h.run_steps(1);
    h.get_by_label("The XQuery query").type_text(query);
    h.run_steps(2);
    h.get_by_label("Run").click();
    wait_for(h, expect);
}

const SHOP: &str = "<shop><item n=\"2\">Pear</item><item n=\"1\">Apple</item></shop>";

#[test]
fn a_query_runs_on_the_document_and_a_node_of_the_result_is_selected() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "shop.xml", SHOP));
    assert!(h.state_mut().execute("xml.xquery"));
    h.run_steps(3);
    assert!(h.query_by_label("Close Query").is_some(), "the tool window is up");
    run_query(&mut h, "for $i in //item order by $i/@n return ($i, <seen>{string($i)}</seen>)", "4 items.");
    assert!(h.query_by_label_contains("<seen>Apple</seen>").is_some(), "a node the query built is shown serialized");
    h.get_by_label_contains("/shop[1]/item[2]").hover();
    h.run_steps(2);
    h.get_by_label_contains("/shop[1]/item[2]").click();
    h.run_steps(3);
    let doc = h.state().session.active_document().expect("shown");
    let text = doc.editor().text();
    let selection = doc.editor().selections()[0];
    let (start, end) = (selection.start().min(selection.end()), selection.start().max(selection.end()));
    assert_eq!(&text[start..end], "<item n=\"1\">Apple</item>", "the first item of the result is the one with n=1");
}

#[test]
fn an_error_says_its_code_and_where_it_is_in_the_query() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    open(&mut h, &file(dir.path(), "shop.xml", SHOP));
    assert!(h.state_mut().execute("xml.xquery"));
    h.run_steps(3);
    run_query(&mut h, "for $i in //item\nretrun $i", "XPST0003");
    assert!(h.query_by_label_contains("line 2, column 1 of the query").is_some(), "the place of the error");
}

#[test]
fn an_xquery_document_has_problems_while_typing_and_runs_from_the_command_bar() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    let broken = "declare function local:twice($n) { $n * 2 };\nlocal:twice(21) +";
    open(&mut h, &file(dir.path(), "broken.xq", broken));
    assert_eq!(h.state().problems(), Some((1, 0)), "the syntax error is a problem");
    let messages = h.state().problem_messages_at(broken.len());
    assert!(messages.iter().any(|m| m.contains("XPST0003")), "{messages:?}");
    // a query that reads has none, and the command bar's Run runs it in the Query tool
    open(&mut h, &file(dir.path(), "twice.xq", "declare function local:twice($n) { $n * 2 };\nlocal:twice(21)"));
    assert_eq!(h.state().problems(), Some((0, 0)));
    h.get_by_label("Run").click();
    wait_for(&mut h, "xs:integer");
    assert!(h.query_by_label_contains("42").is_some(), "the result of the document's query");
}
