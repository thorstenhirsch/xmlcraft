//! The transport of the UI control channel, and its client.
//!
//! **Opt-in only.** Nothing here runs unless the app is started with `--control <file>`. The server listens on loopback only, on a
//! random port, and every connection must first present the random token written (readable by its owner only) to `<file>`. What
//! the requests *mean* is the app's business (the egui half is `xmlcraft-ui-egui`'s `control` module): this module only moves
//! them, so it can be tested without a window.
//!
//! Protocol: newline-delimited JSON-RPC 2.0. The first request on a connection must be `{"method": "auth", "params": {"token": …}}`;
//! anything else closes the connection, and so does a line longer than [`MAX_AUTH_LINE`] before that, or the deadline passing
//! without it. An unauthenticated client can therefore neither hold a thread for ever nor use up all of them.

use std::io::{BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::lines::{LineRead, read_line_limited};

/// A request's answer: the result, or a message for the person or agent.
pub type Reply = Result<Value, String>;

/// Where a running app's control channel can be reached (written to the `--control` file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub port: u16,
    pub token: String,
}

/// What the server asks of the app: answer one request, later, on the returned channel.
pub trait Dispatch: Send + Sync + 'static {
    fn dispatch(&self, method: &str, params: Value) -> Receiver<Reply>;
}

impl<F> Dispatch for F
where
    F: Fn(&str, Value) -> Receiver<Reply> + Send + Sync + 'static,
{
    fn dispatch(&self, method: &str, params: Value) -> Receiver<Reply> {
        self(method, params)
    }
}

/// What a connection may do, so a local client cannot hold threads for ever or exhaust them.
#[derive(Clone, Copy)]
pub(crate) struct Limits {
    /// How long a connection has to authenticate, counted from its acceptance: an absolute deadline, so a client feeding one byte
    /// at a time cannot extend it.
    pub auth_deadline: Duration,
    /// How many connections may wait for authentication at once; further ones are refused. A refused connection keeps its place
    /// until it is closed, so the number of threads it can cost is bounded by this too.
    pub max_unauthenticated: usize,
    /// How long a write may block on a client that never reads its replies.
    pub write_timeout: Duration,
    /// How long a connection refused after a reply is kept half-open, discarding input, so the reply is not lost to a reset.
    pub linger: Duration,
    /// How long the app has to answer one request.
    pub reply_timeout: Duration,
}

pub(crate) const LIMITS: Limits = Limits {
    auth_deadline: Duration::from_secs(10),
    max_unauthenticated: 8,
    write_timeout: Duration::from_secs(30),
    linger: Duration::from_secs(1),
    reply_timeout: Duration::from_secs(30),
};

/// Longest line read before a connection has authenticated. The only acceptable request then is `auth`, about a hundred bytes; a
/// longer line is refused without buffering the rest of it.
pub const MAX_AUTH_LINE: usize = 4 * 1024;

/// Longest line read after authentication (a request is a small JSON object; this is generous).
pub const MAX_LINE: usize = 16 * 1024 * 1024;

/// How long the accepting thread waits after a failed accept (for example while the process has no file descriptors left), so it
/// does not keep a core busy until it recovers.
const ACCEPT_RETRY: Duration = Duration::from_millis(100);

/// One of a limited number of places, given back when dropped.
struct Slot(Arc<AtomicUsize>);

impl Slot {
    fn take(places: &Arc<AtomicUsize>, max: usize) -> Option<Self> {
        let mut n = places.load(Ordering::Acquire);
        loop {
            if n >= max {
                return None;
            }
            match places.compare_exchange_weak(n, n.saturating_add(1), Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Some(Slot(places.clone())),
                Err(now) => n = now,
            }
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The reading half of a connection, with the authentication deadline: every read before it gets the time left as its timeout, and
/// none at all once it has passed.
struct Reading {
    stream: TcpStream,
    until: Option<Instant>,
}

impl Read for Reading {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(until) = self.until {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "the authentication deadline passed"));
            }
            self.stream.set_read_timeout(Some(left))?;
        }
        self.stream.read(buf)
    }
}

