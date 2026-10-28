//! Resolution: the rules of XML Catalogs 1.1 §7, over a list of catalog files. The catalogs that `nextCatalog` and the `delegate…`
//! entries name are loaded through a [`Loader`]: this crate does no I/O.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::reader::{Catalog, Entry};
use crate::uri::{normalize_public, normalize_uri, unwrap_publicid_urn};

/// What a resolution may load, in all: a catalog that names itself, or a long chain of them, ends here.
pub const MAX_LOADS: usize = 64;
/// How often a delegation may hand over to other catalogs in one resolution (a delegation that goes round in a circle does not load
/// anything new, so the load limit would not end it).
const MAX_DELEGATIONS: usize = MAX_LOADS;

/// Where the catalogs that are named but not given come from. `location` is the absolute URI the entry names; the answer is the text
/// of the catalog, or `None` when it cannot be had (then it is skipped).
pub trait Loader {
    fn load(&mut self, location: &str) -> Option<String>;

    /// A catalog that was asked for could not be used, or had entries left out: `problem` says why. For a caller that wants to show it;
    /// the default does nothing.
    fn note(&mut self, _location: &str, _problem: &str) {}
}

/// A loader that has nothing: catalogs that are named are not read.
pub struct NoLoader;

impl Loader for NoLoader {
    fn load(&mut self, _: &str) -> Option<String> {
        None
    }
}

/// The catalog entry file list: the catalogs to consult, in order.
#[derive(Clone, Debug)]
pub struct Catalogs {
    list: Vec<Arc<Catalog>>,
    prefer_public: bool,
}

impl Default for Catalogs {
    fn default() -> Self {
        Self { list: Vec::new(), prefer_public: true }
    }
}

/// What is asked: an external identifier (either part may be missing) or a URI (in `system`).
#[derive(Clone)]
struct Ids {
    public: Option<String>,
    system: Option<String>,
    external: bool,
}

/// What a catalog says about the identifiers.
enum Lookup {
    Found(String),
    /// The catalogs (by location, the longest match first) that take over, and what they are asked: the spec has a delegation restart
    /// with only the identifier that matched.
    Delegate(Vec<String>, Ids),
    /// Nothing here; the catalogs it names next, in order.
    Next(Vec<String>),
}

