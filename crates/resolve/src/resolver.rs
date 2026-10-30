//! The resolution chain: what text does this reference name?
//!
//! In order, for a reference with an optional public identifier, an optional system identifier (relative to the document that has it) or
//! a URI/namespace:
//! 1. the documents that are open, by the location the reference makes (so an unsaved schema is used as it is);
//! 2. the user's catalogs;
//! 3. the bundled catalogs (which lead to the schemas that ship with the app);
//! 4. the location itself: a bundled file, a file, or, for `http` and `https`, the cache and then the network.
//!
//! A location a catalog maps to is read the same way as one a document names. Downloading happens only when the policy allows it, and an
//! `http` URL is tried as `https` first. Every place that was looked in is recorded in the [`Trail`].

use std::sync::Arc;

use xmlcraft_catalog::{Catalog, Catalogs, Loader};

use crate::budget::{Budget, MAX_DOWNLOAD_BYTES};
use crate::cache::{CacheEntry, ResourceCache};
use crate::env::{Buffers, Clock, Conditions, FetchLimits, FetchOutcome, Fetcher, FileAccess};
use crate::location::{file_path, https_of, join, scheme_of};
use crate::trail::{Outcome, Stage, Trail};

/// What kind of name a reference is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An external identifier of a DTD part: a public identifier, a system identifier, or both.
    Identifier,
    /// A URI or a namespace (an `xs:import`, an `xs:include`, a schema location): looked up in the catalogs' `uri` entries.
    Uri,
}

/// A reference to resolve.
#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    pub kind: Kind,
    pub public: Option<&'a str>,
    /// The system identifier or URI as written; relative to `base` when it is a relative reference. For a [`Kind::Uri`] reference that
    /// has only a namespace, the namespace.
    pub system: Option<&'a str>,
    /// The location of the document that has the reference (a URI or a path; empty when it has none).
    pub base: &'a str,
    /// Look in the open documents and the catalogs only, never at the address itself (not as a file and not on the web). A namespace is
    /// asked for so: it names a schema, and is not where one is.
    pub lookup_only: bool,
}

impl<'a> Request<'a> {
    /// A DTD's external subset or parameter entity.
    pub fn external_id(public: Option<&'a str>, system: Option<&'a str>, base: &'a str) -> Self {
        Self { kind: Kind::Identifier, public, system, base, lookup_only: false }
    }

    /// A schema import, include or location.
    pub fn uri(uri: &'a str, base: &'a str) -> Self {
        Self { kind: Kind::Uri, public: None, system: Some(uri), base, lookup_only: false }
    }

    /// The same reference, looked for in the open documents and the catalogs only (see [`Request::lookup_only`]).
    #[must_use]
    pub fn lookup_only(mut self) -> Self {
        self.lookup_only = true;
        self
    }

    fn describe(&self) -> String {
        match (self.kind, self.public, self.system) {
            (Kind::Uri, _, system) => format!("uri {}", system.unwrap_or_default()),
            (Kind::Identifier, Some(public), Some(system)) => format!("public \"{public}\" system \"{system}\""),
            (Kind::Identifier, Some(public), None) => format!("public \"{public}\""),
            (Kind::Identifier, None, system) => format!("system \"{}\"", system.unwrap_or_default()),
        }
    }
}

/// Where the text came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A document that is open.
    Buffer,
    /// A schema that ships with the app.
    Bundle,
    /// A file.
    File,
    /// The cache; `stale` when it was past its time and could not be refreshed (offline, downloading off, or the server not answering).
    Cache { stale: bool },
    /// Just downloaded.
    Network,
}

/// A resolved reference.
#[derive(Clone, Debug)]
pub struct Resolved {
    /// Where the text was read from, after redirects: what relative references inside it are resolved against.
    pub location: String,
    pub bytes: Vec<u8>,
    pub origin: Origin,
    pub trail: Trail,
}

