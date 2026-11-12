//! The HTTP & OpenAPI tool window in the real shell: the request made for an operation (with a server chosen from the description), sent to a
//! server on this computer, and the answer shown. The server is a local socket; nothing goes to the web.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_resolve::{Conditions, FetchError, FetchLimits, FetchOutcome, FixedClock, MemoryCache};
use xmlcraft_ui_egui::{Network, XmlCraftApp};

struct NoWeb;

impl xmlcraft_resolve::Fetcher for NoWeb {
    fn fetch(&self, _: &str, _: &Conditions, _: &FetchLimits) -> Result<FetchOutcome, FetchError> {
        Err(FetchError::Status(404))
    }
}

fn api(server: &str) -> String {
    format!(
        r#"{{"openapi": "3.0.3", "info": {{"title": "Pets", "version": "1"}}, "servers": [{{"url": "{server}"}}],
          "paths": {{"/pets/{{id}}": {{"get": {{"operationId": "getPet", "summary": "One pet",
            "parameters": [{{"name": "id", "in": "path", "required": true, "schema": {{"type": "integer"}}}}]}}}}}}}}"#
    )
}

const ANSWER: &str = r#"{"id": 0, "name": "Rex"}"#;

fn serve() -> (String, std::sync::mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let address = format!("http://{}", listener.local_addr().expect("addr"));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else { return };
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = stream.read(&mut buf) {
            got.extend_from_slice(&buf[..n]);
            if n == 0 || got.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let _ = tx.send(String::from_utf8_lossy(&got).into_owned());
        let reply =
            format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{ANSWER}", ANSWER.len());
        let _ = stream.write_all(reply.as_bytes());
    });
    (address, rx)
}

fn shell() -> Harness<'static, XmlCraftApp> {
    let network = Network::new(Some(Arc::new(NoWeb)), Some(Arc::new(MemoryCache::new())), Arc::new(FixedClock(1_000)));
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 800.0)).build_eframe(move |cc| XmlCraftApp::with_network(cc, network.clone()));
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
        assert!(started.elapsed() < Duration::from_secs(15), "waiting for {what}");
        h.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    h.run_steps(2);
}

#[test]
fn the_request_of_an_operation_is_sent_with_the_method_and_the_path_filled_in_and_the_answer_is_shown() {
    let dir = tempfile::tempdir().expect("dir");
    let (server, received) = serve();
    let mut h = shell();
    assert!(h.state_mut().open_path(&file(dir.path(), "pets.json", &api(&server))));
    h.run_steps(3);
    assert!(h.state_mut().execute("xml.openapi"));
    h.run_steps(3);
    assert!(h.state().openapi_window().is_some());
    assert!(h.query_by_label("Close HTTP & OpenAPI").is_some(), "a tool window");
    assert!(h.query_by_label("→ pets.json").is_some(), "it says which document it works on");
    assert_eq!(h.state().openapi_request(), Some(("GET".to_owned(), format!("{server}/pets/0"))), "the request of the first operation");
    h.get_by_label("Allow this computer and the local network").click_accesskit();
    h.run_steps(2);
    h.get_by_label("Send").click_accesskit();
    wait(&mut h, "the answer", |app| app.openapi_window().is_some_and(|(_, a, e)| a.is_some() || e.is_some()));
    let (_, answer, error) = h.state().openapi_window().expect("window");
    assert_eq!(error, None);
    let (status, body) = answer.expect("answer");
    assert_eq!((status, body.contains("Rex")), (200, true), "{body}");
    let sent = received.recv_timeout(Duration::from_secs(5)).expect("the server got the request");
    assert!(sent.starts_with("GET /pets/0 HTTP/1.1"), "{sent}");
}

#[test]
fn a_yaml_description_is_read_and_a_document_that_is_not_one_says_so() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    let yaml =
        "openapi: 3.1.0\ninfo: {title: T, version: '1'}\nservers: [{url: 'http://localhost:9'}]\npaths:\n  /ping:\n    get: {operationId: ping}\n";
    assert!(h.state_mut().open_path(&file(dir.path(), "api.yaml", yaml)));
    h.run_steps(3);
    assert!(h.state_mut().execute("xml.openapi"));
    h.run_steps(3);
    assert!(h.state().openapi_window().is_some(), "{}", h.state().status);
    assert_eq!(h.state().openapi_request(), Some(("GET".to_owned(), "http://localhost:9/ping".to_owned())));
    // A JSON document that is no description says so.
    let mut other = shell();
    assert!(other.state_mut().open_path(&file(dir.path(), "plain.json", "{\"a\": 1}")));
    other.run_steps(3);
    assert!(other.state_mut().execute("xml.openapi"));
    other.run_steps(3);
    assert!(other.state().openapi_window().is_none());
    assert!(other.state().status.contains("not an OpenAPI description"), "{}", other.state().status);
    assert!(other.query_by_label_contains("needs an OpenAPI description").is_some(), "and the panel says what it needs");
}

#[test]
fn the_tool_follows_the_document_in_front_and_keeps_the_request_it_made() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell();
    assert!(h.state_mut().open_path(&file(dir.path(), "pets.json", &api("http://localhost:9"))));
    h.run_steps(3);
    assert!(h.state_mut().execute("xml.openapi"));
    h.run_steps(3);
    assert_eq!(h.state().openapi_request(), Some(("GET".to_owned(), "http://localhost:9/pets/0".to_owned())));
    // A plain document in front: the panel says what it needs. The description is back in front: the request is as it was.
    assert!(h.state_mut().open_path(&file(dir.path(), "plain.json", "{\"a\": 1}")));
    h.run_steps(4);
    assert!(h.query_by_label_contains("needs an OpenAPI description").is_some());
    assert!(h.query_by_label("→ plain.json").is_some());
    h.state_mut().session.set_active(Some(0));
    h.run_steps(4);
    assert!(h.query_by_label("→ pets.json").is_some());
    assert_eq!(h.state().openapi_request(), Some(("GET".to_owned(), "http://localhost:9/pets/0".to_owned())));
}
