//! What the resolvers of the app and the tools download with: the client, the cache on disk, the clock and the person's choice to work
//! offline, in one value that is cloned freely and shared by the background workers.
//!
//! - [`Network::standard`] is the real thing: [`HttpFetcher`], [`DiskCache`] and the system clock. `XMLCRAFT_OFFLINE=1` starts it
//!   offline, and `XMLCRAFT_ALLOW_PRIVATE_HOSTS=1` lets it download from addresses on the local network (an intranet's schema server).
//! - [`Network::offline`] downloads nothing and keeps nothing: what tests and a build with no network use.
//! - [`Network::new`] takes any fetcher and cache, for tests with a mock.
//!
//! Offline is a switch the person can turn at any time ([`Network::set_offline`], Tools ▸ Work Offline): downloading stops, and the
//! copies already in the cache are used even when they are older than they should be. `XMLCRAFT_OFFLINE` keeps it off whatever the
//! switch says ([`Network::offline_by_environment`]), and is not something the app remembers.

#[cfg(any(test, not(target_arch = "wasm32")))]
use std::ffi::OsString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use xmlcraft_resolve::{Buffers, Clock, Environment, Fetcher, FileAccess, Policy, Resolver, ResourceCache};

#[cfg(not(target_arch = "wasm32"))]
use crate::disk_cache::{DiskCache, default_cache_dir};
#[cfg(not(target_arch = "wasm32"))]
use crate::http::HttpFetcher;
use crate::schema_files::LocalFiles;

/// The time, from the system clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
    }
}

/// Whether an environment variable is switched on: `1`, `true`, `yes` or `on`, in any case.
#[cfg(any(test, not(target_arch = "wasm32")))]
fn is_on(value: Option<OsString>) -> bool {
    value.and_then(|v| v.into_string().ok()).is_some_and(|v| ["1", "true", "yes", "on"].contains(&v.trim().to_ascii_lowercase().as_str()))
}

struct Inner {
    fetcher: Option<Arc<dyn Fetcher>>,
    cache: Option<Arc<dyn ResourceCache>>,
    clock: Arc<dyn Clock>,
    /// The switch.
    offline: AtomicBool,
    /// `XMLCRAFT_OFFLINE` was on when the network was made.
    forced: bool,
    /// The catalogs the person named (locations), consulted before the ones that ship with the app.
    user_catalogs: Mutex<Vec<(String, String)>>,
}

/// How a download is made, how long a copy is kept, and whether it is allowed now. Clones share the offline switch.
#[derive(Clone)]
pub struct Network {
    inner: Arc<Inner>,
}

impl Network {
    /// The real client and the cache on disk (none if there is no place for it), online unless `XMLCRAFT_OFFLINE` is on. (On the web
    /// there is no client: it is [`Network::offline`].)
    #[cfg(not(target_arch = "wasm32"))]
    pub fn standard() -> Self {
        let agent = format!("XmlCraft/{} (+https://github.com/thorstenhirsch/xmlcraft)", env!("CARGO_PKG_VERSION"));
        let fetcher = HttpFetcher::new(&agent).allow_private_hosts(is_on(std::env::var_os("XMLCRAFT_ALLOW_PRIVATE_HOSTS")));
        let cache = default_cache_dir().map(|dir| Arc::new(DiskCache::new(dir)) as Arc<dyn ResourceCache>);
        Self::build(Some(Arc::new(fetcher)), cache, Arc::new(SystemClock), is_on(std::env::var_os("XMLCRAFT_OFFLINE")))
    }

    #[cfg(target_arch = "wasm32")]
    pub fn standard() -> Self {
        Self::offline()
    }

    /// Nothing is downloaded and nothing is kept.
    pub fn offline() -> Self {
        Self::new(None, None, Arc::new(SystemClock))
    }

    /// A network made of the parts given, online.
    pub fn new(fetcher: Option<Arc<dyn Fetcher>>, cache: Option<Arc<dyn ResourceCache>>, clock: Arc<dyn Clock>) -> Self {
        Self::build(fetcher, cache, clock, false)
    }

    fn build(fetcher: Option<Arc<dyn Fetcher>>, cache: Option<Arc<dyn ResourceCache>>, clock: Arc<dyn Clock>, forced: bool) -> Self {
        Self { inner: Arc::new(Inner { fetcher, cache, clock, offline: AtomicBool::new(false), forced, user_catalogs: Mutex::new(Vec::new()) }) }
    }

    /// Work offline (or stop doing so): seen by every clone, and by the resolvers made from now on.
    pub fn set_offline(&self, offline: bool) {
        self.inner.offline.store(offline, Ordering::Relaxed);
    }

    /// Whether nothing is downloaded now: the switch is on, `XMLCRAFT_OFFLINE` is, or there is no client.
    pub fn is_offline(&self) -> bool {
        self.inner.forced || !self.can_download() || self.inner.offline.load(Ordering::Relaxed)
    }

