//! The Debugger tab of the SOAP tool window in the real shell: a proxy on this computer in front of a target on this computer. A client's request
//! comes through and is listed; with requests held it waits until it is released from the window. Everything is on local sockets.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use xmlcraft_ui_egui::{Panel, XmlCraftApp};

/// A target that answers every request with a small SOAP envelope and sends what it got to the test.
fn target() -> (String, std::sync::mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let url = format!("http://{}/svc", listener.local_addr().expect("addr"));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            while let Ok(n) = stream.read(&mut buf) {
                got.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&got).into_owned();
                if n == 0 {
                    break;
                }
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
            let body = "<answer/>";
            let reply = format!("HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    (url, rx)
}

fn client(address: SocketAddr, body: &'static str) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut stream = TcpStream::connect(address).expect("connects");
        stream.set_read_timeout(Some(Duration::from_secs(15))).expect("timeout");
        let request = format!("POST /call HTTP/1.1\r\nHost: x\r\nContent-Type: text/xml\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        stream.write_all(request.as_bytes()).expect("writes");
        let mut bytes = Vec::new();
        let _ = stream.read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

fn shell() -> Harness<'static, XmlCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 900.0)).build_eframe(|cc| XmlCraftApp::new(cc));
    h.run_steps(4);
    h
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

fn address(h: &Harness<'static, XmlCraftApp>) -> SocketAddr {
    h.state().soap_debugger().and_then(|(a, _)| a).expect("listening").parse().expect("an address")
}

#[test]
fn a_request_comes_through_the_proxy_and_is_listed_with_its_path_and_stage() {
    let (url, seen) = target();
    let mut h = shell();
    assert!(h.state_mut().execute("xml.soap_debugger"));
    h.run_steps(3);
    assert!(h.query_by_label("Close SOAP").is_some(), "a tool window");
    assert_eq!(h.state().soap_debugger(), Some((None, Vec::new())), "not listening until Start");
    h.state_mut().soap_debugger_start("0", &url, true, false);
    wait(&mut h, "the proxy to listen", |app| app.soap_debugger().is_some_and(|(a, _)| a.is_some()));
    let answer = client(address(&h), "<ask/>").join().expect("client");
    assert!(answer.starts_with("HTTP/1.1 200 OK") && answer.ends_with("<answer/>"), "{answer}");
    assert!(seen.recv_timeout(Duration::from_secs(5)).expect("target").starts_with("POST /svc/call HTTP/1.1"));
    wait(&mut h, "the exchange to be done", |app| app.soap_debugger().is_some_and(|(_, rows)| rows.iter().any(|r| r.1 == "done")));
    let (_, rows) = h.state().soap_debugger().expect("window");
    assert_eq!(rows.iter().map(|r| r.2.as_str()).collect::<Vec<_>>(), ["/call"]);
    assert!(h.query_by_label_contains("POST /call").is_some(), "the list shows it");
    // Hiding the panel does not stop the proxy: it keeps listening and answering.
    let listening = address(&h);
    h.get_by_label("Close SOAP").click_accesskit();
    h.run_steps(3);
    assert!(!h.state().panel_visible(Panel::Soap));
    assert!(h.state().soap_debugger().is_some_and(|(a, _)| a.is_some()), "still listening");
    let second = client(listening, "<again/>").join().expect("client");
    assert!(second.ends_with("<answer/>"), "{second}");
    // Stop stops it.
    assert!(h.state_mut().execute("xml.soap_debugger"));
    h.run_steps(3);
    h.get_by_label("Stop").click_accesskit();
    h.run_steps(4);
    assert!(h.state().soap_debugger().is_some_and(|(a, _)| a.is_none()), "not listening");
    std::thread::sleep(Duration::from_millis(300));
    assert!(TcpStream::connect_timeout(&listening, Duration::from_millis(500)).is_err(), "the proxy stopped with Stop");
}

#[test]
fn a_held_request_is_released_from_the_window_and_only_then_reaches_the_target() {
    let (url, seen) = target();
    let mut h = shell();
    assert!(h.state_mut().execute("xml.soap_debugger"));
    h.run_steps(3);
    h.state_mut().soap_debugger_start("0", &url, true, true);
    wait(&mut h, "the proxy to listen", |app| app.soap_debugger().is_some_and(|(a, _)| a.is_some()));
    let pending = client(address(&h), "<held/>");
    wait(&mut h, "the request to be held", |app| app.soap_debugger().is_some_and(|(_, rows)| rows.iter().any(|r| r.1 == "request held")));
    assert!(seen.recv_timeout(Duration::from_millis(300)).is_err(), "nothing reached the target");
    h.get_by_label_contains("#1").click_accesskit();
    h.run_steps(3);
    h.get_by_label("Release").click_accesskit();
    h.run_steps(4);
    let answer = pending.join().expect("client");
    assert!(answer.ends_with("<answer/>"), "{answer}");
    assert!(seen.recv_timeout(Duration::from_secs(5)).expect("target").ends_with("<held/>"));
}

#[test]
fn a_message_held_while_the_panel_is_hidden_marks_the_rail_icon_until_the_panel_is_shown() {
    let (url, _seen) = target();
    let mut h = shell();
    assert!(h.state_mut().execute("xml.soap_debugger"));
    h.run_steps(3);
    h.state_mut().soap_debugger_start("0", &url, true, true);
    wait(&mut h, "the proxy to listen", |app| app.soap_debugger().is_some_and(|(a, _)| a.is_some()));
    let listening = address(&h);
    h.state_mut().set_panel_visible(Panel::Soap, false);
    h.run_steps(3);
    assert!(!h.state().panel_attention(Panel::Soap), "nothing is held yet");
    let pending = client(listening, "<held/>");
    wait(&mut h, "the rail icon to be marked", |app| app.panel_attention(Panel::Soap));
    // Showing the panel clears the mark; the message is there to be released.
    h.get_by_label("Toggle SOAP").click();
    h.run_steps(3);
    assert!(!h.state().panel_attention(Panel::Soap));
    h.get_by_label_contains("#1").click_accesskit();
    h.run_steps(3);
    h.get_by_label("Release").click_accesskit();
    h.run_steps(4);
    assert!(pending.join().expect("client").ends_with("<answer/>"));
}
