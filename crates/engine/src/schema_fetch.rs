//! Fetching the schema or DTD a document names (Load Schema): found with the resolver as any other schema document is, in the open
//! documents, the catalogs, the bundle, the files or, when the app may, on the web. The work runs on a worker thread so that the window
//! does not wait for a download, and behind the tool `xml_load_schema`.

use std::sync::Arc;

use xmlcraft_resolve::{Budget, Origin, Request, Trail, file_path};
use xmlcraft_text::Buffer;
use xmlcraft_text::encoding::{decode, sniff};

use crate::schema_link::SchemaLink;
use crate::validation::{OpenDocuments, ResolverFactory};
use crate::worker::Worker;

/// One request for a named grammar.
pub struct FetchJob {
    pub serial: u64,
    pub link: SchemaLink,
    /// Where the document that names it is (a path or a URI; empty when it has no file): relative locations are resolved against it.
    pub base: String,
    /// The documents that are open, by their paths, so that a schema being edited is read as it is.
    pub open: Vec<(String, Buffer)>,
}

/// A grammar that was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetched {
    /// Where it was read from, after redirects.
    pub location: String,
    /// Its path, when it is a file on this machine.
    pub path: Option<String>,
    pub text: String,
    /// Characters that could not be decoded were replaced.
    pub lossy: bool,
    pub origin: Origin,
}

impl Fetched {
    /// Where it came from, in a few words.
    pub fn origin_name(&self) -> &'static str {
        match self.origin {
            Origin::Buffer => "an open document",
            Origin::Bundle => "the schemas that ship with the app",
            Origin::File => "a file",
            Origin::Cache { stale: false } => "the download cache",
            Origin::Cache { stale: true } => "the download cache (it could not be refreshed)",
            Origin::Network => "the web",
        }
    }
}

pub struct FetchOutcome {
    pub serial: u64,
    /// The link that was looked up.
    pub link: SchemaLink,
    pub result: Result<Fetched, String>,
    /// Every place the resolver looked in, and what each said.
    pub trail: Trail,
}

/// Look `link` up with `resolver`.
pub fn fetch(resolver: &xmlcraft_resolve::Resolver, link: &SchemaLink, base: &str) -> Result<Fetched, String> {
    fetch_traced(resolver, link, base).0
}

/// Look `link` up with `resolver`, and say every place it looked in.
pub fn fetch_traced(resolver: &xmlcraft_resolve::Resolver, link: &SchemaLink, base: &str) -> (Result<Fetched, String>, Trail) {
    let request = match link {
        SchemaLink::ExternalId { public, system } => Request::external_id(public.as_deref(), system.as_deref(), base),
        SchemaLink::Uri { location, .. } => Request::uri(location, base),
    };
    match resolver.resolve(&request, &mut Budget::default()) {
        Ok(resolved) => {
            let xmlcraft_resolve::Resolved { location, bytes, origin, trail } = resolved;
            let decoded = decode(&bytes, &sniff(&bytes));
            let path = if matches!(origin, Origin::File | Origin::Buffer) { file_path(&location) } else { None };
            (Ok(Fetched { location, path, text: decoded.text, lossy: decoded.lossy, origin }), trail)
        }
        Err(failure) => (Err(failure.message), failure.trail),
    }
}

fn run(resolvers: &ResolverFactory, job: FetchJob) -> FetchOutcome {
    let resolver = resolvers(Arc::new(OpenDocuments::new(job.open)));
    let (result, trail) = match crate::guard(|| fetch_traced(&resolver, &job.link, &job.base)) {
        Ok(done) => done,
        Err(panic) => {
            log::error!("loading a schema failed: {panic}");
            (Err("loading the schema failed (a bug); nothing was changed".to_owned()), Trail::default())
        }
    };
    FetchOutcome { serial: job.serial, link: job.link, result, trail }
}

/// Fetches named grammars on a worker thread.
pub struct SchemaFetcher {
    worker: Worker<FetchJob, FetchOutcome>,
}

impl SchemaFetcher {
    pub fn new(resolvers: ResolverFactory) -> Self {
        Self { worker: Worker::new("schema loading", move |job| run(&resolvers, job)) }
    }

    pub fn inline(resolvers: ResolverFactory) -> Self {
        Self { worker: Worker::inline(move |job| run(&resolvers, job)) }
    }

    pub fn on_done(&self, notify: impl Fn() + Send + Sync + 'static) {
        self.worker.on_done(notify);
    }

    pub fn submit(&self, job: FetchJob) {
        self.worker.submit(0, job);
    }

    pub fn poll(&self) -> Vec<FetchOutcome> {
        self.worker.poll().into_iter().map(|(_, outcome)| outcome).collect()
    }
}