/// Close a connection whose last reply has just been written without losing that reply. Closing a socket with input still unread
/// makes the kernel send a reset instead of a FIN, and the reset can discard the reply before the client reads it (Linux does): a
/// client still sending when it is refused would never learn why. So send FIN first, then read and discard whatever arrives until
/// the client closes its side or `linger` passes, and only then close.
fn close_after_reply(reading: &mut Reading, linger: Duration) {
    let _ = reading.stream.shutdown(Shutdown::Write);
    reading.until = Some(Instant::now() + linger);
    let mut sink = [0u8; 1024];
    // Each read is bounded by the time left, and none is made once `linger` has passed.
    while matches!(reading.read(&mut sink), Ok(n) if n > 0) {}
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 128 random bits from the OS, hex-encoded.
fn random_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Listen on `127.0.0.1` (random port) and forward authenticated requests to `dispatch`.
pub fn serve(dispatch: impl Dispatch) -> std::io::Result<Endpoint> {
    serve_with(dispatch, LIMITS)
}

pub(crate) fn serve_with(dispatch: impl Dispatch, limits: Limits) -> std::io::Result<Endpoint> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let token = random_token()?;
    let expected = token.clone();
    let dispatch = Arc::new(dispatch);
    let waiting = Arc::new(AtomicUsize::new(0));
    std::thread::Builder::new().name("xmlcraft-control".into()).spawn(move || {
        for stream in listener.incoming() {
            let stream = match stream {
                Ok(stream) => stream,
                Err(_) => {
                    std::thread::sleep(ACCEPT_RETRY);
                    continue;
                }
            };
            let _ = stream.set_write_timeout(Some(limits.write_timeout));
            // Admitted only while fewer than the limit wait to authenticate: refused on the accepting thread, so a refused
            // connection costs no thread at all.
            let Some(slot) = Slot::take(&waiting, limits.max_unauthenticated) else {
                let mut write = stream;
                let e = rpc_error(Value::Null, -32003, "too many connections waiting to authenticate; try again later");
                let _ = writeln!(write, "{e}").and_then(|()| write.flush());
                continue;
            };
            let (dispatch, expected) = (dispatch.clone(), expected.clone());
            let _ = std::thread::Builder::new()
                .name("xmlcraft-control-conn".into())
                .spawn(move || connection(stream, slot, &*dispatch, &expected, limits));
        }
    })?;
    Ok(Endpoint { port, token })
}

fn connection(stream: TcpStream, slot: Slot, dispatch: &dyn Dispatch, expected: &str, limits: Limits) {
    // Held until `auth` succeeds (and through a refusal's linger); given back however this thread ends.
    let mut slot = Some(slot);
    let Ok(read) = stream.try_clone() else { return };
    let mut write = stream;
    let mut authed = false;
    let mut reader = BufReader::new(Reading { stream: read, until: Some(Instant::now() + limits.auth_deadline) });
    let mut buf = Vec::new();
    loop {
        // Until `auth` succeeds, lines are capped at MAX_AUTH_LINE; afterwards at MAX_LINE.
        let max = if authed { MAX_LINE } else { MAX_AUTH_LINE };
        match read_line_limited(&mut reader, &mut buf, max) {
            Ok(LineRead::Line) => {}
            // The deadline passed before `auth`: say so and close.
            Err(e) if !authed && matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => {
                let secs = limits.auth_deadline.as_secs_f64();
                let e = rpc_error(Value::Null, -32004, &format!("not authenticated within {secs} s"));
                if writeln!(write, "{e}").and_then(|()| write.flush()).is_ok() {
                    close_after_reply(reader.get_mut(), limits.linger);
                }
                break;
            }
            // End of stream or a failed read ends the connection.
            Ok(LineRead::End) | Err(_) => break,
            Ok(LineRead::TooLong) => {
                let e = rpc_error(Value::Null, -32600, &format!("request line longer than {max} bytes"));
                if writeln!(write, "{e}").and_then(|()| write.flush()).is_ok() {
                    close_after_reply(reader.get_mut(), limits.linger);
                }
                break;
            }
        }
        // Bytes that aren't UTF-8 end the connection.
        let Ok(line) = std::str::from_utf8(&buf) else { break };
        if line.trim().is_empty() {
            continue;
        }
        let (reply, close) = match serde_json::from_str::<Value>(line) {
            // Before `auth`, like any other first request, this closes the connection.
            Err(e) => (rpc_error(Value::Null, -32700, &format!("parse error: {e}")), !authed),
            Ok(msg) => {
                let id = msg.get("id").cloned().unwrap_or(Value::Null);
                let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                if !authed {
                    let ok = method == "auth" && params.get("token").and_then(Value::as_str).is_some_and(|t| constant_time_eq(t, expected));
                    authed = ok;
                    if ok {
                        // Authenticated: no deadline from here on, and its place is free.
                        reader.get_mut().until = None;
                        let _ = reader.get_mut().stream.set_read_timeout(None);
                        drop(slot.take());
                        (json!({ "jsonrpc": "2.0", "id": id, "result": { "ok": true } }), false)
                    } else {
                        (rpc_error(id, -32001, "authenticate first: auth {token}"), true)
                    }
                } else {
                    match dispatch.dispatch(method, params).recv_timeout(limits.reply_timeout) {
                        Ok(Ok(v)) => (json!({ "jsonrpc": "2.0", "id": id, "result": v }), false),
                        Ok(Err(e)) => (rpc_error(id, -32000, &e), false),
                        Err(_) => (rpc_error(id, -32002, &format!("the app did not answer within {} s", limits.reply_timeout.as_secs_f64())), false),
                    }
                }
            }
        };
        if writeln!(write, "{reply}").and_then(|()| write.flush()).is_err() {
            break;
        }
        if close {
            close_after_reply(reader.get_mut(), limits.linger);
            break;
        }
    }
}