    /// Use these catalogs (paths of OASIS catalog files) from now on, before the bundled ones: seen by the resolvers made after this. What
    /// a catalog names with a relative location is found next to the catalog.
    pub fn set_user_catalogs(&self, catalogs: Vec<String>) {
        let both = catalogs
            .into_iter()
            .map(|path| {
                // A relative path means the folder the program was started in, now: not the root of the file system.
                // (A location that has a scheme, such as a `file:` URI, stays as it is.)
                let absolute = if crate::file_uri::uri_of(&path) == path {
                    path.clone()
                } else {
                    std::path::absolute(&path).ok().and_then(|p| p.to_str().map(str::to_owned)).unwrap_or_else(|| path.clone())
                };
                let uri = crate::file_uri::uri_of(&absolute);
                (path, uri)
            })
            .collect();
        *self.inner.user_catalogs.lock().unwrap_or_else(PoisonError::into_inner) = both;
    }

    /// The catalogs the person named, as they were given.
    pub fn user_catalogs(&self) -> Vec<String> {
        self.inner.user_catalogs.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|(path, _)| path.clone()).collect()
    }

    fn user_catalog_uris(&self) -> Vec<String> {
        self.inner.user_catalogs.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|(_, uri)| uri.clone()).collect()
    }

    /// How much the download cache holds (nothing, when there is none).
    pub fn cache_stats(&self) -> xmlcraft_resolve::CacheStats {
        self.inner.cache.as_ref().map(|c| c.stats()).unwrap_or_default()
    }

    /// Delete every downloaded copy; how many there were.
    pub fn clear_cache(&self) -> usize {
        self.inner.cache.as_ref().map_or(0, |c| c.clear())
    }

    /// Whether `XMLCRAFT_OFFLINE` was on when this network was made (the switch cannot turn downloading on then).
    pub fn offline_by_environment(&self) -> bool {
        self.inner.forced
    }

    /// Whether this network can download at all (a build with no client cannot; offline is a switch, not this).
    pub fn can_download(&self) -> bool {
        self.inner.fetcher.is_some()
    }

    /// A resolver that finds what documents name among the open documents (`buffers`, as they are being edited), the schemas that ship
    /// with the app, files and, unless offline, the web through the cache.
    pub fn resolver(&self, buffers: Arc<dyn Buffers>) -> Resolver {
        self.resolver_with_files(buffers, Arc::new(LocalFiles))
    }

    /// [`Network::resolver`] reading files with `files` (a tool confined to a folder reads only inside it; the web is a different
    /// matter, which the person's switch and `XMLCRAFT_OFFLINE` decide).
    pub fn resolver_with_files(&self, buffers: Arc<dyn Buffers>, files: Arc<dyn FileAccess>) -> Resolver {
        let env =
            Environment { buffers, files, fetcher: self.inner.fetcher.clone(), cache: self.inner.cache.clone(), clock: self.inner.clock.clone() };
        let offline = self.is_offline();
        Resolver::new(env, Policy { offline, ..Policy::default() }, &self.user_catalog_uris())
    }
}

impl std::fmt::Debug for Network {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Network").field("can_download", &self.can_download()).field("offline", &self.is_offline()).finish()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use xmlcraft_resolve::{Budget, Conditions, FetchError, FetchLimits, FetchOutcome, Fetched, FixedClock, MemoryCache, NoBuffers, Origin, Request};

    use super::*;

    /// Answers every URL with the same body and counts the questions.
    struct Mock {
        asked: Mutex<Vec<String>>,
    }

    impl Fetcher for Mock {
        fn fetch(&self, url: &str, _: &Conditions, _: &FetchLimits) -> Result<FetchOutcome, FetchError> {
            self.asked.lock().expect("lock").push(url.to_owned());
            Ok(FetchOutcome::Fresh(Fetched { body: b"<x/>".to_vec(), final_url: url.to_owned(), etag: None, last_modified: None }))
        }
    }

    fn resolve(network: &Network, url: &str) -> Result<Origin, String> {
        let resolver = network.resolver(Arc::new(NoBuffers));
        let request = Request::uri(url, "");
        resolver.resolve(&request, &mut Budget::new()).map(|found| found.origin).map_err(|failure| failure.message)
    }

    #[test]
    fn the_switch_is_shared_and_decides_whether_the_web_is_asked() {
        let mock = Arc::new(Mock { asked: Mutex::new(Vec::new()) });
        let network = Network::new(Some(mock.clone()), Some(Arc::new(MemoryCache::new())), Arc::new(FixedClock(1000)));
        let clone = network.clone();
        assert!(!network.is_offline() && network.can_download());
        assert_eq!(resolve(&network, "http://example.com/a.xsd"), Ok(Origin::Network));
        // It is in the cache now: no question is asked again.
        assert_eq!(resolve(&network, "http://example.com/a.xsd"), Ok(Origin::Cache { stale: false }));
        let before = mock.asked.lock().expect("lock").len();
        // Offline (set through a clone): a copy that is not in the cache is not downloaded.
        clone.set_offline(true);
        assert!(network.is_offline());
        let why = resolve(&network, "http://example.com/b.xsd").expect_err("not in the cache");
        assert!(why.contains("offline"), "{why}");
        assert_eq!(mock.asked.lock().expect("lock").len(), before, "nothing was asked");
        // Online again.
        network.set_offline(false);
        assert_eq!(resolve(&network, "http://example.com/b.xsd"), Ok(Origin::Network));
    }

