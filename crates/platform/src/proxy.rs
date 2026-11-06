//! An intercepting proxy for debugging web services: a person points a client at it, and every request it forwards (and answer it
//! brings back) is recorded and can be held, edited and released, or dropped.
//!
//! The proxy is started by a person (Start in the SOAP debugger window), listens on this computer only (`127.0.0.1`), and forwards
//! to one target the person named, through [`crate::client::send`] (so the same rules hold: no redirects, hosts on this computer or the
//! local network only when allowed, answers capped). It speaks plain HTTP/1.1 with one request to a connection (`Connection: close`);
//! a client that wants TLS talks to the target directly. Nothing runs unless it was started, and it ends when stopped.
//!
//! Held messages wait for a person: a request or an answer is held when breaking on that kind is on and the message contains the
//! condition text (when there is one). Released, it goes on as it was or as it was edited.

use std::time::Duration;

/// Where a message is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// The request is held, waiting for a person.
    HeldRequest,
    /// The request was sent on; the answer is awaited.
    Forwarding,
    /// The answer is held, waiting for a person.
    HeldResponse,
    Done,
    /// A person dropped it.
    Dropped,
    Failed,
}

/// The headers and the body of a request or an answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Message {
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// One request and what came of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exchange {
    pub id: u64,
    pub stage: Stage,
    pub method: String,
    /// The path and query as the client sent them.
    pub path: String,
    pub request: Message,
    /// The status and reason of the answer, once there is one.
    pub status: Option<(u16, String)>,
    pub response: Message,
    /// Why it failed.
    pub error: Option<String>,
}

/// What to start the proxy with.
#[derive(Clone, Debug)]
pub struct Config {
    /// The port on this computer (0: one the system picks).
    pub port: u16,
    /// Where requests are forwarded: `http://host:port/base`; the path of a request is added to it.
    pub target: String,
    /// Whether the target may be on this computer or the local network.
    pub allow_private_hosts: bool,
    pub timeout: Duration,
}

