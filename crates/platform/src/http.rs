//! Downloading the grammars that documents name: the resolver's [`Fetcher`] over HTTP and HTTPS.
//!
//! The client is `ureq` over rustls, trusting the operating system's certificate roots (no list of roots is bundled). What a document
//! names is untrusted input, so a request is made only when all of this holds:
//!
//! - the URL is `http` or `https` and has no user name or password;
//! - the host is not this computer or a computer on the local network (see [`is_public_address`]), unless the person allowed that
//!   ([`HttpFetcher::allow_private_hosts`]); a host name is looked up first and refused if it leads there;
//! - a redirect is followed (at most [`MAX_REDIRECTS`] times) only to another `http` or `https` URL that passes the same checks, and
//!   never from `https` to `http`;
//! - the answer is at most [`FetchLimits::max_bytes`] long, and the request is over within [`TIMEOUT`];
//! - a host that could not be reached is not asked again for [`COOLDOWN`], so a document that names many resources on a dead server
//!   costs one wait, not one per resource.
//!
//! Nothing is sent but the request line and the headers `Host`, `User-Agent`, `Accept` and, when the cached copy has them,
//! `If-None-Match` and `If-Modified-Since`. There are no cookies, no credentials and no referrer.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::sync::Mutex;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use xmlcraft_resolve::{Conditions, FetchError, FetchLimits, FetchOutcome, Fetched, Fetcher, join, scheme_of};

/// The most redirects one download follows.
pub const MAX_REDIRECTS: usize = 5;
/// How long one request may take in all, from connecting to the last byte of the body.
pub const TIMEOUT: Duration = Duration::from_secs(30);
/// How long a host that could not be reached is left alone.
pub const COOLDOWN: Duration = Duration::from_secs(60);
/// How long looking up a host name may take.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether `address` is one that a document should be able to send a request to: not this computer, not the local network, not a
/// special-purpose range (the link-local range includes the metadata service of cloud hosts).
pub fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(v4: Ipv4Addr) -> bool {
    let [first, second, ..] = v4.octets();
    let shared = first == 100 && (64..128).contains(&second); // 100.64.0.0/10, carrier-grade NAT
    let this_network = first == 0; // 0.0.0.0/8
    let reserved = first >= 240; // 240.0.0.0/4, which includes the broadcast address
    !(v4.is_unspecified() || v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_multicast() || v4.is_documentation())
        && !(shared || this_network || reserved)
}

fn is_public_v6(v6: Ipv6Addr) -> bool {
    // An IPv4 address written in IPv6 is the IPv4 address (`::ffff:a.b.c.d`, and NAT64's `64:ff9b::a.b.c.d`).
    if let Some(v4) = v6.to_ipv4_mapped() {
        return is_public_v4(v4);
    }
    let [a, b, c, d, e, f, high, low] = v6.segments();
    if (a, b, c, d, e, f) == (0x64, 0xff9b, 0, 0, 0, 0) {
        let [h0, h1] = high.to_be_bytes();
        let [l0, l1] = low.to_be_bytes();
        return is_public_v4(Ipv4Addr::new(h0, h1, l0, l1));
    }
    let documentation = a == 0x2001 && b == 0x0db8;
    !(v6.is_unspecified() || v6.is_loopback() || v6.is_multicast() || v6.is_unique_local() || v6.is_unicast_link_local() || documentation)
}

/// The parts of a URL that a request is checked by.
#[derive(Debug, PartialEq, Eq)]
struct Target<'a> {
    https: bool,
    host: &'a str,
    port: u16,
}

