//! The XML Schema a document names, loaded in the background so that Entry Helpers have it when asked.
//!
//! Loading a schema can mean reading files (and, when documents are open, taking those as they are being edited), so it is not done for
//! every edit: it is asked for when the schemas the document names, the place the document is at, or the documents that are open
//! (other than the one being worked in) change, and the answer says which request it is for.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

use xmlcraft_resolve::normalized;
use xmlcraft_text::Buffer;
use xmlcraft_validate::{SchemaHint, load_hinted};
use xmlcraft_xsd::ResolvingLoader;
use xmlcraft_xsd::schema::Schema;

use crate::validation::{OpenDocuments, ResolverFactory};
use crate::worker::Worker;

/// The assembled schema.
pub type XsdSchema = Schema;

/// What came of a request to load a schema.
#[derive(Clone, Debug)]
pub struct XsdAnswer {
    /// Which request this answers ([`xsd_key`]).
    pub key: u64,
    /// The assembled schema, or why there is none. A schema with problems of its own is still one: what it declares can be offered.
    pub schema: Result<Arc<Schema>, String>,
}

/// A number for a request: two requests with the same key have the same answer (as long as the files they read are the same).
/// `open` are the paths and versions of the documents that are open; those that are not the document asked about are what the schema
/// may be made of, so an edit of one of them is a new request.
pub fn xsd_key(hints: &[SchemaHint], path: Option<&Path>, open: &[(String, u64)]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for hint in hints {
        hint.namespace.hash(&mut hasher);
        hint.location.hash(&mut hasher);
    }
    path.hash(&mut hasher);
    for (location, version) in open {
        normalized(location).hash(&mut hasher);
        version.hash(&mut hasher);
    }
    hasher.finish()
}

/// What to ask for when a document names no schema but its root element is in `namespace`: the schema the catalogs or the bundle know
/// for it (the namespace is asked for as a location, which they answer by what they map it to).
pub fn implicit_hints(namespace: &str) -> Vec<SchemaHint> {
    vec![SchemaHint { namespace: Some(namespace.to_owned()), location: namespace.to_owned() }]
}

struct Request {
    key: u64,
    hints: Vec<SchemaHint>,
    location: String,
    open: Vec<(String, Buffer)>,
}

/// Loads schemas on a worker thread. `submit` returns at once; `poll` collects what has finished.
pub struct XsdSchemas {
    worker: Worker<Request, XsdAnswer>,
}

impl XsdSchemas {
    /// A loader whose requests find schema documents with the resolvers `resolvers` makes.
    pub fn new(resolvers: ResolverFactory) -> Self {
        Self { worker: Worker::new("xsd-schema", move |request| run(&resolvers, request)) }
    }

    /// A loader with no thread (what the web does, and handy for tests).
    pub fn inline(resolvers: ResolverFactory) -> Self {
        Self { worker: Worker::inline(move |request| run(&resolvers, request)) }
    }

    /// Called whenever a result is ready.
    pub fn on_done(&self, notify: impl Fn() + Send + Sync + 'static) {
        self.worker.on_done(notify);
    }

    /// Ask for the schemas `hints` (named by the document with this id, saved at `path` if it is) to be loaded as the request `key`.
    /// `open` are the documents that are open, by their paths. A request that has not started is replaced by a newer one for the same
    /// document.
    pub fn submit(&self, document: u64, key: u64, hints: Vec<SchemaHint>, path: Option<&Path>, open: Vec<(String, Buffer)>) {
        let location = path.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        self.worker.submit(document, Request { key, hints, location, open });
    }

    /// The answers that arrived since the last call, oldest first.
    pub fn poll(&self) -> Vec<(u64, XsdAnswer)> {
        self.worker.poll()
    }
}