/// A reference that could not be resolved.
#[derive(Clone, Debug)]
pub struct Failure {
    pub message: String,
    pub trail: Trail,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

/// What the user has allowed.
#[derive(Clone, Debug)]
pub struct Policy {
    /// Download what is referenced and not found anywhere else (Preferences ▸ Schemas; on by default).
    pub auto_download: bool,
    /// Work offline: nothing is downloaded, whatever `auto_download` says, and a copy past its time is still served.
    pub offline: bool,
    /// Use the schemas that ship with the app.
    pub use_bundled: bool,
    /// How long a downloaded copy is current without asking the server, in seconds (30 days).
    pub fresh_for_secs: i64,
}

impl Default for Policy {
    fn default() -> Self {
        Self { auto_download: true, offline: false, use_bundled: true, fresh_for_secs: 30 * 24 * 60 * 60 }
    }
}

/// What the resolver reads and asks.
#[derive(Clone)]
pub struct Environment {
    pub buffers: Arc<dyn Buffers>,
    pub files: Arc<dyn FileAccess>,
    /// None: nothing is downloaded.
    pub fetcher: Option<Arc<dyn Fetcher>>,
    /// None: nothing is kept.
    pub cache: Option<Arc<dyn ResourceCache>>,
    pub clock: Arc<dyn Clock>,
}

/// Catalogs named but not given are read from the bundle or from files.
struct CatalogLoader<'a> {
    files: &'a dyn FileAccess,
}

impl Loader for CatalogLoader<'_> {
    fn load(&mut self, location: &str) -> Option<String> {
        if let Some(path) = xmlcraft_schemas::path_of(location) {
            return xmlcraft_schemas::get_text(path);
        }
        String::from_utf8(self.files.read(&file_path(location)?).ok()?).ok()
    }
}

struct Found {
    location: String,
    bytes: Vec<u8>,
    origin: Origin,
}

/// Finds the text that references name.
pub struct Resolver {
    env: Environment,
    policy: Policy,
    user: Catalogs,
    bundled: Catalogs,
    warnings: Vec<String>,
}

impl Resolver {
    /// A resolver over `env`. `user_catalogs` are the locations of the catalogs the user named, consulted before the bundled ones; one
    /// that cannot be read is skipped and noted in [`Resolver::warnings`].
    pub fn new(env: Environment, policy: Policy, user_catalogs: &[String]) -> Self {
        let mut warnings = Vec::new();
        let mut user = Catalogs::new();
        for location in user_catalogs {
            let read = file_path(location)
                .ok_or_else(|| "it is not a file".to_owned())
                .and_then(|path| env.files.read(&path))
                .and_then(|bytes| String::from_utf8(bytes).map_err(|_| "it is not UTF-8".to_owned()))
                .and_then(|text| Catalog::parse(&text, location).map_err(|e| e.to_string()));
            match read {
                Ok(catalog) => {
                    warnings.extend(catalog.warnings().iter().map(|w| format!("{location}: {w}")));
                    user.push(catalog);
                }
                Err(why) => warnings.push(format!("{location}: the catalog is not used: {why}")),
            }
        }
        let mut bundled = Catalogs::new();
        let root = xmlcraft_schemas::uri_of(xmlcraft_schemas::ROOT_CATALOG);
        match xmlcraft_schemas::root_catalog().map(|text| Catalog::parse(&text, &root)) {
            Some(Ok(catalog)) => bundled.push(catalog),
            Some(Err(why)) => warnings.push(format!("{root}: the bundled catalog is not used: {why}")),
            None => warnings.push(format!("{root}: there is no bundled catalog")),
        }
        Self { env, policy, user, bundled, warnings }
    }

    /// What was wrong with the catalogs given to [`Resolver::new`].
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// Resolve `request`, spending `budget`.
    pub fn resolve(&self, request: &Request<'_>, budget: &mut Budget) -> Result<Resolved, Failure> {
        let mut trail = Trail::default();
        let written = request.system.filter(|s| !s.is_empty());
        let absolute = written.map(|s| join(request.base, s));
        let fail = |message: String, trail: Trail| Failure { message, trail };
        // 1. The open documents come before everything else.
        if let Some(location) = &absolute
            && !self.is_remote(location)
            && let Some(text) = self.env.buffers.text(location)
        {
            trail.push(Stage::Buffer, location.clone(), Outcome::Found);
            return self.finish(Found { location: location.clone(), bytes: text.as_bytes().to_vec(), origin: Origin::Buffer }, budget, trail);
        }
        // 2 and 3. The catalogs. A catalog that finds something we cannot read does not stop the search: the next one may do better.
        let catalogs = [(Stage::UserCatalog, &self.user, true), (Stage::BundledCatalog, &self.bundled, self.policy.use_bundled)];
        for (stage, catalogs, enabled) in catalogs {
            if !enabled {
                continue;
            }
            let Some(target) = self.through(stage, catalogs, request, absolute.as_deref(), &mut trail) else { continue };
            match self.load(&target, &mut trail) {
                Ok(found) => return self.finish(found, budget, trail),
                Err(_) => continue,
            }
        }
        // 4. The location itself.
        match absolute {
            Some(_) if request.lookup_only => {
                trail.push(Stage::File, request.describe(), Outcome::Skipped("it is looked up in the catalogs only".to_owned()));
                Err(fail(format!("no catalog knows {}", request.describe()), trail))
            }
            Some(location) => match self.load(&location, &mut trail) {
                Ok(found) => self.finish(found, budget, trail),
                Err(why) => Err(fail(why, trail)),
            },
            None => Err(fail(format!("nothing names {}: no system identifier, and no catalog knows it", request.describe()), trail)),
        }
    }