/// The scheme, host and port of an `http` or `https` URL; why not, for anything else (another scheme, no host, a user name).
fn target_of(url: &str) -> Result<Target<'_>, String> {
    let https = match scheme_of(url).as_deref() {
        Some("https") => true,
        Some("http") => false,
        _ => return Err(format!("{url}: only http and https addresses are downloaded")),
    };
    let after = url.split_once("://").map_or("", |(_, rest)| rest);
    let authority = &after[..after.find(['/', '?', '#']).unwrap_or(after.len())];
    if authority.contains('@') {
        return Err(format!("{url}: addresses with a user name are not downloaded"));
    }
    let (host, port) = match authority.strip_prefix('[') {
        // An IPv6 address: `[::1]` or `[::1]:8080`.
        Some(rest) => {
            let (host, tail) = rest.split_once(']').ok_or_else(|| format!("{url}: the address is not understood"))?;
            (host, tail.strip_prefix(':'))
        }
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };
    let port = match port {
        None | Some("") => {
            if https {
                443
            } else {
                80
            }
        }
        Some(port) => port.parse().map_err(|_| format!("{url}: the port is not understood"))?,
    };
    if host.is_empty() {
        return Err(format!("{url}: there is no host in the address"));
    }
    Ok(Target { https, host, port })
}

/// What a server is called in the table of those that could not be reached.
fn server_of(target: &Target<'_>) -> String {
    format!("{}://{}:{}", if target.https { "https" } else { "http" }, target.host.to_ascii_lowercase(), target.port)
}

/// Whether a host name stands for this computer whatever the name servers say (`localhost` and everything below it).
fn is_local_name(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    host == "localhost" || host.ends_with(".localhost")
}

/// The first of the `addresses` a host leads to that is not on the internet.
fn first_private(addresses: &[IpAddr]) -> Option<IpAddr> {
    addresses.iter().copied().find(|address| !is_public_address(*address))
}

/// Looks the addresses of `host` up, giving up after [`LOOKUP_TIMEOUT`].
fn lookup(host: &str, port: u16) -> Result<Vec<IpAddr>, String> {
    let (sender, receiver) = mpsc::channel();
    let name = host.to_owned();
    std::thread::Builder::new()
        .name("lookup".to_owned())
        .spawn(move || {
            let found = (name.as_str(), port).to_socket_addrs().map(|found| found.map(|a| a.ip()).collect::<Vec<_>>());
            let _ = sender.send(found);
        })
        .map_err(|e| format!("{host}: the address could not be looked up ({e})"))?;
    match receiver.recv_timeout(LOOKUP_TIMEOUT) {
        Ok(Ok(addresses)) => Ok(addresses),
        Ok(Err(e)) => Err(format!("{host}: {e}")),
        Err(_) => Err(format!("{host}: looking up the address took too long")),
    }
}

/// Downloads over HTTP and HTTPS, within the rules in the module documentation.
pub struct HttpFetcher {
    agent: ureq::Agent,
    allow_private_hosts: bool,
    /// A proxy is in use: it looks the host up, so the names are not.
    proxied: bool,
    /// When a request to a server (scheme, host and port) last failed to get an answer. The server for `https` is not the one for `http`:
    /// the resolver tries one and then the other.
    failed: Mutex<HashMap<String, Instant>>,
}

impl HttpFetcher {
    /// A client that introduces itself as `user_agent`.
    pub fn new(user_agent: &str) -> Self {
        Self::with_timeout(user_agent, TIMEOUT)
    }

    /// A client whose requests take `timeout` at most.
    pub fn with_timeout(user_agent: &str, timeout: Duration) -> Self {
        let tls = ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build();
        // The proxy the environment names (HTTPS_PROXY, HTTP_PROXY, ALL_PROXY, NO_PROXY), as for ureq's default configuration.
        let proxy = ureq::Proxy::try_from_env();
        let proxied = proxy.is_some();
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .user_agent(user_agent)
            .tls_config(tls)
            .proxy(proxy)
            .timeout_global(Some(timeout))
            .timeout_connect(Some(timeout.min(Duration::from_secs(10))))
            // Every redirect is followed here, by hand, so that each one is checked.
            .max_redirects(0)
            .max_redirects_will_error(false)
            .http_status_as_error(false)
            .build()
            .into();
        Self { agent, allow_private_hosts: false, proxied, failed: Mutex::new(HashMap::new()) }
    }

