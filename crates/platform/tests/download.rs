//! Downloading over the loopback interface: the client against a small server that says what it is told to. Nothing here leaves this
//! computer (the client is told it may download from it; by default it may not).
#![cfg(not(target_arch = "wasm32"))]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use xmlcraft_platform::disk_cache::DiskCache;
use xmlcraft_platform::http::HttpFetcher;
use xmlcraft_platform::network::Network;
use xmlcraft_resolve::{Budget, Clock, Conditions, FetchError, FetchLimits, FetchOutcome, Fetcher, NoBuffers, Origin, Request, ResourceCache, Stage};

/// What a request looked like.
#[derive(Clone, Debug)]
struct Seen {
    path: String,
    /// Header names in lower case.
    headers: Vec<(String, String)>,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
}

/// What the server answers.
struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
    /// Wait this long before answering.
    delay: Duration,
    /// Say how long the body is (a server may leave that to the end of the connection).
    length: bool,
}

impl Reply {
    fn ok(body: &str) -> Self {
        Self { status: 200, headers: Vec::new(), body: body.as_bytes().to_vec(), delay: Duration::ZERO, length: true }
    }

    fn status(status: u16) -> Self {
        Self { status, ..Self::ok("") }
    }

    fn redirect(status: u16, to: &str) -> Self {
        Self { headers: vec![("Location", to.to_owned())], ..Self::status(status) }
    }

    fn header(mut self, name: &'static str, value: &str) -> Self {
        self.headers.push((name, value.to_owned()));
        self
    }

    fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

struct Server {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    fn start(handler: impl Fn(&Seen) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds");
        listener.set_nonblocking(true).expect("non-blocking");
        let port = listener.local_addr().expect("address").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let handler = Arc::new(handler);
        let thread = {
            let (seen, stop) = (seen.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let (seen, handler) = (seen.clone(), handler.clone());
                            std::thread::spawn(move || answer(stream, &seen, &*handler));
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
            })
        };
        Self { port, seen, stop, thread: Some(thread) }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().expect("lock").clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn answer(mut stream: TcpStream, seen: &Mutex<Vec<Seen>>, handler: &dyn Fn(&Seen) -> Reply) {
    // A socket accepted from a non-blocking listener may be non-blocking itself.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        // Something that is not a request (the start of a TLS handshake) gets no answer.
        if head.len() >= 4 && !head.starts_with(b"GET ") {
            return;
        }
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => head.extend_from_slice(&chunk[..n]),
        }
    }
    if !head.starts_with(b"GET ") {
        return;
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    let mut lines = text.split("\r\n");
    let path = lines.next().unwrap_or("").split(' ').nth(1).unwrap_or("").to_owned();
    let headers = lines
        .take_while(|line| !line.is_empty())
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let request = Seen { path, headers };
    seen.lock().expect("lock").push(request.clone());
    let reply = handler(&request);
    std::thread::sleep(reply.delay);
    let mut out = format!("HTTP/1.1 {} Reply\r\nConnection: close\r\n", reply.status);
    if reply.length {
        out.push_str(&format!("Content-Length: {}\r\n", reply.body.len()));
    }
    for (name, value) in &reply.headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str("\r\n");
    let _ = stream.write_all(out.as_bytes());
    let _ = stream.write_all(&reply.body);
    let _ = stream.flush();
}

fn fetcher() -> HttpFetcher {
    HttpFetcher::new("XmlCraft-test/1").allow_private_hosts(true)
}

const LIMITS: FetchLimits = FetchLimits { max_bytes: 1000 };

fn fetch(fetcher: &HttpFetcher, url: &str) -> Result<FetchOutcome, FetchError> {
    fetcher.fetch(url, &Conditions::default(), &LIMITS)
}

fn fresh(outcome: Result<FetchOutcome, FetchError>) -> xmlcraft_resolve::Fetched {
    match outcome {
        Ok(FetchOutcome::Fresh(fetched)) => fetched,
        other => panic!("expected a download, got {other:?}"),
    }
}

#[test]
fn a_download_gives_the_body_where_it_came_from_and_what_the_server_said_about_it() {
    let server = Server::start(|_| Reply::ok("<xs:schema/>").header("ETag", "\"v1\"").header("Last-Modified", "Tue, 15 Nov 1994 12:45:26 GMT"));
    let got = fresh(fetch(&fetcher(), &server.url("/a.xsd")));
    assert_eq!(got.body, b"<xs:schema/>");
    assert_eq!(got.final_url, server.url("/a.xsd"));
    assert_eq!(got.etag.as_deref(), Some("\"v1\""));
    assert_eq!(got.last_modified.as_deref(), Some("Tue, 15 Nov 1994 12:45:26 GMT"));
    // What was sent: who asks, what for, and nothing about the person.
    let seen = server.requests();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].path, "/a.xsd");
    assert_eq!(seen[0].header("user-agent"), Some("XmlCraft-test/1"));
    assert!(seen[0].header("accept").is_some_and(|a| a.contains("xml")));
    for never in ["cookie", "referer", "authorization", "if-none-match", "if-modified-since"] {
        assert_eq!(seen[0].header(never), None, "{never}");
    }
}