fn run(resolvers: &ResolverFactory, request: Request) -> XsdAnswer {
    let key = request.key;
    let loaded = crate::guard(|| {
        let resolver = resolvers(Arc::new(OpenDocuments::new(request.open)));
        let mut loader = ResolvingLoader::new(&resolver);
        load_hinted(request.hints, &request.location, &mut loader).map(|loaded| Arc::new(loaded.assembly.schema)).map_err(|miss| miss.why)
    });
    let schema = loaded.unwrap_or_else(|message| {
        log::error!("loading a schema failed: {message}");
        Err("loading the schema failed; this is a bug".to_owned())
    });
    XsdAnswer { key, schema }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use xmlcraft_resolve::{Buffers, Environment, FileAccess, FixedClock, Policy, Resolver};

    use super::*;

    const XS: &str = "http://www.w3.org/2001/XMLSchema";

    struct NoFiles;

    impl FileAccess for NoFiles {
        fn read(&self, path: &str) -> Result<Vec<u8>, String> {
            Err(format!("{path}: no such file"))
        }
    }

    /// Resolvers that find the schema documents among the documents that are open, and nowhere else.
    fn resolvers() -> ResolverFactory {
        Arc::new(|buffers: Arc<dyn Buffers>| {
            let env = Environment { buffers, files: Arc::new(NoFiles), fetcher: None, cache: None, clock: Arc::new(FixedClock(0)) };
            Resolver::new(env, Policy { auto_download: false, offline: true, ..Policy::default() }, &[])
        })
    }

    fn schema_text(element: &str) -> String {
        format!("<xs:schema xmlns:xs=\"{XS}\"><xs:element name=\"{element}\" type=\"xs:string\"/></xs:schema>")
    }

    fn hint(location: &str) -> SchemaHint {
        SchemaHint { namespace: None, location: location.to_owned() }
    }

    #[test]
    fn the_schema_for_a_namespace_is_asked_for_by_the_namespace() {
        assert_eq!(implicit_hints("urn:a"), [SchemaHint { namespace: Some("urn:a".to_owned()), location: "urn:a".to_owned() }]);
    }

    #[test]
    fn the_key_changes_with_what_the_schema_is_made_of() {
        let base = xsd_key(&[hint("a.xsd")], None, &[("/s/a.xsd".to_owned(), 1)]);
        assert_eq!(base, xsd_key(&[hint("a.xsd")], None, &[("/s/a.xsd".to_owned(), 1)]));
        assert_ne!(base, xsd_key(&[hint("b.xsd")], None, &[("/s/a.xsd".to_owned(), 1)]), "another schema");
        assert_ne!(base, xsd_key(&[hint("a.xsd")], Some(Path::new("/d/x.xml")), &[("/s/a.xsd".to_owned(), 1)]), "another place");
        assert_ne!(base, xsd_key(&[hint("a.xsd")], None, &[("/s/a.xsd".to_owned(), 2)]), "an open document was edited");
        assert_ne!(base, xsd_key(&[hint("a.xsd")], None, &[]), "an open document was closed");
        assert_ne!(
            xsd_key(&[SchemaHint { namespace: Some("urn:a".to_owned()), location: "a.xsd".to_owned() }], None, &[]),
            xsd_key(&[hint("a.xsd")], None, &[]),
            "a schema for a namespace"
        );
        // The spellings of one place are one place.
        assert_eq!(xsd_key(&[], None, &[("C:\\s\\a.xsd".to_owned(), 1)]), xsd_key(&[], None, &[("C:/s/a.xsd".to_owned(), 1)]));
    }

    #[test]
    fn a_schema_is_loaded_from_the_open_documents_as_they_are_being_edited() {
        let schemas = XsdSchemas::inline(resolvers());
        schemas.submit(
            1,
            77,
            vec![hint("a.xsd")],
            Some(Path::new("/d/x.xml")),
            vec![("/d/a.xsd".to_owned(), Buffer::from_text(&schema_text("edited")))],
        );
        let answers = schemas.poll();
        assert_eq!(answers.len(), 1);
        let (document, answer) = &answers[0];
        assert_eq!((*document, answer.key), (1, 77));
        let schema = answer.schema.as_ref().expect("loaded");
        assert!(schema.element_by_name(&xmlcraft_xsd::model::QName::new(None, "edited")).is_some());
    }

    #[test]
    fn a_schema_that_cannot_be_found_says_so() {
        let schemas = XsdSchemas::inline(resolvers());
        schemas.submit(2, 5, vec![hint("gone.xsd")], Some(Path::new("/d/x.xml")), Vec::new());
        let answers = schemas.poll();
        let why = answers[0].1.schema.as_ref().expect_err("not there");
        assert!(why.contains("gone.xsd"), "{why}");
        // Nothing named at all.
        schemas.submit(2, 6, Vec::new(), None, Vec::new());
        assert!(schemas.poll()[0].1.schema.is_err());
    }
}