    /// Whether addresses on this computer and on the local network may be downloaded from (off unless the person turns it on).
    #[must_use]
    pub fn allow_private_hosts(mut self, allow: bool) -> Self {
        self.allow_private_hosts = allow;
        self
    }

    /// Whether a request to `target` may be made now.
    fn allowed(&self, url: &str, target: &Target<'_>) -> Result<(), FetchError> {
        let refused = |why: String| Err(FetchError::Network(why));
        if !self.allow_private_hosts {
            if is_local_name(target.host) {
                return refused(format!("{url}: {} is this computer, which is not downloaded from", target.host));
            }
            match target.host.parse::<IpAddr>() {
                Ok(address) if !is_public_address(address) => {
                    return refused(format!("{url}: {} is not an address on the internet, which is all that is downloaded from", target.host));
                }
                Ok(_) => {}
                // A name: where it leads must be checked too (unless a proxy looks it up, and decides).
                Err(_) if !self.proxied => match lookup(target.host, target.port) {
                    Ok(addresses) => {
                        if let Some(address) = first_private(&addresses) {
                            return refused(format!(
                                "{url}: {} leads to {address}, which is not an address on the internet, which is all that is downloaded from",
                                target.host
                            ));
                        }
                    }
                    // Not found, or not in time: the request would not do better, and its error says so.
                    Err(why) => return refused(why),
                },
                Err(_) => {}
            }
        }
        let failed = self.failed.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match failed.get(&server_of(target)) {
            Some(when) if when.elapsed() < COOLDOWN => {
                refused(format!("{url}: {} could not be reached a moment ago; it is not asked again yet", target.host))
            }
            _ => Ok(()),
        }
    }

    fn note_failure(&self, target: &Target<'_>) {
        let mut failed = self.failed.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // The table stays small: servers that were left alone long enough are forgotten.
        failed.retain(|_, when| when.elapsed() < COOLDOWN);
        failed.insert(server_of(target), Instant::now());
    }
}

/// Where a redirect from `current` to `location` (which may be relative) leads: another `http` or `https` address, and `https` stays
/// `https`.
fn next_hop(current: &str, location: &str) -> Result<String, String> {
    let next = join(current, location.trim());
    let from = target_of(current)?;
    let to = target_of(&next)?;
    if from.https && !to.https {
        return Err(format!("{current}: the server redirected from https to {next}, which is not secure"));
    }
    Ok(next)
}

/// One header of a response as text.
fn header(response: &ureq::http::Response<ureq::Body>, name: &str) -> Option<String> {
    response.headers().get(name).and_then(|value| value.to_str().ok()).map(str::to_owned)
}

impl Fetcher for HttpFetcher {
    fn fetch(&self, url: &str, conditions: &Conditions, limits: &FetchLimits) -> Result<FetchOutcome, FetchError> {
        // The resolver puts the address before what it is told went wrong; the errors below name the address too (a redirect may have
        // led elsewhere), so the one that is `url` itself is left out.
        self.get(url, conditions, limits).map_err(|error| match error {
            FetchError::Network(why) => FetchError::Network(why.strip_prefix(&format!("{url}: ")).map_or(why.clone(), str::to_owned)),
            other => other,
        })
    }
}