/// When messages are held.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Breaks {
    pub requests: bool,
    pub responses: bool,
    /// Hold only messages whose body contains this text (empty: all).
    pub contains: String,
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::{Breaks, Config, Exchange, Message, Stage};
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
    use std::time::{Duration, Instant};

    use crate::client::{Request, send};

    /// The most exchanges kept (the oldest go first).
    pub const MAX_EXCHANGES: usize = 500;
    /// The most connections served at once.
    const MAX_CONNECTIONS: usize = 32;
    /// The longest a request line and headers may be.
    const MAX_HEAD_BYTES: usize = 64 * 1024;
    /// The most body a request may have.
    const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
    /// How long a held message waits for a person.
    const MAX_HOLD: Duration = Duration::from_secs(15 * 60);
    /// How long a client may take to send its request.
    const READ_TIMEOUT: Duration = Duration::from_secs(30);

    /// Headers that belong to one connection and are not passed on.
    const HOP_BY_HOP: &[&str] =
        &["connection", "keep-alive", "proxy-connection", "transfer-encoding", "te", "trailer", "upgrade", "content-length", "host"];

    #[derive(Default)]
    struct State {
        exchanges: Vec<Exchange>,
        next_id: u64,
        breaks: Breaks,
        /// What the held exchanges were released with: `Some(edited)` forwards, `None` drops.
        released: Vec<(u64, Option<Message>)>,
    }

    struct Shared {
        state: Mutex<State>,
        wake: Condvar,
        stop: AtomicBool,
        connections: AtomicUsize,
        config: Config,
        on_change: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    }

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(PoisonError::into_inner)
    }

    impl Shared {
        fn changed(&self) {
            let callback = lock(&self.on_change).clone();
            if let Some(callback) = callback {
                callback();
            }
        }

        fn update(&self, id: u64, change: impl FnOnce(&mut Exchange)) {
            {
                let mut state = lock(&self.state);
                if let Some(exchange) = state.exchanges.iter_mut().find(|e| e.id == id) {
                    change(exchange);
                }
            }
            self.changed();
        }

        /// Whether a message with this body is held under the current breaks.
        fn holds(&self, response: bool, body: &[u8]) -> bool {
            let state = lock(&self.state);
            let wanted = if response { state.breaks.responses } else { state.breaks.requests };
            wanted && (state.breaks.contains.is_empty() || String::from_utf8_lossy(body).contains(&state.breaks.contains))
        }

        /// Wait until a person releases exchange `id`: the edited message to go on with, or none to drop it.
        fn wait_released(&self, id: u64) -> Option<Message> {
            let started = Instant::now();
            let mut state = lock(&self.state);
            loop {
                if let Some(at) = state.released.iter().position(|(r, _)| *r == id) {
                    return state.released.remove(at).1;
                }
                if self.stop.load(Ordering::SeqCst) || started.elapsed() >= MAX_HOLD {
                    return None;
                }
                let (guard, _) = self.wake.wait_timeout(state, Duration::from_millis(250)).unwrap_or_else(PoisonError::into_inner);
                state = guard;
            }
        }
    }

    /// A running proxy.
    pub struct Debugger {
        shared: Arc<Shared>,
        address: SocketAddr,
    }

    impl Debugger {
        /// Start listening on this computer. `Err` says why not (the port is taken, the target is not an `http` or `https` address).
        pub fn start(config: Config) -> Result<Debugger, String> {
            let target = config.target.trim();
            if !(target.starts_with("http://") || target.starts_with("https://")) {
                return Err("the target is an http or https address".to_owned());
            }
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, config.port)).map_err(|e| format!("port {}: {e}", config.port))?;
            let address = listener.local_addr().map_err(|e| e.to_string())?;
            // (A short wait for a connection, so that a stop is noticed.)
            listener.set_nonblocking(true).map_err(|e| e.to_string())?;
            let shared = Arc::new(Shared {
                state: Mutex::new(State::default()),
                wake: Condvar::new(),
                stop: AtomicBool::new(false),
                connections: AtomicUsize::new(0),
                config: Config { target: target.trim_end_matches('/').to_owned(), ..config },
                on_change: Mutex::new(None),
            });
            let for_thread = shared.clone();
            std::thread::Builder::new()
                .name("proxy".into())
                .spawn(move || accept_loop(&listener, &for_thread))
                .map_err(|e| format!("the proxy could not start: {e}"))?;
            Ok(Debugger { shared, address })
        }

        /// Where the proxy listens (what a client is pointed at).
        pub fn address(&self) -> SocketAddr {
            self.address
        }

        /// Called whenever an exchange changes.
        pub fn on_change(&self, callback: impl Fn() + Send + Sync + 'static) {
            *lock(&self.shared.on_change) = Some(Arc::new(callback));
        }

        /// What is recorded, oldest first.
        pub fn exchanges(&self) -> Vec<Exchange> {
            lock(&self.shared.state).exchanges.clone()
        }

        pub fn set_breaks(&self, breaks: Breaks) {
            lock(&self.shared.state).breaks = breaks;
        }

        pub fn breaks(&self) -> Breaks {
            lock(&self.shared.state).breaks.clone()
        }

        /// Let the held exchange `id` go on, as it was (`None`) or as `edited` says. False when it is not held.
        pub fn release(&self, id: u64, edited: Option<Message>) -> bool {
            self.finish_hold(id, true, edited)
        }

        /// Drop the held exchange `id`: the client is told that it was dropped. False when it is not held.
        pub fn drop_held(&self, id: u64) -> bool {
            self.finish_hold(id, false, None)
        }

        fn finish_hold(&self, id: u64, go_on: bool, edited: Option<Message>) -> bool {
            let mut state = lock(&self.shared.state);
            let Some(exchange) = state.exchanges.iter().find(|e| e.id == id) else { return false };
            if !matches!(exchange.stage, Stage::HeldRequest | Stage::HeldResponse) {
                return false;
            }
            let message = if go_on {
                Some(
                    edited.unwrap_or_else(|| if exchange.stage == Stage::HeldRequest { exchange.request.clone() } else { exchange.response.clone() }),
                )
            } else {
                None
            };
            state.released.push((id, message));
            drop(state);
            self.shared.wake.notify_all();
            true
        }

        /// Stop listening; what is held is dropped. Exchanges stay readable.
        pub fn stop(&self) {
            self.shared.stop.store(true, Ordering::SeqCst);
            self.shared.wake.notify_all();
        }
    }

    impl Drop for Debugger {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn accept_loop(listener: &TcpListener, shared: &Arc<Shared>) {
        while !shared.stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((stream, _)) => {
                    if shared.connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                        shared.connections.fetch_sub(1, Ordering::SeqCst);
                        let mut stream = stream;
                        let _ = stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        continue;
                    }
                    let for_connection = shared.clone();
                    let started = std::thread::Builder::new().name("proxy-connection".into()).spawn(move || {
                        serve(stream, &for_connection);
                        for_connection.connections.fetch_sub(1, Ordering::SeqCst);
                    });
                    if started.is_err() {
                        shared.connections.fetch_sub(1, Ordering::SeqCst);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(25)),
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    }

    /// A request as it was read.
    struct Head {
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    /// Read one request from `stream`: the request line, the headers and the body (by `Content-Length` or chunked).
    fn read_request(stream: &mut TcpStream) -> Result<Head, String> {
        let mut received: Vec<u8> = Vec::new();
        let mut buffer = [0u8; 8192];
        let split = loop {
            if let Some(at) = find(&received, b"\r\n\r\n") {
                break at;
            }
            if received.len() > MAX_HEAD_BYTES {
                return Err("the request head is too long".to_owned());
            }
            let n = stream.read(&mut buffer).map_err(|e| format!("reading the request: {e}"))?;
            if n == 0 {
                return Err("the client closed the connection".to_owned());
            }
            received.extend_from_slice(buffer.get(..n).unwrap_or_default());
        };
        let head = String::from_utf8_lossy(received.get(..split).unwrap_or_default()).into_owned();
        let mut lines = head.split("\r\n");
        let request_line = lines.next().unwrap_or_default();
        let mut parts = request_line.split(' ');
        let (method, path, version) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
        if method.is_empty() || !path.starts_with('/') || !version.starts_with("HTTP/") {
            return Err("this is not an HTTP request".to_owned());
        }
        let headers: Vec<(String, String)> =
            lines.filter_map(|l| l.split_once(':')).map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned())).collect();
        let header = |name: &str| headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str());
        if header("expect").is_some_and(|v| v.eq_ignore_ascii_case("100-continue")) {
            let _ = stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
        }
        let mut body: Vec<u8> = received.get(split + 4..).unwrap_or_default().to_vec();
        if header("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked")) {
            body = read_chunked(stream, body)?;
        } else {
            let length =
                header("content-length").map_or(Ok(0), |v| v.parse::<usize>().map_err(|_| "the Content-Length is not a number".to_owned()))?;
            if length > MAX_BODY_BYTES {
                return Err("the request body is too large".to_owned());
            }
            while body.len() < length {
                let n = stream.read(&mut buffer).map_err(|e| format!("reading the request body: {e}"))?;
                if n == 0 {
                    return Err("the client closed the connection before the body was complete".to_owned());
                }
                body.extend_from_slice(buffer.get(..n).unwrap_or_default());
            }
            body.truncate(length);
        }
        Ok(Head { method: method.to_ascii_uppercase(), path: path.to_owned(), headers, body })
    }

    /// Decode a chunked body: `already` is what was read after the headers; more is read as needed.
    fn read_chunked(stream: &mut TcpStream, already: Vec<u8>) -> Result<Vec<u8>, String> {
        let mut raw = already;
        let mut out: Vec<u8> = Vec::new();
        let mut at = 0usize;
        let mut buffer = [0u8; 8192];
        loop {
            // The size line.
            let line_end = loop {
                if let Some(n) = find(raw.get(at..).unwrap_or_default(), b"\r\n") {
                    break at + n;
                }
                if raw.len() > MAX_BODY_BYTES + MAX_HEAD_BYTES {
                    return Err("the request body is too large".to_owned());
                }
                let n = stream.read(&mut buffer).map_err(|e| format!("reading the request body: {e}"))?;
                if n == 0 {
                    return Err("the client closed the connection before the body was complete".to_owned());
                }
                raw.extend_from_slice(buffer.get(..n).unwrap_or_default());
            };
            let size_text = String::from_utf8_lossy(raw.get(at..line_end).unwrap_or_default()).into_owned();
            let size = usize::from_str_radix(size_text.split(';').next().unwrap_or_default().trim(), 16)
                .map_err(|_| "a chunk size is not a number".to_owned())?;
            at = line_end + 2;
            if size == 0 {
                return Ok(out);
            }
            if out.len().saturating_add(size) > MAX_BODY_BYTES {
                return Err("the request body is too large".to_owned());
            }
            let end = at.checked_add(size).and_then(|e| e.checked_add(2)).ok_or_else(|| "a chunk is too large".to_owned())?;
            while raw.len() < end {
                let n = stream.read(&mut buffer).map_err(|e| format!("reading the request body: {e}"))?;
                if n == 0 {
                    return Err("the client closed the connection before the body was complete".to_owned());
                }
                raw.extend_from_slice(buffer.get(..n).unwrap_or_default());
            }
            out.extend_from_slice(raw.get(at..at + size).unwrap_or_default());
            at = end;
        }
    }

    fn write_response(stream: &mut TcpStream, status: u16, reason: &str, headers: &[(String, String)], body: &[u8]) {
        let mut head = format!("HTTP/1.1 {status} {reason}\r\n");
        for (name, value) in headers {
            if HOP_BY_HOP.contains(&name.to_ascii_lowercase().as_str()) || value.contains(['\r', '\n']) || name.contains(['\r', '\n', ':']) {
                continue;
            }
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()));
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(body);
        let _ = stream.flush();
    }

    fn plain(stream: &mut TcpStream, status: u16, reason: &str, text: &str) {
        write_response(stream, status, reason, &[("Content-Type".to_owned(), "text/plain; charset=utf-8".to_owned())], text.as_bytes());
    }

    /// One connection: read the request, record it, hold it when asked, forward it, record and hold the answer, give it back.
    fn serve(mut stream: TcpStream, shared: &Arc<Shared>) {
        let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
        let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
        let head = match read_request(&mut stream) {
            Ok(head) => head,
            Err(why) => {
                plain(&mut stream, 400, "Bad Request", &why);
                return;
            }
        };
        let id = {
            let mut state = lock(&shared.state);
            state.next_id += 1;
            let id = state.next_id;
            state.exchanges.push(Exchange {
                id,
                stage: Stage::Forwarding,
                method: head.method.clone(),
                path: head.path.clone(),
                request: Message { headers: head.headers.clone(), body: head.body.clone() },
                status: None,
                response: Message::default(),
                error: None,
            });
            if state.exchanges.len() > MAX_EXCHANGES {
                let excess = state.exchanges.len() - MAX_EXCHANGES;
                state.exchanges.drain(..excess);
            }
            id
        };
        shared.changed();
        let mut request = Message { headers: head.headers, body: head.body };
        if shared.holds(false, &request.body) {
            shared.update(id, |e| e.stage = Stage::HeldRequest);
            match shared.wait_released(id) {
                Some(edited) => {
                    request = edited;
                    shared.update(id, |e| {
                        e.request = request.clone();
                        e.stage = Stage::Forwarding;
                    });
                }
                None => {
                    shared.update(id, |e| e.stage = Stage::Dropped);
                    plain(&mut stream, 502, "Bad Gateway", "The request was dropped in the debugger.");
                    return;
                }
            }
        }
        let mut outgoing = Request::new(&head.method, &format!("{}{}", shared.config.target, head.path));
        outgoing.headers = request.headers.clone();
        outgoing.body = request.body.clone();
        outgoing.allow_private_hosts = shared.config.allow_private_hosts;
        outgoing.timeout = shared.config.timeout;
        let response = match send("XmlCraft-debugger", &outgoing) {
            Ok(response) => response,
            Err(why) => {
                shared.update(id, |e| {
                    e.stage = Stage::Failed;
                    e.error = Some(why.clone());
                });
                plain(&mut stream, 502, "Bad Gateway", &why);
                return;
            }
        };
        let mut answer = Message { headers: response.headers.clone(), body: response.body.clone() };
        let mut status = (response.status, response.reason.clone());
        shared.update(id, |e| {
            e.status = Some(status.clone());
            e.response = answer.clone();
            e.stage = Stage::Done;
        });
        if shared.holds(true, &answer.body) {
            shared.update(id, |e| e.stage = Stage::HeldResponse);
            match shared.wait_released(id) {
                Some(edited) => {
                    answer = edited;
                    shared.update(id, |e| {
                        e.response = answer.clone();
                        e.stage = Stage::Done;
                    });
                }
                None => {
                    shared.update(id, |e| e.stage = Stage::Dropped);
                    plain(&mut stream, 502, "Bad Gateway", "The answer was dropped in the debugger.");
                    return;
                }
            }
        }
        if status.1.is_empty() {
            status.1 = "OK".to_owned();
        }
        write_response(&mut stream, status.0, &status.1, &answer.headers, &answer.body);
    }

    #[cfg(test)]
    #[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
    mod tests {
        use super::*;

        /// A target server on this computer: answers each request with `answer` and sends what it got to the test.
        fn target(answer: &'static str) -> (String, std::sync::mpsc::Receiver<String>) {
            let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
            let url = format!("http://{}/base", listener.local_addr().expect("addr"));
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
                    let reply =
                        format!("HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}", answer.len());
                    let _ = stream.write_all(reply.as_bytes());
                }
            });
            (url, rx)
        }

        /// A client: send `request` to the proxy and read the whole answer.
        fn ask(address: SocketAddr, request: &str) -> String {
            let mut stream = TcpStream::connect(address).expect("connects");
            stream.set_read_timeout(Some(Duration::from_secs(10))).expect("timeout");
            stream.write_all(request.as_bytes()).expect("writes");
            let mut out = String::new();
            let mut bytes = Vec::new();
            let _ = stream.read_to_end(&mut bytes);
            out.push_str(&String::from_utf8_lossy(&bytes));
            out
        }

        fn config(target: &str) -> Config {
            Config { port: 0, target: target.to_owned(), allow_private_hosts: true, timeout: Duration::from_secs(10) }
        }

        fn post(path: &str, body: &str) -> String {
            format!(
                "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: text/xml\r\nSOAPAction: \"urn:x\"\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
        }

        fn wait_for(debugger: &Debugger, what: &str, done: impl Fn(&[Exchange]) -> bool) {
            let started = Instant::now();
            while !done(&debugger.exchanges()) {
                assert!(started.elapsed() < Duration::from_secs(10), "waiting for {what}");
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        #[test]
        fn a_request_is_forwarded_to_the_target_and_the_answer_comes_back_and_both_are_recorded() {
            let (url, seen) = target("<answer/>");
            let debugger = Debugger::start(config(&url)).expect("starts");
            let answer = ask(debugger.address(), &post("/calc?x=1", "<ask/>"));
            assert!(answer.starts_with("HTTP/1.1 200 OK") && answer.ends_with("<answer/>"), "{answer}");
            let got = seen.recv_timeout(Duration::from_secs(5)).expect("the target got it");
            assert!(got.starts_with("POST /base/calc?x=1 HTTP/1.1") && got.ends_with("<ask/>"), "{got}");
            assert!(got.to_ascii_lowercase().contains("soapaction: \"urn:x\""), "{got}");
            let exchanges = debugger.exchanges();
            assert_eq!(exchanges.len(), 1);
            let e = &exchanges[0];
            assert_eq!((e.stage, e.method.as_str(), e.path.as_str()), (Stage::Done, "POST", "/calc?x=1"));
            assert_eq!(
                (e.request.body.as_slice(), e.response.body.as_slice(), e.status.as_ref().map(|s| s.0)),
                (&b"<ask/>"[..], &b"<answer/>"[..], Some(200))
            );
        }

        #[test]
        fn a_held_request_waits_for_a_person_and_goes_on_as_edited() {
            let (url, seen) = target("<ok/>");
            let debugger = Debugger::start(config(&url)).expect("starts");
            debugger.set_breaks(Breaks { requests: true, responses: false, contains: "interesting".to_owned() });
            let address = debugger.address();
            let client = std::thread::spawn(move || ask(address, &post("/s", "<interesting/>")));
            wait_for(&debugger, "the request to be held", |e| e.first().is_some_and(|x| x.stage == Stage::HeldRequest));
            assert!(seen.recv_timeout(Duration::from_millis(200)).is_err(), "nothing reached the target while it is held");
            let held = debugger.exchanges()[0].clone();
            let mut edited = held.request.clone();
            edited.body = b"<edited/>".to_vec();
            assert!(debugger.release(held.id, Some(edited)));
            let answer = client.join().expect("client");
            assert!(answer.ends_with("<ok/>"), "{answer}");
            let got = seen.recv_timeout(Duration::from_secs(5)).expect("the target got it");
            assert!(
                got.ends_with("<edited/>") && got.contains("Content-Length: 9") || got.to_ascii_lowercase().contains("content-length: 9"),
                "{got}"
            );
            assert!(!debugger.release(held.id, None), "it is not held any more");
            // A request that does not contain the text is not held.
            let plain_answer = ask(debugger.address(), &post("/s", "<boring/>"));
            assert!(plain_answer.ends_with("<ok/>"), "{plain_answer}");
        }

        #[test]
        fn a_dropped_request_never_reaches_the_target_and_a_held_answer_can_be_edited() {
            let (url, seen) = target("<real/>");
            let debugger = Debugger::start(config(&url)).expect("starts");
            debugger.set_breaks(Breaks { requests: true, responses: false, contains: String::new() });
            let address = debugger.address();
            let client = std::thread::spawn(move || ask(address, &post("/s", "<a/>")));
            wait_for(&debugger, "the hold", |e| e.first().is_some_and(|x| x.stage == Stage::HeldRequest));
            assert!(debugger.drop_held(debugger.exchanges()[0].id));
            let answer = client.join().expect("client");
            assert!(answer.starts_with("HTTP/1.1 502") && answer.contains("dropped"), "{answer}");
            assert!(seen.recv_timeout(Duration::from_millis(300)).is_err());
            assert_eq!(debugger.exchanges()[0].stage, Stage::Dropped);
            // Now only the answer is held, and the client gets it as it was edited.
            debugger.set_breaks(Breaks { requests: false, responses: true, contains: String::new() });
            let client = std::thread::spawn(move || ask(address, &post("/s", "<b/>")));
            wait_for(&debugger, "the held answer", |e| e.iter().any(|x| x.stage == Stage::HeldResponse));
            let held = debugger.exchanges().into_iter().find(|e| e.stage == Stage::HeldResponse).expect("held");
            assert_eq!(held.response.body, b"<real/>");
            let mut edited = held.response.clone();
            edited.body = b"<changed/>".to_vec();
            assert!(debugger.release(held.id, Some(edited)));
            assert!(client.join().expect("client").ends_with("<changed/>"));
        }

        #[test]
        fn what_is_not_an_http_request_or_is_too_large_is_refused_and_a_dead_target_is_a_bad_gateway() {
            let (url, _seen) = target("<x/>");
            let debugger = Debugger::start(config(&url)).expect("starts");
            assert!(ask(debugger.address(), "hello\r\n\r\n").starts_with("HTTP/1.1 400"));
            assert!(ask(debugger.address(), "POST /s HTTP/1.1\r\nContent-Length: x\r\n\r\n").starts_with("HTTP/1.1 400"));
            let too_large = format!("POST /s HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY_BYTES + 1);
            assert!(ask(debugger.address(), &too_large).starts_with("HTTP/1.1 400"));
            let dead = Debugger::start(config("http://127.0.0.1:1/x")).expect("starts");
            let answer = ask(dead.address(), &post("/s", "<a/>"));
            assert!(answer.starts_with("HTTP/1.1 502"), "{answer}");
            assert_eq!(dead.exchanges()[0].stage, Stage::Failed);
            assert!(Debugger::start(config("ftp://x")).is_err());
        }

        #[test]
        fn a_chunked_request_body_is_decoded_and_stop_frees_what_is_held() {
            let (url, seen) = target("<ok/>");
            let debugger = Debugger::start(config(&url)).expect("starts");
            let chunked = "POST /s HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n3\r\n<a/\r\n2\r\n>x\r\n0\r\n\r\n";
            assert!(ask(debugger.address(), chunked).ends_with("<ok/>"));
            assert!(seen.recv_timeout(Duration::from_secs(5)).expect("got it").ends_with("<a/>x"));
            debugger.set_breaks(Breaks { requests: true, responses: false, contains: String::new() });
            let address = debugger.address();
            let client = std::thread::spawn(move || ask(address, &post("/s", "<held/>")));
            wait_for(&debugger, "the hold", |e| e.iter().any(|x| x.stage == Stage::HeldRequest));
            debugger.stop();
            assert!(client.join().expect("client").starts_with("HTTP/1.1 502"), "stopping drops what is held");
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg(not(target_arch = "wasm32"))]
pub use native::{Debugger, MAX_EXCHANGES};

/// On the web there are no sockets to listen on: the debugger does not start.
#[cfg(target_arch = "wasm32")]
pub struct Debugger;

#[cfg(target_arch = "wasm32")]
impl Debugger {
    pub fn start(_config: Config) -> Result<Debugger, String> {
        Err("the debugger needs sockets, which the web build does not have".to_owned())
    }

    pub fn address(&self) -> std::net::SocketAddr {
        std::net::SocketAddr::from(([127, 0, 0, 1], 0))
    }

    pub fn on_change(&self, _callback: impl Fn() + Send + Sync + 'static) {}

    pub fn exchanges(&self) -> Vec<Exchange> {
        Vec::new()
    }

    pub fn set_breaks(&self, _breaks: Breaks) {}

    pub fn breaks(&self) -> Breaks {
        Breaks::default()
    }

    pub fn release(&self, _id: u64, _edited: Option<Message>) -> bool {
        false
    }

    pub fn drop_held(&self, _id: u64) -> bool {
        false
    }

    pub fn stop(&self) {}
}

/// The most exchanges kept.
#[cfg(target_arch = "wasm32")]
pub const MAX_EXCHANGES: usize = 500;