impl Catalogs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a catalog at the end of the list.
    pub fn push(&mut self, catalog: Catalog) {
        self.list.push(Arc::new(catalog));
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Whether a public identifier is used when a system identifier is given too, where no `prefer` attribute says (the spec leaves it
    /// to the application; the default is yes, as in libxml2 and the DocBook catalogs).
    pub fn set_prefer_public(&mut self, prefer_public: bool) {
        self.prefer_public = prefer_public;
    }

    /// Where the external identifier (`public`, `system`) maps to, or `None` when no catalog says. A `urn:publicid:` identifier is
    /// unwrapped first (§6.4, §7.1.1): a system identifier that is one counts as the public identifier (and as no system identifier);
    /// if a public identifier was given and is another, the system identifier is discarded.
    pub fn resolve(&self, public: Option<&str>, system: Option<&str>, loader: &mut dyn Loader) -> Option<String> {
        let mut public = public.map(|p| normalize_public(&unwrap_publicid_urn(p).unwrap_or_else(|| p.to_owned())));
        let mut system = system.map(str::to_owned);
        if let Some(unwrapped) = system.as_deref().and_then(unwrap_publicid_urn) {
            let unwrapped = normalize_public(&unwrapped);
            if public.is_none() {
                public = Some(unwrapped);
            }
            system = None;
        }
        let ids =
            Ids { public: public.filter(|p| !p.is_empty()), system: system.map(|s| normalize_uri(&s)).filter(|s| !s.is_empty()), external: true };
        if ids.public.is_none() && ids.system.is_none() {
            return None;
        }
        self.run(ids, loader)
    }

    /// Where the URI (a namespace name, a schema location, …) maps to, or `None`. A URN that unwraps to a public identifier is looked up
    /// as one (§7.2.1).
    pub fn resolve_uri(&self, uri: &str, loader: &mut dyn Loader) -> Option<String> {
        if let Some(public) = unwrap_publicid_urn(uri) {
            return self.resolve(Some(&public), None, loader);
        }
        self.run(Ids { public: None, system: Some(normalize_uri(uri)), external: false }, loader)
    }

    /// Go through the list as §7 says.
    fn run(&self, mut ids: Ids, loader: &mut dyn Loader) -> Option<String> {
        let mut queue: VecDeque<Arc<Catalog>> = self.list.iter().cloned().collect();
        let mut state = State { loader, loaded: HashMap::new(), loads: 0 };
        // The locations of the catalogs that have been consulted (in the original list or in a delegated one): a `nextCatalog` that names
        // one of them is not followed, which is what ends a cycle.
        let mut consulted: HashSet<String> = HashSet::new();
        let mut delegations = 0;
        while let Some(catalog) = queue.pop_front() {
            if !catalog.location().is_empty() {
                consulted.insert(catalog.location().to_owned());
            }
            let entries = catalog.entries();
            let lookup = if ids.external {
                lookup_external(entries, &ids, self.prefer_public)
            } else {
                lookup_uri(entries, ids.system.as_deref().unwrap_or_default())
            };
            match lookup {
                Lookup::Found(uri) => return Some(uri),
                Lookup::Delegate(locations, delegated) => {
                    // The delegated catalogs replace the list, and are asked only about what matched; the original ones are not asked
                    // again, whatever comes of it (§7.1.2 steps 5 and 7).
                    delegations += 1;
                    if delegations > MAX_DELEGATIONS {
                        return None;
                    }
                    ids = delegated;
                    queue.clear();
                    for location in locations {
                        if let Some(next) = state.load(&location) {
                            queue.push_back(next);
                        }
                    }
                }
                Lookup::Next(locations) => {
                    // Right after this catalog, in the order they are named (pushed to the front last to first). One that is waiting in
                    // the list already is moved up to its place after this catalog; one that has been consulted is not followed.
                    for location in locations.into_iter().rev() {
                        // A `nextCatalog` with no location (an empty reference and no base to resolve it against) names nothing that can
                        // be found; one that was consulted already is not followed.
                        if location.is_empty() || consulted.contains(&location) {
                            continue;
                        }
                        if let Some(at) = queue.iter().position(|waiting| waiting.location() == location) {
                            if let Some(waiting) = queue.remove(at) {
                                queue.push_front(waiting);
                            }
                        } else if let Some(next) = state.load(&location) {
                            queue.push_front(next);
                        }
                    }
                }
            }
        }
        None
    }
}

/// Loads and parses the catalogs that are named, each once, and no more than [`MAX_LOADS`] in a resolution.
struct State<'a> {
    loader: &'a mut dyn Loader,
    loaded: HashMap<String, Option<Arc<Catalog>>>,
    loads: usize,
}

impl State<'_> {
    fn load(&mut self, location: &str) -> Option<Arc<Catalog>> {
        if let Some(known) = self.loaded.get(location) {
            return known.clone();
        }
        if self.loads >= MAX_LOADS {
            return None;
        }
        self.loads += 1;
        let catalog = match self.loader.load(location) {
            None => {
                self.loader.note(location, "the catalog cannot be read");
                None
            }
            Some(text) => match Catalog::parse(&text, location) {
                Ok(catalog) => {
                    for warning in catalog.warnings() {
                        self.loader.note(location, warning);
                    }
                    Some(Arc::new(catalog))
                }
                Err(error) => {
                    self.loader.note(location, &error.to_string());
                    None
                }
            },
        };
        self.loaded.insert(location.to_owned(), catalog.clone());
        catalog
    }
}

/// The entry whose `key` is the longest of those that `matches` (with that length); the first of equals.
fn longest<'a, T>(items: impl Iterator<Item = (&'a str, T)>, matches: impl Fn(&str) -> bool) -> Option<(usize, T)> {
    let mut best: Option<(usize, T)> = None;
    for (key, value) in items {
        if matches(key) && best.as_ref().is_none_or(|(len, _)| key.len() > *len) {
            best = Some((key.len(), value));
        }
    }
    best
}

/// The catalogs of the matching delegate entries, the longest start string first (the first named of equals).
fn delegates<'a>(matching: impl Iterator<Item = (usize, &'a str)>) -> Vec<String> {
    let mut matching: Vec<(usize, &str)> = matching.collect();
    matching.sort_by_key(|(len, _)| std::cmp::Reverse(*len));
    matching.into_iter().map(|(_, catalog)| catalog.to_owned()).collect()
}

