//! The JSON Schema a JSON or YAML document names, loaded in the background so that completion and hover have it when asked.
//!
//! Reading a schema can mean reading files and, where the app may, downloading, so it is not done for every edit: it is asked for when
//! what the document names, the place it is at, or the documents that are open (other than itself) change, and the answer says which
//! request it is for. The schema is held as read: a schema that has problems of its own still offers what it declares.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

pub use xmlcraft_json::SchemaSet;
use xmlcraft_resolve::normalized;
use xmlcraft_text::Buffer;

use crate::json_validation::load_schema_set;
use crate::validation::{OpenDocuments, ResolverFactory};
use crate::worker::Worker;

/// What came of a request to load a schema.
#[derive(Clone)]
pub struct JsonAnswer {
    /// Which request this answers ([`json_key`]).
    pub key: u64,
    /// The schema and what it refers to, or why there is none.
    pub schema: Result<Arc<SchemaSet>, String>,
}

/// A number for a request: two requests with the same key have the same answer (as long as the files they read are the same). `open`
/// are the paths and versions of the documents that are open and are not the one asked about: an edit of one of them, which may be the
/// schema, is a new request.
pub fn json_key(reference: &str, path: Option<&Path>, open: &[(String, u64)]) -> u64 {
    let mut hasher = DefaultHasher::new();
    reference.hash(&mut hasher);
    path.hash(&mut hasher);
    for (location, version) in open {
        normalized(location).hash(&mut hasher);
        version.hash(&mut hasher);
    }
    hasher.finish()
}

struct Request {
    key: u64,
    reference: String,
    location: String,
    open: Vec<(String, Buffer)>,
}

/// Loads schemas on a worker thread. `submit` returns at once; `poll` collects what has finished.
pub struct JsonSchemas {
    worker: Worker<Request, JsonAnswer>,
}

impl JsonSchemas {
    /// A loader whose requests find schema documents with the resolvers `resolvers` makes.
    pub fn new(resolvers: ResolverFactory) -> Self {
        Self { worker: Worker::new("json-schema", move |request| run(&resolvers, request)) }
    }

    /// A loader with no thread (what the web does, and handy for tests).
    pub fn inline(resolvers: ResolverFactory) -> Self {
        Self { worker: Worker::inline(move |request| run(&resolvers, request)) }
    }

    /// Called whenever a result is ready.
    pub fn on_done(&self, notify: impl Fn() + Send + Sync + 'static) {
        self.worker.on_done(notify);
    }

    /// Ask for the schema `reference` (named by the document with this id, saved at `path` if it is) to be loaded as the request `key`.
    /// `open` are the documents that are open, by their paths. A request that has not started is replaced by a newer one for the same
    /// document.
    pub fn submit(&self, document: u64, key: u64, reference: String, path: Option<&Path>, open: Vec<(String, Buffer)>) {
        let location = path.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        self.worker.submit(document, Request { key, reference, location, open });
    }

    /// The answers that arrived since the last call, oldest first.
    pub fn poll(&self) -> Vec<(u64, JsonAnswer)> {
        self.worker.poll()
    }
}

fn run(resolvers: &ResolverFactory, request: Request) -> JsonAnswer {
    let key = request.key;
    let loaded = crate::guard(|| {
        let resolver = resolvers(Arc::new(OpenDocuments::new(request.open)));
        load_schema_set(&request.reference, &request.location, &resolver).map(Arc::new)
    });
    let schema = loaded.unwrap_or_else(|message| {
        log::error!("loading a JSON Schema failed: {message}");
        Err("loading the schema failed; this is a bug".to_owned())
    });
    JsonAnswer { key, schema }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use xmlcraft_resolve::{Buffers, Environment, FileAccess, FixedClock, Policy, Resolver};

    use super::*;

    struct NoFiles;

    impl FileAccess for NoFiles {
        fn read(&self, path: &str) -> Result<Vec<u8>, String> {
            Err(format!("{path}: no such file"))
        }
    }

    /// Resolvers that find schema documents among the documents that are open, and nowhere else.
    fn resolvers() -> ResolverFactory {
        Arc::new(|buffers: Arc<dyn Buffers>| {
            let env = Environment { buffers, files: Arc::new(NoFiles), fetcher: None, cache: None, clock: Arc::new(FixedClock(0)) };
            Resolver::new(env, Policy { auto_download: false, offline: true, ..Policy::default() }, &[])
        })
    }

    #[test]
    fn the_key_changes_with_the_reference_the_place_and_the_other_documents() {
        let open = vec![("/d/s.json".to_owned(), 1u64)];
        let key = json_key("s.json", Some(Path::new("/d/a.json")), &open);
        assert_eq!(key, json_key("s.json", Some(Path::new("/d/a.json")), &open));
        assert_ne!(key, json_key("t.json", Some(Path::new("/d/a.json")), &open));
        assert_ne!(key, json_key("s.json", Some(Path::new("/e/a.json")), &open));
        assert_ne!(key, json_key("s.json", Some(Path::new("/d/a.json")), &[("/d/s.json".to_owned(), 2)]), "the schema was edited");
    }

    #[test]
    fn a_schema_among_the_open_documents_is_loaded_as_it_is_being_edited_and_a_missing_one_says_so() {
        let loader = JsonSchemas::inline(resolvers());
        let schema = Buffer::from_text(r#"{"properties": {"name": {"type": "string"}}}"#);
        loader.submit(7, 1, "s.json".to_owned(), Some(Path::new("/d/a.json")), vec![("/d/s.json".to_owned(), schema)]);
        loader.submit(8, 2, "nothing.json".to_owned(), Some(Path::new("/d/b.json")), Vec::new());
        let mut answers = loader.poll();
        answers.sort_by_key(|(document, _)| *document);
        let [(7, found), (8, missing)] = answers.as_slice() else { panic!("two answers: {}", answers.len()) };
        let set = found.schema.as_ref().expect("found");
        assert_eq!((found.key, set.properties_at(&[], &[]).len()), (1, 1));
        assert_eq!(missing.key, 2);
        assert!(missing.schema.is_err());
    }
}