/// Write the control file: `{"port": …, "token": …, "pid": …}`, atomically and readable by its owner only.
pub fn write_endpoint_file(path: &Path, endpoint: &Endpoint) -> std::io::Result<()> {
    let body = json!({ "port": endpoint.port, "token": endpoint.token, "pid": std::process::id() });
    let options = craft_atomic::WriteOptions::default().with_new_file_permissions(craft_atomic::NewFilePermissions::Private);
    options.atomic_write(path, body.to_string().as_bytes())
}

/// Read a control file written by [`write_endpoint_file`].
pub fn read_endpoint_file(path: &Path) -> Result<Endpoint, String> {
    // A control file is about a hundred bytes; refuse to read a huge file someone pointed us at by mistake.
    let len = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
    if len > 4096 {
        return Err(format!("{} is not a control file ({len} bytes)", path.display()));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("{} is not a control file: {e}", path.display()))?;
    let port = v.get("port").and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok()).filter(|p| *p != 0);
    let token = v.get("token").and_then(Value::as_str).filter(|t| !t.is_empty());
    match (port, token) {
        (Some(port), Some(token)) => Ok(Endpoint { port, token: token.to_owned() }),
        _ => Err(format!("{} has no port and token", path.display())),
    }
}

/// A connection to a running app's control channel.
pub struct ControlClient {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    next_id: u64,
}

impl std::fmt::Debug for ControlClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlClient").field("next_id", &self.next_id).finish_non_exhaustive()
    }
}

impl ControlClient {
    /// Connect to the app whose control file is `file`, and authenticate.
    pub fn connect(file: &Path) -> Result<Self, String> {
        let endpoint = read_endpoint_file(file)?;
        Self::connect_to(&endpoint)
    }

    pub fn connect_to(endpoint: &Endpoint) -> Result<Self, String> {
        let stream = TcpStream::connect(("127.0.0.1", endpoint.port))
            .map_err(|e| format!("cannot reach the app on port {}: {e} (is it running with --control?)", endpoint.port))?;
        let writer = stream.try_clone().map_err(|e| e.to_string())?;
        // A reply can take a while (a screenshot waits for a frame), but not for ever.
        stream.set_read_timeout(Some(Duration::from_secs(60))).map_err(|e| e.to_string())?;
        let mut client = Self { reader: BufReader::new(stream), writer, next_id: 1 };
        client.call("auth", json!({ "token": endpoint.token })).map_err(|e| format!("the app refused the token: {e}"))?;
        Ok(client)
    }