impl HttpFetcher {
    fn get(&self, url: &str, conditions: &Conditions, limits: &FetchLimits) -> Result<FetchOutcome, FetchError> {
        let mut current = url.to_owned();
        for _ in 0..=MAX_REDIRECTS {
            let target = target_of(&current).map_err(FetchError::Network)?;
            self.allowed(&current, &target)?;
            let mut request = self
                .agent
                .get(&current)
                .header("Accept", "application/xml, text/xml, application/xml-dtd, application/xsd+xml, text/plain;q=0.5, */*;q=0.1");
            if let Some(etag) = &conditions.etag {
                request = request.header("If-None-Match", etag);
            }
            if let Some(date) = &conditions.last_modified {
                request = request.header("If-Modified-Since", date);
            }
            let mut response = match request.call() {
                Ok(response) => response,
                Err(error) => {
                    self.note_failure(&target);
                    return Err(FetchError::Network(format!("{error}")));
                }
            };
            let status = response.status().as_u16();
            match status {
                200 => {
                    let too_large = header(&response, "content-length")
                        .and_then(|length| length.trim().parse::<u64>().ok())
                        .is_some_and(|length| length > limits.max_bytes as u64);
                    if too_large {
                        return Err(FetchError::TooLarge);
                    }
                    // The reader fails when it is asked for more after the limit, even if the body ends there: allow one byte more than may
                    // be, so that a body of exactly the limit is read and one longer fails (reading that byte is where it does).
                    let allowed = u64::try_from(limits.max_bytes).unwrap_or(u64::MAX).saturating_add(1);
                    let body = match response.body_mut().with_config().limit(allowed).read_to_vec() {
                        Ok(body) => body,
                        Err(ureq::Error::BodyExceedsLimit(_)) => return Err(FetchError::TooLarge),
                        Err(error) => {
                            self.note_failure(&target);
                            return Err(FetchError::Network(format!("{error}")));
                        }
                    };
                    return Ok(FetchOutcome::Fresh(Fetched {
                        body,
                        final_url: current,
                        etag: header(&response, "etag"),
                        last_modified: header(&response, "last-modified"),
                    }));
                }
                304 if conditions.etag.is_some() || conditions.last_modified.is_some() => return Ok(FetchOutcome::NotModified),
                301 | 302 | 303 | 307 | 308 => {
                    let Some(location) = header(&response, "location") else {
                        return Err(FetchError::Network(format!("{current}: the server redirected without saying where to")));
                    };
                    current = next_hop(&current, &location).map_err(FetchError::Network)?;
                }
                code => return Err(FetchError::Status(code)),
            }
        }
        Err(FetchError::Network(format!("{url}: more than {MAX_REDIRECTS} redirects")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().expect("an address")
    }

    #[test]
    fn only_addresses_on_the_internet_are_public() {
        for public in ["8.8.8.8", "93.184.216.34", "2606:2800:220:1:248:1893:25c8:1946", "::ffff:8.8.8.8", "64:ff9b::808:808", "172.32.0.1"] {
            assert!(is_public_address(ip(public)), "{public}");
        }
        for private in [
            "127.0.0.1",
            "127.1.2.3",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254", // the metadata service of cloud hosts
            "100.64.0.1",
            "0.0.0.0",
            "0.1.2.3",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "192.0.2.1",
            "::1",
            "::",
            "fe80::1",
            "fd00::1",
            "fc00::1",
            "ff02::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::7f00:1",
        ] {
            assert!(!is_public_address(ip(private)), "{private}");
        }
    }

    #[test]
    fn a_url_is_taken_apart_for_the_checks() {
        let target = |url| target_of(url).map(|t| (t.https, t.host.to_owned(), t.port));
        assert_eq!(target("https://www.w3.org/2001/xml.xsd"), Ok((true, "www.w3.org".into(), 443)));
        assert_eq!(target("http://example.com/a?b#c"), Ok((false, "example.com".into(), 80)));
        assert_eq!(target("http://example.com:8080/x"), Ok((false, "example.com".into(), 8080)));
        assert_eq!(target("https://example.com?x=1"), Ok((true, "example.com".into(), 443)));
        assert_eq!(target("http://[::1]/x"), Ok((false, "::1".into(), 80)));
        assert_eq!(target("http://[2001:db8::1]:81/x"), Ok((false, "2001:db8::1".into(), 81)));
        assert_eq!(target("HTTP://Example.COM/x"), Ok((false, "Example.COM".into(), 80)));
        for refused in [
            "ftp://example.com/x",
            "file:///etc/passwd",
            "example.com/x",
            "http://",
            "http:///x",
            "http://user:pw@example.com/x",
            "http://user@example.com/x",
            "http://example.com:notaport/x",
            "http://example.com:99999/x",
            "http://[::1/x",
        ] {
            assert!(target(refused).is_err(), "{refused}");
        }
        // The address after a user name is not the host: `http://127.0.0.1@evil/` and `http://evil@127.0.0.1/` are both refused.
        assert!(target("http://good.example@127.0.0.1/").is_err());
    }

    #[test]
    fn a_redirect_leads_to_another_web_address_and_never_from_https_to_http() {
        assert_eq!(next_hop("http://a.example/x/y.xsd", "z.xsd"), Ok("http://a.example/x/z.xsd".to_owned()));
        assert_eq!(next_hop("https://a.example/x/y.xsd", "/z.xsd"), Ok("https://a.example/z.xsd".to_owned()));
        assert_eq!(next_hop("http://a.example/y.xsd", " https://b.example/y.xsd "), Ok("https://b.example/y.xsd".to_owned()));
        assert_eq!(next_hop("https://a.example/y.xsd", "https://b.example/y.xsd"), Ok("https://b.example/y.xsd".to_owned()));
        // Down to http, to a file, to another scheme, to an address with a user name.
        assert!(next_hop("https://a.example/y.xsd", "http://b.example/y.xsd").is_err());
        assert!(next_hop("https://a.example/y.xsd", "http://a.example/y.xsd").is_err());
        assert!(next_hop("http://a.example/y.xsd", "file:///etc/passwd").is_err());
        assert!(next_hop("http://a.example/y.xsd", "ftp://b.example/y.xsd").is_err());
        assert!(next_hop("http://a.example/y.xsd", "http://u:p@b.example/y.xsd").is_err());
        // No place at all.
        assert!(next_hop("http://a.example/y.xsd", "").is_err());
        assert!(next_hop("http://a.example/y.xsd", "  ").is_err());
    }

    #[test]
    fn a_name_that_leads_even_once_to_this_computer_is_refused() {
        assert_eq!(first_private(&[]), None);
        assert_eq!(first_private(&[ip("93.184.216.34"), ip("2606:2800:220:1::1")]), None);
        assert_eq!(first_private(&[ip("93.184.216.34"), ip("10.0.0.7")]), Some(ip("10.0.0.7")));
        assert_eq!(first_private(&[ip("::1")]), Some(ip("::1")));
    }

    #[test]
    fn this_computer_goes_by_a_few_names() {
        for local in ["localhost", "LOCALHOST", "localhost.", "app.localhost", "a.b.localhost."] {
            assert!(is_local_name(local), "{local}");
        }
        for other in ["example.com", "notlocalhost", "localhost.example.com", "localhostx"] {
            assert!(!is_local_name(other), "{other}");
        }
    }

    #[test]
    fn a_request_to_this_computer_is_refused_before_it_is_made() {
        let fetcher = HttpFetcher::new("test");
        let limits = FetchLimits { max_bytes: 1000 };
        for (url, reason) in [
            ("http://127.0.0.1:1/x.xsd", "not an address on the internet"),
            ("http://localhost:1/x.xsd", "this computer"),
            ("https://[::1]/x.xsd", "not an address on the internet"),
            ("http://169.254.169.254/latest/meta-data/", "not an address on the internet"),
            ("http://192.168.0.1/", "not an address on the internet"),
            ("ftp://example.com/x.xsd", "only http and https"),
            ("http://user:pw@example.com/x.xsd", "user name"),
        ] {
            match fetcher.fetch(url, &Conditions::default(), &limits) {
                // The error does not repeat the address: the resolver says which it was.
                Err(FetchError::Network(why)) => assert!(why.contains(reason) && !why.starts_with(url), "{url}: {why}"),
                other => panic!("{url}: {other:?}"),
            }
        }
    }
}