#[test]
fn a_copy_the_server_says_is_current_is_not_downloaded_again() {
    let server = Server::start(|seen| match seen.header("if-none-match") {
        Some("\"v1\"") => Reply::status(304),
        _ => Reply::ok("new"),
    });
    let url = server.url("/a.xsd");
    let known = Conditions { etag: Some("\"v1\"".to_owned()), last_modified: Some("Tue, 15 Nov 1994 12:45:26 GMT".to_owned()) };
    assert_eq!(fetcher().fetch(&url, &known, &LIMITS), Ok(FetchOutcome::NotModified));
    let seen = server.requests();
    assert_eq!(seen[0].header("if-none-match"), Some("\"v1\""));
    assert_eq!(seen[0].header("if-modified-since"), Some("Tue, 15 Nov 1994 12:45:26 GMT"));
    // A copy that has changed is downloaded.
    let changed = Conditions { etag: Some("\"v0\"".to_owned()), last_modified: None };
    assert_eq!(fresh(fetcher().fetch(&url, &changed, &LIMITS)).body, b"new");
    // "Not modified" to a question that was not conditional is an error, not a copy.
    let always = Server::start(|_| Reply::status(304));
    assert_eq!(fetch(&fetcher(), &always.url("/x")), Err(FetchError::Status(304)));
}

