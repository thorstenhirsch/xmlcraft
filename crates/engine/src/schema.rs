//! The grammar a document's DOCTYPE declares, built in the background so that completion has it when asked.
//!
//! Reading a DTD can mean reading files, so it is not done for every edit: it is asked for when the DOCTYPE (its text, or the place the
//! document is at) changes, and the result says which request it answers.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

use xmlcraft_dtd::{SubsetLoader, compile, read_doctype};
use xmlcraft_grammar::Grammar;
use xmlcraft_syntax::parse;

use crate::validation::LoaderFactory;
use crate::worker::Worker;

/// DOCTYPE declarations larger than this are not read for completion.
pub const MAX_DOCTYPE_BYTES: usize = 4 * 1024 * 1024;

/// What a DOCTYPE was read as.
#[derive(Clone, Debug)]
pub struct Schema {
    /// Which request this is the answer to ([`key_of`]).
    pub key: u64,
    /// The grammar, or none when the DOCTYPE declares nothing that could be read.
    pub grammar: Option<Arc<Grammar>>,
}

/// A number for a DOCTYPE at a place: two requests with the same key have the same answer (as long as the files it names are the same).
pub fn key_of(doctype: &str, path: Option<&Path>) -> u64 {
    let mut hasher = DefaultHasher::new();
    doctype.hash(&mut hasher);
    path.hash(&mut hasher);
    hasher.finish()
}

/// Read the DTD of the DOCTYPE declaration `doctype`, which starts at byte `start` of a document at `location`. (The offsets in the
/// grammar's places are those of the document.) None when the declaration is too big, or starts too far into the document, to be read.
pub fn read(doctype: &str, start: usize, location: &str, loader: &mut dyn SubsetLoader) -> Option<Grammar> {
    if doctype.len() > MAX_DOCTYPE_BYTES || start > MAX_DOCTYPE_BYTES {
        return None;
    }
    // The declaration where it is in the document, so that what the grammar says about places is true of the document.
    let text = format!("{}{doctype}<_/>", " ".repeat(start));
    let parsed = parse(&text);
    let (dtd, root) = read_doctype(&parsed.syntax(), &text, location, loader)?;
    Some(compile(&dtd.dtd, root.as_deref()).grammar)
}

struct Request {
    key: u64,
    doctype: String,
    start: usize,
    location: String,
}

/// Reads DOCTYPEs on a worker thread. `submit` returns at once; `poll` collects what has finished.
pub struct Schemas {
    worker: Worker<Request, Schema>,
}

impl Schemas {
    /// A reader whose requests read the external parts of DTDs with the loaders `loaders` makes.
    pub fn new(loaders: LoaderFactory) -> Self {
        Self { worker: Worker::new("schema", move |request| run(&loaders, request)) }
    }

    /// A reader with no thread (what the web does, and handy for tests).
    pub fn inline(loaders: LoaderFactory) -> Self {
        Self { worker: Worker::inline(move |request| run(&loaders, request)) }
    }

    /// Called whenever a result is ready.
    pub fn on_done(&self, notify: impl Fn() + Send + Sync + 'static) {
        self.worker.on_done(notify);
    }

    /// Ask for the DOCTYPE declaration `doctype` (at byte `start` of the document with this id, saved at `path` if it is) to be read as
    /// the request `key`. A request that has not started is replaced by a newer one for the same document.
    pub fn submit(&self, document: u64, key: u64, doctype: String, start: usize, path: Option<&Path>) {
        let location = path.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        self.worker.submit(document, Request { key, doctype, start, location });
    }

    /// The results that finished since the last call, oldest first.
    pub fn poll(&self) -> Vec<(u64, Schema)> {
        self.worker.poll()
    }
}

fn run(loaders: &LoaderFactory, request: Request) -> Schema {
    let mut loader = loaders();
    let grammar = crate::guard(|| read(&request.doctype, request.start, &request.location, loader.as_mut())).unwrap_or_else(|message| {
        log::error!("reading a DOCTYPE failed: {message}");
        None
    });
    Schema { key: request.key, grammar: grammar.map(Arc::new) }
}

#[cfg(test)]
mod tests {
    use xmlcraft_dtd::NoLoader;
    use xmlcraft_grammar::Name;

    use super::*;

    #[test]
    fn the_grammar_of_a_doctype_is_built_with_places_in_the_document() {
        let doctype = "<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY>]>";
        let grammar = read(doctype, 10, "", &mut NoLoader).expect("a grammar");
        assert!(grammar.element(&Name::local("a")).is_some());
        let at = grammar.element(&Name::local("r")).map(|e| e.origin.range.clone()).expect("r");
        assert_eq!(doctype.get(at.start - 10..at.end - 10), Some("<!ELEMENT r (a)>"), "places are those of the document");
        assert_eq!(grammar.root, Some(Name::local("r")));
    }

    #[test]
    fn a_doctype_without_declarations_or_that_is_too_big_has_no_grammar_to_speak_of() {
        assert!(read("<!DOCTYPE r>", 0, "", &mut NoLoader).is_some_and(|g| g.elements().is_empty()));
        assert!(read(&format!("<!DOCTYPE r [{}]>", " ".repeat(MAX_DOCTYPE_BYTES)), 0, "", &mut NoLoader).is_none());
        assert!(read("not a doctype", 0, "", &mut NoLoader).is_none());
        // Far into the document, the places in the grammar could not be those of the document.
        assert!(read("<!DOCTYPE r>", MAX_DOCTYPE_BYTES + 1, "", &mut NoLoader).is_none());
    }

    #[test]
    fn the_key_changes_with_the_text_and_the_place() {
        let a = key_of("<!DOCTYPE r>", None);
        assert_eq!(a, key_of("<!DOCTYPE r>", None));
        assert_ne!(a, key_of("<!DOCTYPE s>", None));
        assert_ne!(a, key_of("<!DOCTYPE r>", Some(Path::new("/x/y.xml"))));
    }

    #[test]
    fn the_reader_answers_the_request_in_the_background() {
        let schemas = Schemas::inline(Arc::new(|| Box::new(NoLoader)));
        schemas.submit(3, 77, "<!DOCTYPE r [<!ELEMENT r ANY>]>".to_owned(), 0, None);
        let results = schemas.poll();
        assert_eq!(results.len(), 1);
        let (document, schema) = &results[0];
        assert_eq!((*document, schema.key, schema.grammar.is_some()), (3, 77, true));
    }
}