    /// What the document at `location` is, so that two locations of one document are told to be one: for a file (a path, or a `file:` URI)
    /// what the files say it is (see [`FileAccess::identity`]), for anything else (the address of a resource, which is case-sensitive) the
    /// location itself.
    pub fn identity(&self, location: &str) -> String {
        match file_path(location) {
            Some(path) => self.env.files.identity(&path),
            None => location.to_owned(),
        }
    }

    fn is_remote(&self, location: &str) -> bool {
        matches!(scheme_of(location).as_deref(), Some("http" | "https"))
    }

    /// What `catalogs` map the request to, written as it is or made absolute.
    fn through(&self, stage: Stage, catalogs: &Catalogs, request: &Request<'_>, absolute: Option<&str>, trail: &mut Trail) -> Option<String> {
        if catalogs.is_empty() {
            return None;
        }
        let mut loader = CatalogLoader { files: &*self.env.files };
        let written = request.system.filter(|s| !s.is_empty());
        let mut ask = |system: Option<&str>| match request.kind {
            Kind::Identifier => catalogs.resolve(request.public, system, &mut loader),
            Kind::Uri => system.and_then(|s| catalogs.resolve_uri(s, &mut loader)),
        };
        let found = ask(written).or_else(|| absolute.filter(|a| Some(*a) != written).and_then(|a| ask(Some(a))));
        match &found {
            Some(target) => trail.push(stage, format!("{} → {target}", request.describe()), Outcome::Found),
            None => trail.push(stage, request.describe(), Outcome::Missed),
        }
        found
    }

    /// Spend the budget on what was found.
    fn finish(&self, found: Found, budget: &mut Budget, mut trail: Trail) -> Result<Resolved, Failure> {
        match budget.charge(found.bytes.len()) {
            Ok(()) => Ok(Resolved { location: found.location, bytes: found.bytes, origin: found.origin, trail }),
            Err(why) => {
                trail.push(Stage::File, found.location, Outcome::Failed(why.clone()));
                Err(Failure { message: why, trail })
            }
        }
    }

    /// Read what `target` names (not through the catalogs). Open documents are asked for a file-like `target`.
    fn load(&self, target: &str, trail: &mut Trail) -> Result<Found, String> {
        match scheme_of(target).as_deref() {
            Some("bundled") => {
                let bytes = xmlcraft_schemas::path_of(target).and_then(xmlcraft_schemas::get);
                match bytes {
                    Some(bytes) => {
                        trail.push(Stage::Bundle, target, Outcome::Found);
                        Ok(Found { location: target.to_owned(), bytes, origin: Origin::Bundle })
                    }
                    None => {
                        let why = format!("{target}: there is no such bundled schema");
                        trail.push(Stage::Bundle, target, Outcome::Failed(why.clone()));
                        Err(why)
                    }
                }
            }
            None | Some("file") => {
                if let Some(text) = self.env.buffers.text(target) {
                    trail.push(Stage::Buffer, target, Outcome::Found);
                    return Ok(Found { location: target.to_owned(), bytes: text.as_bytes().to_vec(), origin: Origin::Buffer });
                }
                let Some(path) = file_path(target) else {
                    let why = format!("{target}: not a file this computer can read");
                    trail.push(Stage::File, target, Outcome::Failed(why.clone()));
                    return Err(why);
                };
                match self.env.files.read(&path) {
                    Ok(bytes) => {
                        trail.push(Stage::File, target, Outcome::Found);
                        Ok(Found { location: target.to_owned(), bytes, origin: Origin::File })
                    }
                    Err(why) => {
                        trail.push(Stage::File, target, Outcome::Failed(why.clone()));
                        Err(why)
                    }
                }
            }
            Some("http" | "https") => self.network(target, trail),
            Some(other) => {
                let why = format!("{target}: the scheme {other}: is not read (only files, http and https)");
                trail.push(Stage::File, target, Outcome::Failed(why.clone()));
                Err(why)
            }
        }
    }

