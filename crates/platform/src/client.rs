//! Sending one request a person asked for (a SOAP call, a REST request) and reading the whole answer.
//!
//! This is not [`crate::http::HttpFetcher`], which downloads what documents name and refuses almost everything. Here the person chose the
//! address, the method, the headers and the body, so they are sent as given (no cookies, no credentials of its own, no referrer). What
//! is still checked, because an address can lead somewhere a person did not mean (a name that resolves to this computer or to the
//! local network):
//!
//! - the URL is `http` or `https` and has no user name or password;
//! - a host on this computer or the local network is refused unless the request says [`Request::allow_private_hosts`] (a test server on
//!   `localhost`, a service in the office: the person turns that on for the request);
//! - redirects are **not** followed: the answer says where to, and the person decides (a redirect would resend a body to another host);
//! - the answer is at most [`Request::max_response_bytes`] long and the whole request is over within [`Request::timeout`].

use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use std::net::IpAddr;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

#[cfg(not(target_arch = "wasm32"))]
use crate::http::{first_private, is_local_name, lookup, target_of};

/// The most body a request may send.
pub const MAX_REQUEST_BYTES: usize = 64 * 1024 * 1024;

/// A request.
#[derive(Clone, Debug)]
pub struct Request {
    /// `GET`, `POST`, `PUT`, `PATCH`, `DELETE`, `HEAD` or `OPTIONS`.
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Whether this computer and the local network may be sent to (off unless the person turns it on for this request).
    pub allow_private_hosts: bool,
    pub timeout: Duration,
    pub max_response_bytes: usize,
}

impl Request {
    pub fn new(method: &str, url: &str) -> Self {
        Self {
            method: method.to_owned(),
            url: url.to_owned(),
            headers: Vec::new(),
            body: Vec::new(),
            allow_private_hosts: false,
            timeout: Duration::from_secs(30),
            max_response_bytes: 16 * 1024 * 1024,
        }
    }
}

/// The answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// How long it took, from the request to the last byte.
    pub elapsed: Duration,
}