#[test]
fn redirects_are_followed_a_few_times_and_the_answer_says_where_it_ended() {
    let server = Server::start(|seen| match seen.path.as_str() {
        "/old.xsd" => Reply::redirect(301, "/dir/new.xsd"),
        "/dir/new.xsd" => Reply::redirect(307, "final.xsd"),
        "/dir/final.xsd" => Reply::ok("end"),
        "/loop" => Reply::redirect(302, "/loop"),
        "/nowhere" => Reply::status(302),
        _ => Reply::status(404),
    });
    let got = fresh(fetch(&fetcher(), &server.url("/old.xsd")));
    assert_eq!((got.body.as_slice(), got.final_url.as_str()), (b"end".as_slice(), server.url("/dir/final.xsd").as_str()));
    // A loop ends.
    let before = server.requests().len();
    match fetch(&fetcher(), &server.url("/loop")) {
        Err(FetchError::Network(why)) => assert!(why.contains("redirects"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(server.requests().len() - before, 6, "the first request and five redirects");
    // A redirect that does not say where.
    assert!(matches!(fetch(&fetcher(), &server.url("/nowhere")), Err(FetchError::Network(_))));
}

#[test]
fn errors_say_what_the_server_answered() {
    let server = Server::start(|seen| match seen.path.as_str() {
        "/gone" => Reply::status(404),
        "/broken" => Reply::status(500),
        "/forbidden" => Reply::status(403),
        _ => Reply::status(204),
    });
    for (path, status) in [("/gone", 404), ("/broken", 500), ("/forbidden", 403), ("/empty", 204)] {
        assert_eq!(fetch(&fetcher(), &server.url(path)), Err(FetchError::Status(status)), "{path}");
    }
    // An error is the server's answer, not a server that cannot be reached: the next question is asked (and answered the same).
    assert_eq!(fetch(&fetcher(), &server.url("/gone")), Err(FetchError::Status(404)));
}

#[test]
fn a_body_over_the_limit_is_refused_whether_or_not_the_server_says_how_long_it_is() {
    let big = "x".repeat(LIMITS.max_bytes + 1);
    let exact = "y".repeat(LIMITS.max_bytes);
    let server = Server::start(move |seen| match seen.path.as_str() {
        "/declared" => Reply::ok(&big),
        "/undeclared" => Reply { length: false, ..Reply::ok(&big) },
        "/exact" => Reply::ok(&exact),
        "/exact-undeclared" => Reply { length: false, ..Reply::ok(&exact) },
        _ => Reply::status(404),
    });
    assert_eq!(fetch(&fetcher(), &server.url("/declared")), Err(FetchError::TooLarge));
    assert_eq!(fetch(&fetcher(), &server.url("/undeclared")), Err(FetchError::TooLarge));
    assert_eq!(fresh(fetch(&fetcher(), &server.url("/exact"))).body.len(), LIMITS.max_bytes);
    assert_eq!(fresh(fetch(&fetcher(), &server.url("/exact-undeclared"))).body.len(), LIMITS.max_bytes);
}

#[test]
fn a_server_that_is_too_slow_is_given_up_on_and_then_left_alone_for_a_while() {
    let server = Server::start(|seen| match seen.path.as_str() {
        "/slow" => Reply::ok("late").after(Duration::from_millis(1500)),
        _ => Reply::ok("quick"),
    });
    let client = HttpFetcher::with_timeout("XmlCraft-test/1", Duration::from_millis(300)).allow_private_hosts(true);
    let started = Instant::now();
    let first = fetch(&client, &server.url("/slow"));
    assert!(matches!(first, Err(FetchError::Network(_))), "{first:?}");
    assert!(started.elapsed() < Duration::from_millis(1200), "it waited {:?}", started.elapsed());
    // The next question to that host is not even asked: a document with a hundred references to a dead server waits once.
    let asked = server.requests().len();
    match fetch(&client, &server.url("/quick")) {
        Err(FetchError::Network(why)) => assert!(why.contains("not asked again"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(server.requests().len(), asked);
    // Another client has not been told.
    assert_eq!(fresh(fetch(&fetcher(), &server.url("/quick"))).body, b"quick");
}

#[test]
fn a_server_that_is_not_there_is_an_error_and_so_is_https_with_a_server_that_does_not_speak_it() {
    let port = TcpListener::bind("127.0.0.1:0").expect("binds").local_addr().expect("address").port();
    // (The listener is gone: nothing answers on that port.)
    match fetch(&fetcher(), &format!("http://127.0.0.1:{port}/a.xsd")) {
        Err(FetchError::Network(_)) => {}
        other => panic!("{other:?}"),
    }
    // The TLS handshake runs against a plain server: an error, and not a panic for want of a crypto provider.
    let plain = Server::start(|_| Reply::ok("plain"));
    match fetch(&fetcher(), &format!("https://127.0.0.1:{}/a.xsd", plain.port)) {
        Err(FetchError::Network(_)) => {}
        other => panic!("{other:?}"),
    }
    assert!(plain.requests().is_empty(), "nobody asked for a page");
}

#[test]
fn by_default_nothing_on_this_computer_is_downloaded_from() {
    let server = Server::start(|_| Reply::ok("secret"));
    let default = HttpFetcher::new("XmlCraft-test/1");
    match fetch(&default, &server.url("/a.xsd")) {
        Err(FetchError::Network(why)) => assert!(why.contains("not an address on the internet"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(server.requests().is_empty(), "no request was made");
}

/// A clock the test moves.
struct Clockwork(AtomicI64);

impl Clock for Clockwork {
    fn now(&self) -> i64 {
        self.0.load(Ordering::Relaxed)
    }
}

#[test]
fn the_resolver_downloads_once_keeps_the_copy_on_disk_and_asks_the_server_whether_it_is_still_current() {
    let server = Server::start(|seen| match (seen.path.as_str(), seen.header("if-none-match")) {
        ("/s.xsd", Some("\"v1\"")) => Reply::status(304),
        ("/s.xsd", _) => Reply::ok("<xs:schema/>").header("ETag", "\"v1\""),
        _ => Reply::status(404),
    });
    let dir = tempfile::tempdir().expect("temp dir");
    let clock = Arc::new(Clockwork(AtomicI64::new(1_000_000)));
    let network = || {
        let cache = Arc::new(DiskCache::new(dir.path().join("cache"))) as Arc<dyn ResourceCache>;
        Network::new(Some(Arc::new(fetcher())), Some(cache), clock.clone())
    };
    let url = server.url("/s.xsd");
    let resolve = |network: &Network| {
        let resolver = network.resolver(Arc::new(NoBuffers));
        resolver.resolve(&Request::uri(&url, ""), &mut Budget::new())
    };
    // The first time: downloaded (https was tried first and the server does not speak it; the address as written was then asked).
    let first = resolve(&network()).expect("downloaded");
    assert_eq!((first.bytes.as_slice(), first.origin), (b"<xs:schema/>".as_slice(), Origin::Network));
    assert!(first.trail.of(Stage::Network).count() >= 2, "https first, then the address as written:\n{}", first.trail);
    assert_eq!(server.requests().len(), 1);
    // Another run (a new cache object over the same folder), a day later: from the disk, no question asked.
    clock.0.store(1_000_000 + 24 * 3600, Ordering::Relaxed);
    let second = resolve(&network()).expect("from the cache");
    assert_eq!((second.bytes.as_slice(), second.origin), (b"<xs:schema/>".as_slice(), Origin::Cache { stale: false }));
    assert_eq!(server.requests().len(), 1);
    // A year later: the server is asked whether the copy is current, says it is, and the copy is current for another month.
    clock.0.store(1_000_000 + 365 * 24 * 3600, Ordering::Relaxed);
    let third = resolve(&network()).expect("renewed");
    assert_eq!((third.bytes.as_slice(), third.origin), (b"<xs:schema/>".as_slice(), Origin::Cache { stale: false }));
    let seen = server.requests();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1].header("if-none-match"), Some("\"v1\""));
    let fourth = resolve(&network()).expect("from the cache");
    assert_eq!(fourth.origin, Origin::Cache { stale: false });
    assert_eq!(server.requests().len(), 2);
    // Offline, whatever the age: the copy that is there is used, and one that is not there is not asked for.
    clock.0.store(1_000_000 + 10 * 365 * 24 * 3600, Ordering::Relaxed);
    let offline = network();
    offline.set_offline(true);
    let stale = resolve(&offline).expect("served stale");
    assert_eq!(stale.origin, Origin::Cache { stale: true });
    let other = server.url("/other.xsd");
    let missing = offline.resolver(Arc::new(NoBuffers)).resolve(&Request::uri(&other, ""), &mut Budget::new());
    assert!(missing.is_err());
    assert_eq!(server.requests().len(), 2, "offline asks nobody");
}

#[test]
fn a_server_that_goes_away_leaves_the_old_copy_in_use() {
    let server = Server::start(|_| Reply::ok("<xs:schema/>"));
    let dir = tempfile::tempdir().expect("temp dir");
    let clock = Arc::new(Clockwork(AtomicI64::new(1_000_000)));
    let cache = Arc::new(DiskCache::new(dir.path())) as Arc<dyn ResourceCache>;
    let network = Network::new(Some(Arc::new(fetcher())), Some(cache), clock.clone());
    let url = server.url("/s.xsd");
    let resolve = || network.resolver(Arc::new(NoBuffers)).resolve(&Request::uri(&url, ""), &mut Budget::new());
    assert_eq!(resolve().expect("downloaded").origin, Origin::Network);
    drop(server);
    clock.0.store(1_000_000 + 365 * 24 * 3600, Ordering::Relaxed);
    let found = resolve().expect("the old copy");
    assert_eq!((found.bytes.as_slice(), found.origin), (b"<xs:schema/>".as_slice(), Origin::Cache { stale: true }));
}
