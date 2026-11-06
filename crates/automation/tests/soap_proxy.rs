//! The debugging proxy tools, driven through `Automation::call` with a target and a client on local sockets: forwarding and recording,
//! holding a request and releasing it edited, dropping, and the refusals (offline, started twice, nothing held).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, Network};

fn online() -> Automation {
    Automation::new().with_network(Network::standard())
}

fn call(a: &mut Automation, tool: &str, args: Value) -> Value {
    match a.call(tool, &args).unwrap_or_else(|e| panic!("{tool} failed: {e}")).into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    }
}

fn target() -> (String, std::sync::mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
    let url = format!("http://{}/svc", listener.local_addr().expect("addr"));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
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
            let reply = "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: 9\r\nConnection: close\r\n\r\n<answer/>";
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    (url, rx)
}

fn client(address: SocketAddr, body: &'static str) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut stream = TcpStream::connect(address).expect("connects");
        stream.set_read_timeout(Some(Duration::from_secs(15))).expect("timeout");
        let request = format!("POST /call HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        stream.write_all(request.as_bytes()).expect("writes");
        let mut bytes = Vec::new();
        let _ = stream.read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

fn wait_for(a: &mut Automation, what: &str, stage: &str) -> Value {
    let started = Instant::now();
    loop {
        let state = call(a, "soap_proxy_state", json!({}));
        if let Some(found) = state["exchanges"].as_array().and_then(|l| l.iter().find(|e| e["stage"] == stage)) {
            return found.clone();
        }
        assert!(started.elapsed() < Duration::from_secs(10), "waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_request_is_forwarded_recorded_held_and_released_edited() {
    let (url, seen) = target();
    let mut a = online();
    let started = call(&mut a, "soap_proxy_start", json!({ "target": url, "allow_private_hosts": true, "requests": true, "contains": "hold-me" }));
    let address: SocketAddr = started["address"].as_str().expect("address").parse().expect("addr");
    // A message without the text goes straight through.
    let through = client(address, "<plain/>").join().expect("client");
    assert!(through.ends_with("<answer/>"), "{through}");
    assert!(seen.recv_timeout(Duration::from_secs(5)).expect("target").ends_with("<plain/>"));
    // One with it is held until released, and goes on edited.
    let held_client = client(address, "<hold-me/>");
    let held = wait_for(&mut a, "the hold", "request_held");
    assert_eq!((held["method"].as_str(), held["path"].as_str(), held["request"]["body"].as_str()), (Some("POST"), Some("/call"), Some("<hold-me/>")));
    assert!(seen.recv_timeout(Duration::from_millis(200)).is_err(), "held");
    let released = call(&mut a, "soap_proxy_release", json!({ "id": held["id"], "body": "<edited/>" }));
    assert_eq!(released["released"].as_bool(), Some(true));
    assert!(held_client.join().expect("client").ends_with("<answer/>"));
    assert!(seen.recv_timeout(Duration::from_secs(5)).expect("target").ends_with("<edited/>"));
    let state = call(&mut a, "soap_proxy_state", json!({}));
    let stages: Vec<&str> = state["exchanges"].as_array().expect("list").iter().filter_map(|e| e["stage"].as_str()).collect();
    assert_eq!(stages, ["done", "done"]);
    assert_eq!(state["exchanges"][1]["response"]["body"].as_str(), Some("<answer/>"));
    let stopped = call(&mut a, "soap_proxy_stop", json!({}));
    assert_eq!(stopped["stopped"].as_bool(), Some(true));
    assert_eq!(call(&mut a, "soap_proxy_state", json!({}))["address"], Value::Null);
}

#[test]
fn a_dropped_request_never_reaches_the_target_and_the_refusals_are_said() {
    let (url, seen) = target();
    let mut a = online();
    let started = call(&mut a, "soap_proxy_start", json!({ "target": url, "allow_private_hosts": true, "requests": true }));
    let address: SocketAddr = started["address"].as_str().expect("address").parse().expect("addr");
    let e = a.call("soap_proxy_start", &json!({ "target": url })).expect_err("twice").to_string();
    assert!(e.contains("already started"), "{e}");
    let pending = client(address, "<drop/>");
    let held = wait_for(&mut a, "the hold", "request_held");
    let dropped = call(&mut a, "soap_proxy_drop", json!({ "id": held["id"] }));
    assert_eq!(dropped["dropped"].as_bool(), Some(true));
    assert!(pending.join().expect("client").starts_with("HTTP/1.1 502"));
    assert!(seen.recv_timeout(Duration::from_millis(300)).is_err());
    let e = a.call("soap_proxy_release", &json!({ "id": held["id"] })).expect_err("not held any more").to_string();
    assert!(e.contains("not held"), "{e}");
    call(&mut a, "soap_proxy_stop", json!({}));
    // Offline sessions start nothing; tools that need the proxy say it is not started.
    let mut offline = Automation::new();
    let e = offline.call("soap_proxy_start", &json!({ "target": "http://example.com" })).expect_err("offline").to_string();
    assert!(e.contains("offline"), "{e}");
    let e = offline.call("soap_proxy_breaks", &json!({ "requests": true })).expect_err("not started").to_string();
    assert!(e.contains("not started"), "{e}");
    assert!(a.call("soap_proxy_start", &json!({ "target": "ftp://x" })).is_err());
}
