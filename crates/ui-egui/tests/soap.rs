//! The SOAP window in the real shell: the request made for an operation, sent to a server on this computer (which the person allows
//! for the request), and refused while the app works offline. The server is a local socket; nothing goes to the web.

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

fn wsdl(address: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<wsdl:definitions xmlns:wsdl="http://schemas.xmlsoap.org/wsdl/" xmlns:soap="http://schemas.xmlsoap.org/wsdl/soap/" xmlns:xs="http://www.w3.org/2001/XMLSchema"
    xmlns:tns="http://example.com/calc" targetNamespace="http://example.com/calc">
  <wsdl:types><xs:schema targetNamespace="http://example.com/calc" elementFormDefault="qualified">
    <xs:element name="Add"><xs:complexType><xs:sequence><xs:element name="a" type="xs:int"/></xs:sequence></xs:complexType></xs:element>
    <xs:element name="AddResponse"><xs:complexType><xs:sequence><xs:element name="r" type="xs:int"/></xs:sequence></xs:complexType></xs:element>
  </xs:schema></wsdl:types>
  <wsdl:message name="In"><wsdl:part name="p" element="tns:Add"/></wsdl:message>
  <wsdl:message name="Out"><wsdl:part name="p" element="tns:AddResponse"/></wsdl:message>
  <wsdl:portType name="P"><wsdl:operation name="Add"><wsdl:input message="tns:In"/><wsdl:output message="tns:Out"/></wsdl:operation></wsdl:portType>
  <wsdl:binding name="B11" type="tns:P"><soap:binding style="document" transport="http://schemas.xmlsoap.org/soap/http"/>
    <wsdl:operation name="Add"><soap:operation soapAction="urn:add"/><wsdl:input><soap:body use="literal"/></wsdl:input><wsdl:output><soap:body use="literal"/></wsdl:output></wsdl:operation></wsdl:binding>
  <wsdl:service name="S"><wsdl:port name="B11" binding="tns:B11"><soap:address location="{address}"/></wsdl:port></wsdl:service>
</wsdl:definitions>"#
    )
}

const ANSWER: &str = r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><AddResponse xmlns="http://example.com/calc"><r>3</r></AddResponse></s:Body></s:Envelope>"#;

/// A server on this computer that answers one request with `ANSWER` and hands the request it got to the test.
fn serve() -> (String, std::sync::mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let address = format!("http://{}/calc", listener.local_addr().expect("addr"));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else { return };
        let mut got = Vec::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = stream.read(&mut buf) {
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&got).into_owned();
            if let Some((head, body)) = text.split_once("\r\n\r\n") {
                let length = head
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()))
                    .unwrap_or(0);
                if body.len() >= length {
                    break;
                }
            }
        }
        let _ = tx.send(String::from_utf8_lossy(&got).into_owned());
        let reply = format!("HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{ANSWER}", ANSWER.len());
        let _ = stream.write_all(reply.as_bytes());
    });
    (address, rx)
}

fn shell(online: bool) -> Harness<'static, XmlCraftApp> {
    let network = if online {
        Network::new(Some(Arc::new(NoWeb)), Some(Arc::new(MemoryCache::new())), Arc::new(FixedClock(1_000)))
    } else {
        Network::offline()
    };
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

fn open_window(h: &mut Harness<'static, XmlCraftApp>, dir: &Path, address: &str) {
    assert!(h.state_mut().open_path(&file(dir, "calc.wsdl", &wsdl(address))));
    h.run_steps(3);
    assert!(h.state_mut().execute("xml.soap"));
    wait(h, "the request", |app| app.soap_window().is_some_and(|(body, _, _)| !body.is_empty()));
}

#[test]
fn the_request_made_for_an_operation_is_sent_where_the_person_allows_and_the_answer_is_read() {
    let dir = tempfile::tempdir().expect("dir");
    let (address, received) = serve();
    let mut h = shell(true);
    open_window(&mut h, dir.path(), &address);
    let (request, answer, error) = h.state().soap_window().expect("window");
    assert!(request.contains("http://schemas.xmlsoap.org/soap/envelope/") && request.contains(":a>"), "{request}");
    assert_eq!((answer, error), (None, None));
    // The server is on this computer: it is refused until the person allows it.
    h.get_by_label("Send").click_accesskit();
    wait(&mut h, "the refusal", |app| app.soap_window().is_some_and(|(_, _, e)| e.is_some()));
    let (_, _, error) = h.state().soap_window().expect("window");
    assert!(
        error.is_some_and(|e| e.to_lowercase().contains("local")
            || e.to_lowercase().contains("private")
            || e.to_lowercase().contains("this computer"))
    );
    h.get_by_label("Allow this computer and the local network").click_accesskit();
    h.run_steps(2);
    h.get_by_label("Send").click_accesskit();
    wait(&mut h, "the answer", |app| app.soap_window().is_some_and(|(_, a, _)| a.is_some()));
    let (_, answer, error) = h.state().soap_window().expect("window");
    assert_eq!(error, None);
    let (status, body) = answer.expect("answer");
    assert_eq!(status, 200);
    assert!(body.contains("<r>3</r>"), "{body}");
    let sent = received.recv_timeout(Duration::from_secs(5)).expect("the server got the request");
    assert!(sent.starts_with("POST /calc HTTP/1.1"), "{sent}");
    assert!(sent.to_ascii_lowercase().contains("soapaction: \"urn:add\"") || sent.to_ascii_lowercase().contains("soapaction: urn:add"), "{sent}");
    assert!(sent.contains("<soapenv:Envelope"), "{sent}");
}

#[test]
fn nothing_is_sent_while_the_app_works_offline() {
    let dir = tempfile::tempdir().expect("dir");
    let (address, received) = serve();
    let mut h = shell(false);
    open_window(&mut h, dir.path(), &address);
    h.get_by_label("Allow this computer and the local network").click_accesskit();
    h.run_steps(2);
    h.get_by_label("Send").click_accesskit();
    h.run_steps(5);
    let (_, answer, error) = h.state().soap_window().expect("window");
    assert_eq!(answer, None);
    assert!(error.is_some_and(|e| e.contains("offline")));
    assert!(received.recv_timeout(Duration::from_millis(300)).is_err(), "the server was not asked");
}

#[test]
fn a_document_that_is_not_a_wsdl_says_so_and_opens_no_window() {
    let dir = tempfile::tempdir().expect("dir");
    let mut h = shell(true);
    assert!(h.state_mut().open_path(&file(dir.path(), "a.xml", "<a/>")));
    h.run_steps(3);
    assert!(h.state_mut().execute("xml.soap"));
    h.run_steps(3);
    assert!(h.state().soap_window().is_none());
    assert!(h.state().status.contains("not a WSDL"), "{}", h.state().status);
}