fn next_catalogs(entries: &[Entry]) -> Vec<String> {
    entries.iter().filter_map(|e| if let Entry::NextCatalog { catalog } = e { Some(catalog.clone()) } else { None }).collect()
}

/// §7.1.2, in the order the spec has it: system, rewriteSystem, systemSuffix, delegateSystem, public, delegatePublic, nextCatalog.
fn lookup_external(entries: &[Entry], ids: &Ids, prefer_public_by_default: bool) -> Lookup {
    // A public entry counts with a system identifier too only where `prefer="public"` is in effect.
    let counts = |prefer: &Option<bool>| ids.system.is_none() || prefer.unwrap_or(prefer_public_by_default);
    if let Some(system) = ids.system.as_deref() {
        if let Some(uri) = entries.iter().find_map(|e| match e {
            Entry::System { id, uri } if id == system => Some(uri.clone()),
            _ => None,
        }) {
            return Lookup::Found(uri);
        }
        let rewritten = longest(
            entries
                .iter()
                .filter_map(|e| if let Entry::RewriteSystem { start, prefix } = e { Some((start.as_str(), prefix.as_str())) } else { None }),
            |start| system.starts_with(start),
        );
        if let Some((start_len, prefix)) = rewritten {
            return Lookup::Found(format!("{prefix}{}", system.get(start_len..).unwrap_or_default()));
        }
        let suffixed = longest(
            entries.iter().filter_map(|e| if let Entry::SystemSuffix { suffix, uri } = e { Some((suffix.as_str(), uri.as_str())) } else { None }),
            |suffix| system.ends_with(suffix),
        );
        if let Some((_, uri)) = suffixed {
            return Lookup::Found(uri.to_owned());
        }
        let matching = delegates(entries.iter().filter_map(|e| match e {
            Entry::DelegateSystem { start, catalog } if system.starts_with(start.as_str()) => Some((start.len(), catalog.as_str())),
            _ => None,
        }));
        if !matching.is_empty() {
            return Lookup::Delegate(matching, Ids { public: None, system: ids.system.clone(), external: true });
        }
    }
    if let Some(public) = ids.public.as_deref() {
        if let Some(uri) = entries.iter().find_map(|e| match e {
            Entry::Public { id, uri, prefer } if id == public && counts(prefer) => Some(uri.clone()),
            _ => None,
        }) {
            return Lookup::Found(uri);
        }
        let matching = delegates(entries.iter().filter_map(|e| match e {
            Entry::DelegatePublic { start, catalog, prefer } if public.starts_with(start.as_str()) && counts(prefer) => {
                Some((start.len(), catalog.as_str()))
            }
            _ => None,
        }));
        if !matching.is_empty() {
            return Lookup::Delegate(matching, Ids { public: ids.public.clone(), system: None, external: true });
        }
    }
    Lookup::Next(next_catalogs(entries))
}

/// §7.2.2: uri, rewriteURI, uriSuffix, delegateURI, nextCatalog.
fn lookup_uri(entries: &[Entry], uri: &str) -> Lookup {
    if let Some(found) = entries.iter().find_map(|e| match e {
        Entry::Uri { name, uri: to } if name == uri => Some(to.clone()),
        _ => None,
    }) {
        return Lookup::Found(found);
    }
    let rewritten = longest(
        entries.iter().filter_map(|e| if let Entry::RewriteUri { start, prefix } = e { Some((start.as_str(), prefix.as_str())) } else { None }),
        |start| uri.starts_with(start),
    );
    if let Some((start_len, prefix)) = rewritten {
        return Lookup::Found(format!("{prefix}{}", uri.get(start_len..).unwrap_or_default()));
    }
    let suffixed = longest(
        entries.iter().filter_map(|e| if let Entry::UriSuffix { suffix, uri } = e { Some((suffix.as_str(), uri.as_str())) } else { None }),
        |suffix| uri.ends_with(suffix),
    );
    if let Some((_, found)) = suffixed {
        return Lookup::Found(found.to_owned());
    }
    let matching = delegates(entries.iter().filter_map(|e| match e {
        Entry::DelegateUri { start, catalog } if uri.starts_with(start.as_str()) => Some((start.len(), catalog.as_str())),
        _ => None,
    }));
    if !matching.is_empty() {
        return Lookup::Delegate(matching, Ids { public: None, system: Some(uri.to_owned()), external: false });
    }
    Lookup::Next(next_catalogs(entries))
}