    #[test]
    fn the_cache_says_what_it_holds_and_can_be_cleared() {
        let mock = Arc::new(Mock { asked: Mutex::new(Vec::new()) });
        let network = Network::new(Some(mock.clone()), Some(Arc::new(MemoryCache::new())), Arc::new(FixedClock(1000)));
        assert_eq!(network.cache_stats().entries, 0);
        assert_eq!(resolve(&network, "http://example.com/a.xsd"), Ok(Origin::Network));
        assert_eq!(resolve(&network, "http://example.com/b.xsd"), Ok(Origin::Network));
        let stats = network.cache_stats();
        assert_eq!(stats.entries, 2);
        assert!(stats.bytes > 0);
        assert_eq!(network.clear_cache(), 2);
        assert_eq!(network.cache_stats().entries, 0);
        // Asked for again, it is downloaded again.
        assert_eq!(resolve(&network, "http://example.com/a.xsd"), Ok(Origin::Network));
        // A network with no cache has nothing to say and nothing to clear.
        let none = Network::offline();
        assert_eq!((none.cache_stats().entries, none.clear_cache()), (0, 0));
    }

    #[test]
    fn a_catalog_the_person_named_finds_what_the_bundle_and_the_web_do_not() {
        let dir = tempfile::tempdir().expect("dir");
        let schema = dir.path().join("mine.xsd");
        std::fs::write(&schema, "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"/>").expect("writes");
        let catalog = dir.path().join("catalog.xml");
        std::fs::write(
            &catalog,
            "<catalog xmlns=\"urn:oasis:names:tc:entity:xmlns:xml:catalog\"><uri name=\"urn:mine:schema\" uri=\"mine.xsd\"/></catalog>",
        )
        .expect("writes");
        let network = Network::offline();
        assert!(resolve(&network, "urn:mine:schema").is_err(), "nothing knows the name yet");
        network.set_user_catalogs(vec![catalog.to_string_lossy().into_owned()]);
        assert_eq!(network.user_catalogs(), [catalog.to_string_lossy().into_owned()], "as they were given");
        assert_eq!(resolve(&network, "urn:mine:schema"), Ok(Origin::File));
        network.set_user_catalogs(Vec::new());
        assert!(resolve(&network, "urn:mine:schema").is_err(), "and not any more");
        // A relative path is the file in the folder the program runs in.
        let relative = std::path::Path::new("Cargo.toml").exists();
        assert!(relative, "the tests run in the crate's folder");
        network.set_user_catalogs(vec!["Cargo.toml".to_owned()]);
        network.set_user_catalogs(vec!["file:///x/catalog.xml".to_owned()]);
        assert_eq!(network.user_catalog_uris(), ["file:///x/catalog.xml"], "a URI is not taken for a relative path");
        network.set_user_catalogs(vec!["Cargo.toml".to_owned()]);
        let uris = network.user_catalog_uris();
        assert!(uris[0].starts_with("file:///") && uris[0].ends_with("/Cargo.toml") && uris[0].len() > "file:///Cargo.toml".len(), "{uris:?}");
    }

    #[test]
    fn the_environment_can_keep_a_network_offline_whatever_the_switch_says() {
        let mock = Arc::new(Mock { asked: Mutex::new(Vec::new()) });
        let network = Network::build(Some(mock.clone()), None, Arc::new(FixedClock(1)), true);
        assert!(network.offline_by_environment() && network.is_offline());
        network.set_offline(false);
        assert!(network.is_offline(), "the switch cannot turn it on");
        let why = resolve(&network, "http://example.com/a.xsd").expect_err("offline");
        assert!(why.contains("offline"), "{why}");
        assert!(mock.asked.lock().expect("lock").is_empty());
        // Without it, the switch decides.
        let free = Network::new(Some(mock), None, Arc::new(FixedClock(1)));
        assert!(!free.offline_by_environment() && !free.is_offline());
        free.set_offline(true);
        assert!(free.is_offline());
    }

    #[test]
    fn a_network_with_no_client_downloads_nothing_whatever_the_switch_says() {
        let network = Network::offline();
        network.set_offline(false);
        assert!(network.is_offline() && !network.can_download());
        assert!(!network.offline_by_environment());
        let why = resolve(&network, "http://example.com/a.xsd").expect_err("cannot");
        assert!(why.contains("not in the cache"), "{why}");
    }

    #[test]
    fn switches_in_the_environment_are_read_strictly() {
        for on in ["1", "true", "TRUE", "Yes", " on "] {
            assert!(is_on(Some(on.into())), "{on}");
        }
        for off in ["", "0", "false", "no", "off", "2", "enabled"] {
            assert!(!is_on(Some(off.into())), "{off}");
        }
        assert!(!is_on(None));
    }

    #[test]
    fn the_system_clock_is_after_2020() {
        assert!(SystemClock.now() > 1_577_836_800);
    }
}