    /// Call a method and wait for its reply.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writeln!(self.writer, "{request}").and_then(|()| self.writer.flush()).map_err(|e| format!("sending {method}: {e}"))?;
        let mut buf = Vec::new();
        match read_line_limited(&mut self.reader, &mut buf, MAX_LINE).map_err(|e| format!("waiting for the reply to {method}: {e}"))? {
            LineRead::Line => {}
            LineRead::End => return Err(format!("the app closed the connection before answering {method}")),
            LineRead::TooLong => return Err(format!("the reply to {method} is too long")),
        }
        let reply: Value = serde_json::from_slice(&buf).map_err(|e| format!("the reply to {method} is not JSON: {e}"))?;
        if let Some(error) = reply.get("error") {
            return Err(error.get("message").and_then(Value::as_str).unwrap_or("unknown error").to_owned());
        }
        reply.get("result").cloned().ok_or_else(|| format!("the reply to {method} has no result"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    /// Answers `echo` with its parameters, `fail` with an error, `silent` never.
    fn echo(method: &str, params: Value) -> Receiver<Reply> {
        let (tx, rx) = channel();
        match method {
            "echo" => {
                let _ = tx.send(Ok(json!({ "method": method, "params": params })));
            }
            "fail" => {
                let _ = tx.send(Err("it failed".to_owned()));
            }
            // Keep the sender alive without answering, so the wait times out rather than disconnecting.
            "silent" => std::mem::forget(tx),
            _ => {
                let _ = tx.send(Err(format!("unknown method {method}")));
            }
        }
        rx
    }

    fn quick() -> Limits {
        Limits {
            auth_deadline: Duration::from_millis(300),
            max_unauthenticated: 2,
            write_timeout: Duration::from_secs(5),
            linger: Duration::from_millis(100),
            reply_timeout: Duration::from_millis(300),
        }
    }

    fn raw(port: u16) -> (TcpStream, BufReader<TcpStream>) {
        let s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        s.set_read_timeout(Some(Duration::from_secs(5))).expect("timeout");
        let r = BufReader::new(s.try_clone().expect("clone"));
        (s, r)
    }

    fn next(r: &mut BufReader<TcpStream>) -> Option<Value> {
        let mut buf = Vec::new();
        match read_line_limited(r, &mut buf, 1 << 20) {
            Ok(LineRead::Line) => serde_json::from_slice(&buf).ok(),
            _ => None,
        }
    }

    #[test]
    fn an_authenticated_client_round_trips_requests_and_errors() {
        let endpoint = serve_with(echo, quick()).expect("serves");
        let mut c = ControlClient::connect_to(&endpoint).expect("connects");
        assert_eq!(c.call("echo", json!({"a": 1})).expect("echoes"), json!({"method": "echo", "params": {"a": 1}}));
        assert_eq!(c.call("fail", json!({})).expect_err("an error"), "it failed");
        assert_eq!(c.call("other", Value::Null).expect_err("unknown"), "unknown method other");
        assert!(c.call("silent", json!({})).expect_err("no answer").contains("did not answer"));
        assert!(c.call("echo", json!({})).is_ok(), "the connection survives a request that timed out");
    }

    #[test]
    fn a_wrong_token_is_refused_and_the_connection_closed() {
        let endpoint = serve_with(echo, quick()).expect("serves");
        let bad = Endpoint { port: endpoint.port, token: "0".repeat(32) };
        assert!(ControlClient::connect_to(&bad).expect_err("refused").contains("refused the token"));
        let (mut s, mut r) = raw(endpoint.port);
        writeln!(s, r#"{{"jsonrpc":"2.0","id":1,"method":"auth","params":{{"token":""}}}}"#).expect("write");
        assert_eq!(next(&mut r).expect("a reply")["error"]["code"], -32001);
        assert!(next(&mut r).is_none(), "then the connection is closed");
    }

    #[test]
    fn nothing_but_auth_is_accepted_first() {
        let endpoint = serve_with(echo, quick()).expect("serves");
        for first in [r#"{"jsonrpc":"2.0","id":1,"method":"echo"}"#, "not json", "[]", "{}"] {
            let (mut s, mut r) = raw(endpoint.port);
            writeln!(s, "{first}").expect("write");
            assert!(next(&mut r).is_some_and(|v| v.get("error").is_some()), "{first}");
            assert!(next(&mut r).is_none(), "{first}: closed");
        }
    }

    #[test]
    fn an_overlong_line_before_auth_is_refused_without_reading_it_all() {
        let endpoint = serve_with(echo, quick()).expect("serves");
        let (mut s, mut r) = raw(endpoint.port);
        let _ = s.write_all(&vec![b'x'; 200_000]);
        let reply = next(&mut r).expect("a reply");
        assert_eq!(reply["error"]["code"], -32600);
        assert!(reply["error"]["message"].as_str().is_some_and(|m| m.contains("4096")));
    }

    #[test]
    fn a_connection_that_never_authenticates_is_closed_at_the_deadline() {
        let endpoint = serve_with(echo, quick()).expect("serves");
        let (_s, mut r) = raw(endpoint.port);
        let started = Instant::now();
        assert_eq!(next(&mut r).expect("a reply")["error"]["code"], -32004);
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
        // A client feeding one byte at a time cannot extend the deadline.
        let (mut s, mut r) = raw(endpoint.port);
        let started = Instant::now();
        let drip = std::thread::spawn(move || {
            for _ in 0..40 {
                if s.write_all(b" ").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        assert_eq!(next(&mut r).expect("a reply")["error"]["code"], -32004);
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
        let _ = drip.join();
    }

    #[test]
    fn only_so_many_connections_may_wait_to_authenticate() {
        let endpoint = serve_with(echo, Limits { auth_deadline: Duration::from_secs(30), ..quick() }).expect("serves");
        let held: Vec<_> = (0..2).map(|_| raw(endpoint.port)).collect();
        std::thread::sleep(Duration::from_millis(200));
        let (_s, mut r) = raw(endpoint.port);
        assert_eq!(next(&mut r).expect("refused").pointer("/error/code"), Some(&json!(-32003)));
        // A client that authenticates gives its place back.
        drop(held);
        std::thread::sleep(Duration::from_millis(400));
        assert!(ControlClient::connect_to(&endpoint).is_ok());
    }

    #[test]
    fn long_requests_are_read_whole_after_auth() {
        let endpoint = serve_with(echo, quick()).expect("serves");
        let mut c = ControlClient::connect_to(&endpoint).expect("connects");
        let text = "é".repeat(500_000);
        let reply = c.call("echo", json!({ "text": text })).expect("echoes");
        assert_eq!(reply["params"]["text"].as_str().map(str::len), Some(1_000_000));
    }

    #[test]
    fn the_control_file_round_trips_and_is_private() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("control.json");
        let endpoint = Endpoint { port: 4321, token: "abc".into() };
        write_endpoint_file(&file, &endpoint).expect("written");
        assert_eq!(read_endpoint_file(&file).expect("read"), endpoint);
        let text = std::fs::read_to_string(&file).expect("text");
        assert!(text.contains(&format!("\"pid\":{}", std::process::id())));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode & 0o077, 0, "the token must not be readable by others, mode {mode:o}");
        }
    }

    #[test]
    fn a_bad_control_file_is_described_not_trusted() {
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("c.json");
        for (content, needle) in [
            ("", "not a control file"),
            ("{}", "no port and token"),
            (r#"{"port": 0, "token": "x"}"#, "no port"),
            (r#"{"port": 99999, "token": "x"}"#, "no port"),
            (r#"{"port": 1, "token": ""}"#, "no port"),
        ] {
            std::fs::write(&file, content).expect("write");
            assert!(read_endpoint_file(&file).expect_err(content).contains(needle), "{content}");
        }
        std::fs::write(&file, vec![b'x'; 10_000]).expect("write");
        assert!(read_endpoint_file(&file).expect_err("too big").contains("10000 bytes"));
        assert!(read_endpoint_file(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn tokens_are_long_hex_and_differ() {
        let (a, b) = (random_token().expect("token"), random_token().expect("token"));
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert!(constant_time_eq(&a, &a) && !constant_time_eq(&a, &b) && !constant_time_eq(&a, "short"));
    }

    #[test]
    fn a_dead_port_is_reported_helpfully() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);
        let err = ControlClient::connect_to(&Endpoint { port, token: "x".into() }).expect_err("no server there");
        assert!(err.contains("--control"), "{err}");
    }
}