impl Response {
    /// The first header called `name` (in any case).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// Headers a caller does not set: the client frames the message itself.
const FRAMING: &[&str] = &["host", "content-length", "transfer-encoding", "connection", "upgrade", "te", "trailer"];

#[cfg(not(target_arch = "wasm32"))]
/// Whether the request may go where it says (see the module documentation).
fn check(request: &Request) -> Result<(), String> {
    let target = target_of(&request.url)?;
    if request.allow_private_hosts {
        return Ok(());
    }
    if is_local_name(target.host) {
        return Err(format!("{}: {} is this computer; allow private hosts to send there", request.url, target.host));
    }
    match target.host.parse::<IpAddr>() {
        Ok(address) if !is_public_address_for_client(address) => {
            Err(format!("{}: {} is not an address on the internet; allow private hosts to send there", request.url, target.host))
        }
        Ok(_) => Ok(()),
        Err(_) => {
            let addresses = lookup(target.host, target.port)?;
            match first_private(&addresses) {
                Some(address) => Err(format!(
                    "{}: {} leads to {address}, which is not an address on the internet; allow private hosts to send there",
                    request.url, target.host
                )),
                None => Ok(()),
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn is_public_address_for_client(address: IpAddr) -> bool {
    crate::http::is_public_address(address)
}

/// Send `request` and read the answer. The error says in words what went wrong.
#[cfg(not(target_arch = "wasm32"))]
pub fn send(user_agent: &str, request: &Request) -> Result<Response, String> {
    let method = request.method.trim().to_ascii_uppercase();
    if !matches!(method.as_str(), "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS") {
        return Err(format!("the method {method} is not sent (GET, POST, PUT, PATCH, DELETE, HEAD and OPTIONS are)"));
    }
    if request.body.len() > MAX_REQUEST_BYTES {
        return Err(format!("the body is larger than {} MB", MAX_REQUEST_BYTES / (1024 * 1024)));
    }
    check(request)?;
    let started = Instant::now();
    let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
    let proxy = ureq::Proxy::try_from_env();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(user_agent)
        .tls_config(tls)
        .proxy(proxy)
        .timeout_global(Some(request.timeout))
        .timeout_connect(Some(request.timeout.min(Duration::from_secs(10))))
        .max_redirects(0)
        .max_redirects_will_error(false)
        .http_status_as_error(false)
        .build()
        .into();
    let mut builder = ureq::http::Request::builder().method(method.as_str()).uri(request.url.as_str());
    for (name, value) in &request.headers {
        if FRAMING.contains(&name.trim().to_ascii_lowercase().as_str()) {
            continue;
        }
        builder = builder.header(name.trim(), value.as_str());
    }
    let built = builder.body(request.body.clone()).map_err(|e| format!("the request is not valid: {e}"))?;
    let mut response = agent.run(built).map_err(|e| format!("{}: {e}", request.url))?;
    let status = response.status();
    let headers: Vec<(String, String)> =
        response.headers().iter().filter_map(|(n, v)| Some((n.as_str().to_owned(), v.to_str().ok()?.to_owned()))).collect();
    let allowed = u64::try_from(request.max_response_bytes).unwrap_or(u64::MAX).saturating_add(1);
    let body = match response.body_mut().with_config().limit(allowed).read_to_vec() {
        Ok(body) => body,
        Err(ureq::Error::BodyExceedsLimit(_)) => {
            return Err(format!("{}: the answer is larger than {} MB", request.url, request.max_response_bytes / (1024 * 1024)));
        }
        Err(e) => return Err(format!("{}: {e}", request.url)),
    };
    if body.len() > request.max_response_bytes {
        return Err(format!("{}: the answer is larger than {} MB", request.url, request.max_response_bytes / (1024 * 1024)));
    }
    Ok(Response {
        status: status.as_u16(),
        reason: status.canonical_reason().unwrap_or_default().to_owned(),
        headers,
        body,
        elapsed: started.elapsed(),
    })
}

/// On the web nothing is sent: the browser build has no sockets.
#[cfg(target_arch = "wasm32")]
pub fn send(_user_agent: &str, _request: &Request) -> Result<Response, String> {
    Err("requests are not sent from the web build".to_owned())
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use super::*;

    /// A server on this computer that answers one request with `answer` and hands back what it was sent.
    fn serve(answer: &'static str) -> (String, std::thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
        let url = format!("http://{}/service", listener.local_addr().expect("address"));
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accepts");
            let mut received = Vec::new();
            let mut buffer = [0u8; 4096];
            // Read until the headers are in and as many body bytes as Content-Length says.
            loop {
                let n = stream.read(&mut buffer).expect("reads");
                if n == 0 {
                    break;
                }
                received.extend_from_slice(&buffer[..n]);
                let text = String::from_utf8_lossy(&received).into_owned();
                if let Some(split) = text.find("\r\n\r\n") {
                    let length = text
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()))
                        .unwrap_or(0);
                    if received.len() >= split + 4 + length {
                        break;
                    }
                }
            }
            stream.write_all(answer.as_bytes()).expect("writes");
            received
        });
        (url, handle)
    }

    #[test]
    fn a_post_is_sent_with_its_headers_and_body_and_the_whole_answer_comes_back() {
        let (url, server) = serve(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/xml\r\nX-Test: yes\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
        );
        let mut request = Request::new("post", &url);
        request.allow_private_hosts = true;
        request.headers = vec![
            ("Content-Type".into(), "text/xml; charset=utf-8".into()),
            ("SOAPAction".into(), "\"urn:x\"".into()),
            ("Host".into(), "evil.example".into()),
        ];
        request.body = b"<a/>".to_vec();
        let response = send("xmlcraft-test", &request).expect("answered");
        assert_eq!((response.status, response.reason.as_str(), response.body.as_slice()), (500, "Internal Server Error", &b"hello"[..]));
        assert_eq!((response.header("content-type"), response.header("X-TEST")), (Some("text/xml"), Some("yes")));
        let sent = String::from_utf8(server.join().expect("server")).expect("utf-8");
        let lower = sent.to_ascii_lowercase();
        assert!(sent.starts_with("POST /service HTTP/1.1"), "{sent}");
        assert!(
            lower.contains("content-type: text/xml; charset=utf-8") && lower.contains("soapaction: \"urn:x\"") && sent.ends_with("<a/>"),
            "{sent}"
        );
        assert!(!lower.contains("evil.example"), "the caller does not set Host: {sent}");
        assert!(lower.contains("user-agent: xmlcraft-test"), "{sent}");
    }

    #[test]
    fn a_redirect_is_returned_not_followed() {
        let (url, server) = serve("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let mut request = Request::new("POST", &url);
        request.allow_private_hosts = true;
        request.body = b"x".to_vec();
        let response = send("t", &request).expect("answered");
        assert_eq!((response.status, response.header("location")), (302, Some("http://127.0.0.1:1/elsewhere")));
        server.join().expect("server");
    }

    #[test]
    fn this_computer_is_refused_unless_the_request_allows_it_and_so_are_odd_methods_and_addresses() {
        for url in ["http://127.0.0.1:9/x", "http://localhost:9/x", "http://[::1]:9/x", "http://10.0.0.1/x", "http://169.254.169.254/latest"] {
            let e = send("t", &Request::new("POST", url)).expect_err(url);
            assert!(e.contains("allow private hosts"), "{url}: {e}");
        }
        assert!(send("t", &Request::new("TRACE", "http://example.com/")).expect_err("method").contains("not sent"));
        assert!(send("t", &Request::new("GET", "ftp://example.com/")).is_err());
        assert!(send("t", &Request::new("GET", "http://user:pw@example.com/")).is_err());
        let mut big = Request::new("POST", "http://example.com/");
        big.body = vec![0; MAX_REQUEST_BYTES + 1];
        assert!(send("t", &big).expect_err("big").contains("larger"));
    }

    #[test]
    fn an_answer_larger_than_the_limit_is_an_error_and_a_closed_port_says_so() {
        let (url, server) = serve("HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\n0123456789");
        let mut request = Request::new("GET", &url);
        request.allow_private_hosts = true;
        request.max_response_bytes = 4;
        assert!(send("t", &request).expect_err("too large").contains("larger"));
        server.join().expect("server");
        let mut closed = Request::new("GET", "http://127.0.0.1:1/");
        closed.allow_private_hosts = true;
        closed.timeout = Duration::from_secs(5);
        assert!(send("t", &closed).is_err());
    }
}