    /// The cache first, then, when allowed, the network.
    fn network(&self, url: &str, trail: &mut Trail) -> Result<Found, String> {
        let now = self.env.clock.now();
        let cached = self.env.cache.as_ref().and_then(|c| c.get(url));
        let from_cache =
            |entry: &CacheEntry, stale: bool| Found { location: entry.final_url.clone(), bytes: entry.body.clone(), origin: Origin::Cache { stale } };
        if let Some(entry) = &cached
            && entry.is_fresh(now, self.policy.fresh_for_secs)
        {
            trail.push(Stage::Cache, url, Outcome::Found);
            return Ok(from_cache(entry, false));
        }
        let refusal = if self.policy.offline {
            Some("offline")
        } else if !self.policy.auto_download {
            Some("downloading is off")
        } else if self.env.fetcher.is_none() {
            Some("this build cannot download")
        } else {
            None
        };
        let fetcher = match (&self.env.fetcher, refusal) {
            (Some(fetcher), None) => fetcher,
            (_, why) => {
                let why = why.unwrap_or("this build cannot download");
                trail.push(Stage::Network, url, Outcome::Skipped(why.to_owned()));
                return match &cached {
                    Some(entry) => {
                        trail.push(Stage::Cache, url, Outcome::Found);
                        Ok(from_cache(entry, true))
                    }
                    None => {
                        trail.push(Stage::Cache, url, Outcome::Missed);
                        Err(format!("{url} is not in the cache and {why}"))
                    }
                };
            }
        };
        let conditions = cached.as_ref().map(|e| Conditions { etag: e.etag.clone(), last_modified: e.last_modified.clone() }).unwrap_or_default();
        let limits = FetchLimits { max_bytes: MAX_DOWNLOAD_BYTES };
        // https is tried first for an http URL, then the URL as written.
        let candidates: Vec<String> = https_of(url).into_iter().chain(std::iter::once(url.to_owned())).collect();
        let mut last = String::new();
        for candidate in &candidates {
            match fetcher.fetch(candidate, &conditions, &limits) {
                Ok(FetchOutcome::NotModified) => match &cached {
                    Some(entry) => {
                        trail.push(Stage::Network, candidate.clone(), Outcome::Found);
                        let renewed = CacheEntry { fetched_at: now, ..entry.clone() };
                        if let Some(cache) = &self.env.cache {
                            cache.put(renewed.clone());
                        }
                        return Ok(from_cache(&renewed, false));
                    }
                    None => {
                        last = format!("{candidate}: the server said the copy is current, but there is none");
                        trail.push(Stage::Network, candidate.clone(), Outcome::Failed(last.clone()));
                    }
                },
                Ok(FetchOutcome::Fresh(fetched)) => {
                    if fetched.body.len() > limits.max_bytes {
                        last = format!("{candidate}: the resource is larger than the {} bytes a download may have", limits.max_bytes);
                        trail.push(Stage::Network, candidate.clone(), Outcome::Failed(last.clone()));
                        continue;
                    }
                    trail.push(Stage::Network, candidate.clone(), Outcome::Found);
                    let entry = CacheEntry {
                        url: url.to_owned(),
                        final_url: fetched.final_url,
                        body: fetched.body,
                        etag: fetched.etag,
                        last_modified: fetched.last_modified,
                        fetched_at: now,
                    };
                    if let Some(cache) = &self.env.cache {
                        cache.put(entry.clone());
                    }
                    return Ok(Found { location: entry.final_url, bytes: entry.body, origin: Origin::Network });
                }
                Err(error) => {
                    last = format!("{candidate}: {error}");
                    trail.push(Stage::Network, candidate.clone(), Outcome::Failed(error.to_string()));
                }
            }
        }
        // The server could not be asked: a copy past its time is better than none.
        match &cached {
            Some(entry) => {
                trail.push(Stage::Cache, url, Outcome::Found);
                Ok(from_cache(entry, true))
            }
            None => Err(last),
        }
    }
}
