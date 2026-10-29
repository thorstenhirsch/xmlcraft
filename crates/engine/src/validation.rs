//! What is known about a document against its DTD or its XML Schema: whether it is valid, and the problems if it is not.
//!
//! A document with a DOCTYPE is checked against the grammar of its DTD. One with none that names a schema (`xsi:schemaLocation` or
//! `xsi:noNamespaceSchemaLocation` on its root element) is checked against that XML Schema, which is loaded, with the documents it
//! includes and imports, through a [`SchemaLoader`]. [`validate`] is pure and total, given a way to read the external parts of the DTD
//! and the schema documents. [`Validator`] runs it on a worker thread so that
//! asking for it (F8) never makes the window wait: a request replaces any older one for the same document, a panic becomes a failed
//! result, and a result says which version of the text it is about.

use std::path::Path;
use std::sync::Arc;

use xmlcraft_dtd::SubsetLoader;
use xmlcraft_resolve::{Buffers, Resolver};
use xmlcraft_syntax::{Code, Diagnostic, Severity, SyntaxKind, SyntaxNode, parse};
use xmlcraft_text::Buffer;
use xmlcraft_validate::{SchemaOutcome, check_against_schema, schema_hints};
use xmlcraft_xsd::{ResolvingLoader, SchemaLoader};

use crate::analysis::{MAX_ANALYSED_BYTES, Skipped};
use crate::worker::Worker;

/// The most problems kept.
pub const MAX_PROBLEMS: usize = 10_000;

/// One thing found wrong, or worth saying, about a document and its DTD.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidityProblem {
    /// What, and where: a byte range of the document, or of the external text named by `file`.
    pub diagnostic: Diagnostic,
    /// The external text of the DTD, or the schema document, the problem is in (where it was read from); none when it is in the document.
    pub file: Option<String>,
}

/// How the validation of a document came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Well-formed, and no validity error (there may be notes).
    Valid,
    /// Well-formed, and at least one validity error.
    Invalid,
    /// Not well-formed: it is not validated (the analysis lists what is wrong).
    NotWellFormed,
    /// Not validated, and why.
    Skipped(Skipped),
}

/// The result of validating one version of a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Validation {
    /// The version of the text this is about (`Editor::version`).
    pub version: u64,
    pub verdict: Verdict,
    /// The validity problems, and the notes about the DTD (a part of it that could not be read), document first, in order of position.
    pub problems: Vec<ValidityProblem>,
    /// There were more than [`MAX_PROBLEMS`].
    pub truncated: bool,
}

impl Validation {
    pub fn error_count(&self) -> usize {
        self.problems.iter().filter(|p| p.diagnostic.severity() == Severity::Error).count()
    }

    pub fn warning_count(&self) -> usize {
        self.problems.iter().filter(|p| p.diagnostic.severity() == Severity::Warning).count()
    }

    pub fn is_valid(&self) -> bool {
        self.verdict == Verdict::Valid
    }

    /// The problems that are in the document itself, in order of position.
    pub fn in_document(&self) -> impl Iterator<Item = &Diagnostic> {
        self.problems.iter().filter(|p| p.file.is_none()).map(|p| &p.diagnostic)
    }

    fn skipped(version: u64, why: Skipped) -> Self {
        Self { version, verdict: Verdict::Skipped(why), problems: Vec::new(), truncated: false }
    }
}

/// Validate `text` (version `version` of a document at `location`, which relative identifiers in its DOCTYPE, or the locations of its
/// schemas, are taken from; empty when it has none). `loader` reads the external parts of the DTD, `schemas` the schema documents.
pub fn validate(text: &str, version: u64, location: &str, loader: &mut dyn SubsetLoader, schemas: &mut dyn SchemaLoader) -> Validation {
    if text.len() > MAX_ANALYSED_BYTES {
        return Validation::skipped(version, Skipped::TooLarge);
    }
    let parsed = parse(text);
    let root = parsed.syntax();
    // A DOCTYPE is the document's own say; a schema is looked for only when there is none.
    let has_doctype = root.children().any(|c| c.kind() == SyntaxKind::Doctype);
    if !has_doctype && !schema_hints(&root).is_empty() {
        return validate_with_schema(&root, parsed.diagnostics(), version, location, schemas);
    }
    let found = xmlcraft_validate::check(&root, text, parsed.diagnostics(), location, loader);
    let well_formed = found.well_formedness.iter().all(|p| p.diagnostic.severity() != Severity::Error);
    if !well_formed {
        return Validation { version, verdict: Verdict::NotWellFormed, problems: Vec::new(), truncated: false };
    }
    let problems: Vec<ValidityProblem> = found
        .validity
        .iter()
        // What is not an error in the well-formedness list (a part of the DTD that was not read) is worth saying.
        .chain(found.well_formedness.iter())
        .map(|p| ValidityProblem { diagnostic: p.diagnostic.clone(), file: (p.source != 0).then(|| found.sources.get(p.source).cloned()).flatten() })
        .collect();
    finish(version, problems)
}

/// The check against the XML Schema the document names.
fn validate_with_schema(root: &SyntaxNode, syntax: &[Diagnostic], version: u64, location: &str, schemas: &mut dyn SchemaLoader) -> Validation {
    let found = check_against_schema(root, syntax, location, None, schemas);
    match &found.outcome {
        SchemaOutcome::NotWellFormed => Validation { version, verdict: Verdict::NotWellFormed, problems: Vec::new(), truncated: false },
        SchemaOutcome::NoSchema(why) => {
            // Where the schema is named: the root element's name.
            let span = root_name_span(root);
            let problem = ValidityProblem { diagnostic: Diagnostic::new(Code::XsdSchemaUnavailable, span, vec![why.clone()]), file: None };
            finish(version, vec![problem])
        }
        SchemaOutcome::Validated => {
            let own = found.validity.iter().chain(found.well_formedness.iter()).map(|d| ValidityProblem { diagnostic: d.clone(), file: None });
            let schema = found
                .schema_problems
                .iter()
                .map(|p| ValidityProblem { diagnostic: p.diagnostic.clone(), file: found.sources.get(p.source).cloned() });
            finish(version, own.chain(schema).collect())
        }
    }
}

/// The byte range of the name of the root element (empty at the start when there is none).
fn root_name_span(root: &SyntaxNode) -> std::ops::Range<usize> {
    root.children()
        .find(|c| c.kind() == SyntaxKind::Element)
        .and_then(|e| e.children().find(|c| c.kind() == SyntaxKind::StartTag))
        .and_then(|t| t.children_with_tokens().filter_map(|c| c.into_token()).find(|t| t.kind() == SyntaxKind::Name))
        .map(|t| usize::from(t.text_range().start())..usize::from(t.text_range().end()))
        .unwrap_or(0..0)
}

/// Order the problems (the document's own first, by position; then those in the external texts, by file and position), cap them, and
/// say whether any is an error.
fn finish(version: u64, mut problems: Vec<ValidityProblem>) -> Validation {
    problems.sort_by(|a, b| {
        (a.file.is_some(), &a.file, a.diagnostic.span.start, a.diagnostic.span.end).cmp(&(
            b.file.is_some(),
            &b.file,
            b.diagnostic.span.start,
            b.diagnostic.span.end,
        ))
    });
    let truncated = problems.len() > MAX_PROBLEMS;
    problems.truncate(MAX_PROBLEMS);
    let invalid = problems.iter().any(|p| p.diagnostic.severity() == Severity::Error);
    Validation { version, verdict: if invalid { Verdict::Invalid } else { Verdict::Valid }, problems, truncated }
}

/// [`validate`], with a panic turned into a failed result (the last-resort guard: a bug must not take the app down).
pub fn validate_guarded(text: &str, version: u64, location: &str, loader: &mut dyn SubsetLoader, schemas: &mut dyn SchemaLoader) -> Validation {
    crate::guard(|| validate(text, version, location, loader, schemas)).unwrap_or_else(|message| {
        log::error!("validation failed: {message}");
        Validation::skipped(version, Skipped::Failed)
    })
}

/// Makes the loader for one validation (the worker asks for one per request, in its own thread).
pub type LoaderFactory = Arc<dyn Fn() -> Box<dyn SubsetLoader> + Send + Sync>;

/// Makes the resolver the schema documents of one validation are found with. It is given the documents that are open, so that a
/// schema being edited is validated against as it is, not as it was saved.
pub type ResolverFactory = Arc<dyn Fn(Arc<dyn Buffers>) -> Resolver + Send + Sync>;

/// The documents that are open, by their paths (as they were when a validation was asked for).
struct OpenDocuments(Vec<(String, Buffer)>);

impl Buffers for OpenDocuments {
    fn text(&self, location: &str) -> Option<Arc<str>> {
        self.0.iter().find(|(l, _)| l == location).map(|(_, buffer)| Arc::from(buffer.text().as_str()))
    }
}

struct Request {
    version: u64,
    text: Buffer,
    location: String,
    open: Vec<(String, Buffer)>,
}

/// Runs [`validate`] on a worker thread. `submit` returns at once; `poll` collects what has finished.
pub struct Validator {
    worker: Worker<Request, Validation>,
}

impl Validator {
    /// A validator whose requests read the external parts of DTDs with the loaders `loaders` makes, and find schema documents with the
    /// resolvers `resolvers` makes.
    pub fn new(loaders: LoaderFactory, resolvers: ResolverFactory) -> Self {
        Self { worker: Worker::new("validation", move |request| run(&loaders, &resolvers, request)) }
    }

    /// A validator with no thread: `submit` validates in the caller's thread (what the web does, and handy for tests).
    pub fn inline(loaders: LoaderFactory, resolvers: ResolverFactory) -> Self {
        Self { worker: Worker::inline(move |request| run(&loaders, &resolvers, request)) }
    }

    /// Called whenever a result is ready (the app asks for a repaint).
    pub fn on_done(&self, notify: impl Fn() + Send + Sync + 'static) {
        self.worker.on_done(notify);
    }

    /// Ask for `text` (version `version` of the document with this id, saved at `path` if it is) to be validated. `open` are the
    /// documents that are open, by their paths. A request that has not started is replaced by a newer one for the same document.
    pub fn submit(&self, document: u64, version: u64, text: &Buffer, path: Option<&Path>, open: Vec<(String, Buffer)>) {
        let location = path.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        self.worker.submit(document, Request { version, text: text.clone(), location, open });
    }

    /// The validations that finished since the last call, oldest first.
    pub fn poll(&self) -> Vec<(u64, Validation)> {
        self.worker.poll()
    }
}

fn run(loaders: &LoaderFactory, resolvers: &ResolverFactory, request: Request) -> Validation {
    let mut loader = loaders();
    let resolver = resolvers(Arc::new(OpenDocuments(request.open)));
    let mut schemas = ResolvingLoader::new(&resolver);
    validate_guarded(&request.text.text(), request.version, &request.location, loader.as_mut(), &mut schemas)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use xmlcraft_dtd::{LoadRequest, Loaded, NoLoader};
    use xmlcraft_resolve::{Environment, FileAccess, FixedClock, Policy};
    use xmlcraft_syntax::Code;
    use xmlcraft_xsd::{LoadRequest as SchemaRequest, Loaded as SchemaLoaded};

    use super::*;

    /// No schema document can be found.
    struct NoSchemas;

    impl SchemaLoader for NoSchemas {
        fn load(&mut self, request: &SchemaRequest<'_>) -> Result<SchemaLoaded, String> {
            Err(format!("{}: no such file", request.location.unwrap_or("?")))
        }
    }

    /// Schema documents by the location they are asked for with.
    struct Schemas(HashMap<&'static str, String>);

    impl SchemaLoader for Schemas {
        fn load(&mut self, request: &SchemaRequest<'_>) -> Result<SchemaLoaded, String> {
            let wanted = request.location.unwrap_or_default();
            self.0
                .get(wanted)
                .map(|text| SchemaLoaded { text: text.clone(), location: format!("mem:{wanted}") })
                .ok_or_else(|| format!("{wanted}: no such file"))
        }
    }

    const XS: &str = "http://www.w3.org/2001/XMLSchema";
    const XSI: &str = "xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"";

    fn order_schema() -> String {
        format!(
            "<xs:schema xmlns:xs=\"{XS}\"><xs:element name=\"order\"><xs:complexType><xs:sequence><xs:element name=\"id\" type=\"xs:int\"/></xs:sequence></xs:complexType></xs:element></xs:schema>"
        )
    }

    fn named(body: &str) -> String {
        format!("<order {XSI} xsi:noNamespaceSchemaLocation=\"order.xsd\">{body}</order>")
    }

    struct Files(HashMap<String, String>);

    impl SubsetLoader for Files {
        fn load(&mut self, request: &LoadRequest<'_>) -> Result<Loaded, String> {
            self.0
                .get(request.system)
                .map(|text| Loaded { text: text.clone(), location: format!("mem:{}", request.system) })
                .ok_or_else(|| "not there".to_owned())
        }
    }

    fn codes(v: &Validation) -> Vec<Code> {
        v.problems.iter().map(|p| p.diagnostic.code).collect()
    }

    struct NoFiles;

    impl FileAccess for NoFiles {
        fn read(&self, path: &str) -> Result<Vec<u8>, String> {
            Err(format!("{path}: no such file"))
        }
    }

    fn no_resolvers() -> ResolverFactory {
        Arc::new(|buffers| {
            let env = Environment { buffers, files: Arc::new(NoFiles), fetcher: None, cache: None, clock: Arc::new(FixedClock(0)) };
            Resolver::new(env, xmlcraft_resolve::Policy { offline: true, ..Policy::default() }, &[])
        })
    }

    #[test]
    fn a_document_that_names_a_schema_is_checked_against_it() {
        let mut schemas = Schemas(HashMap::from([("order.xsd", order_schema())]));
        let valid = validate(&named("<id>7</id>"), 1, "", &mut NoLoader, &mut schemas);
        assert_eq!((valid.verdict, valid.problems.len()), (Verdict::Valid, 0), "{:?}", codes(&valid));
        let text = named("<id>seven</id>");
        let invalid = validate(&text, 2, "", &mut NoLoader, &mut schemas);
        assert_eq!(invalid.verdict, Verdict::Invalid);
        assert_eq!(codes(&invalid), [Code::CvcInvalidValue]);
        assert_eq!((invalid.error_count(), invalid.in_document().count()), (1, 1));
        // The problem is at the element it is about.
        let at = invalid.problems[0].diagnostic.span.clone();
        assert_eq!(text.get(at), Some("id"));
    }

    #[test]
    fn a_doctype_comes_before_a_schema_the_root_names() {
        let text = format!("<!DOCTYPE order [<!ELEMENT order EMPTY>]><order {XSI} xsi:noNamespaceSchemaLocation=\"order.xsd\"/>");
        let mut schemas = Schemas(HashMap::from([("order.xsd", order_schema())]));
        let v = validate(&text, 1, "", &mut NoLoader, &mut schemas);
        // The DTD decided: it does not declare the two attributes, which the schema would not have minded (it would have found `id` missing).
        assert_eq!(codes(&v), [Code::VcAttributeNotDeclared, Code::VcAttributeNotDeclared]);
    }

    #[test]
    fn a_schema_that_cannot_be_loaded_is_a_problem_at_the_root_and_not_a_pass() {
        let text = named("<id>7</id>");
        let v = validate(&text, 1, "", &mut NoLoader, &mut NoSchemas);
        assert_eq!(v.verdict, Verdict::Invalid);
        assert_eq!(codes(&v), [Code::XsdSchemaUnavailable]);
        let problem = &v.problems[0];
        assert_eq!(text.get(problem.diagnostic.span.clone()), Some("order"));
        assert!(
            problem.diagnostic.message().contains("order.xsd") && problem.diagnostic.message().contains("no such file"),
            "{}",
            problem.diagnostic.message()
        );
    }

    #[test]
    fn the_problems_of_a_schema_name_its_file_and_come_after_the_documents() {
        let schema = format!(
            "<xs:schema xmlns:xs=\"{XS}\"><xs:element name=\"order\" type=\"nothing\"/><xs:element name=\"b\" type=\"missing\"/></xs:schema>"
        );
        let mut schemas = Schemas(HashMap::from([("order.xsd", schema)]));
        let v = validate(&named("<id>7</id>"), 1, "", &mut NoLoader, &mut schemas);
        assert_eq!(v.verdict, Verdict::Invalid, "a schema with errors is not a pass");
        assert!(codes(&v).contains(&Code::XsdUnresolved));
        let files: Vec<Option<&str>> = v.problems.iter().map(|p| p.file.as_deref()).collect();
        assert!(files.contains(&Some("mem:order.xsd")), "{files:?}");
        // The ones in the document (none, or first) come before those in the schema.
        assert!(files.windows(2).all(|w| w[0].is_none() || w[1].is_some()), "{files:?}");
    }

    #[test]
    fn a_note_about_the_document_is_kept_when_it_is_checked_against_a_schema() {
        let mut schemas = Schemas(HashMap::from([("order.xsd", order_schema())]));
        let text = format!("<?xml version=\"1.1\"?>{}", named("<id>7</id>"));
        let v = validate(&text, 1, "", &mut NoLoader, &mut schemas);
        assert_eq!(codes(&v), [Code::UnsupportedVersion]);
        assert_eq!((v.verdict, v.error_count(), v.warning_count()), (Verdict::Valid, 0, 1));
    }

    #[test]
    fn a_document_that_is_not_well_formed_is_not_checked_against_its_schema() {
        let mut schemas = Schemas(HashMap::from([("order.xsd", order_schema())]));
        let v = validate(&named("<id>7</order>"), 1, "", &mut NoLoader, &mut schemas);
        assert_eq!((v.verdict, v.problems.len()), (Verdict::NotWellFormed, 0));
    }

    #[test]
    fn the_validator_finds_a_schema_in_an_open_document_as_it_is_edited_not_as_it_was_saved() {
        // Nothing is on disk: the schema is only in a buffer, at the path the document names it by.
        let validator = Validator::inline(Arc::new(|| Box::new(NoLoader)), no_resolvers());
        let schema = Buffer::from_text(&order_schema());
        let doc = Buffer::from_text(&named("<id>7</id>"));
        validator.submit(1, 1, &doc, Some(Path::new("/work/doc.xml")), vec![("/work/order.xsd".to_owned(), schema)]);
        let got = validator.poll();
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].1.verdict, got[0].1.problems.len()), (Verdict::Valid, 0), "{:?}", codes(&got[0].1));
        // The same document with no schema open is a problem.
        validator.submit(1, 2, &doc, Some(Path::new("/work/doc.xml")), Vec::new());
        let got = validator.poll();
        assert_eq!(codes(&got[0].1), [Code::XsdSchemaUnavailable]);
    }

    #[test]
    fn a_valid_document_is_valid_and_an_invalid_one_lists_its_problems_by_position() {
        let valid = validate("<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY>]><r><a/></r>", 3, "", &mut NoLoader, &mut NoSchemas);
        assert_eq!((valid.version, valid.verdict, valid.problems.len(), valid.is_valid()), (3, Verdict::Valid, 0, true));
        let text = "<!DOCTYPE r [<!ELEMENT r (a)><!ELEMENT a EMPTY>]><r><b/><a>x</a></r>";
        let invalid = validate(text, 4, "", &mut NoLoader, &mut NoSchemas);
        assert_eq!(invalid.verdict, Verdict::Invalid);
        assert_eq!(codes(&invalid), [Code::VcElementNotDeclared, Code::VcEmptyHasContent]);
        assert!(invalid.problems.windows(2).all(|w| w[0].diagnostic.span.start <= w[1].diagnostic.span.start));
        assert_eq!((invalid.error_count(), invalid.in_document().count()), (2, 2));
    }

    #[test]
    fn a_document_that_is_not_well_formed_is_not_validated() {
        let v = validate("<!DOCTYPE r [<!ELEMENT r EMPTY>]><r><x></r>", 1, "", &mut NoLoader, &mut NoSchemas);
        assert_eq!((v.verdict, v.problems.len()), (Verdict::NotWellFormed, 0));
    }

    #[test]
    fn a_document_without_a_doctype_is_not_valid_and_says_why() {
        let v = validate("<r/>", 1, "", &mut NoLoader, &mut NoSchemas);
        assert_eq!((v.verdict, codes(&v)), (Verdict::Invalid, vec![Code::VcNoDtd]));
    }

    #[test]
    fn problems_in_the_external_subset_name_its_file_and_come_after_the_documents() {
        let mut files = Files(HashMap::new());
        files.0.insert("r.dtd".into(), "<!ELEMENT r ANY>\n<!ATTLIST r a ID #FIXED 'x'>".into());
        let v = validate("<!DOCTYPE r SYSTEM 'r.dtd'><r><undeclared/></r>", 1, "mem:doc.xml", &mut files, &mut NoSchemas);
        assert_eq!(codes(&v), [Code::VcElementNotDeclared, Code::VcIdAttributeDefault]);
        assert_eq!(v.problems.iter().map(|p| p.file.as_deref()).collect::<Vec<_>>(), [None, Some("mem:r.dtd")]);
        assert_eq!(v.in_document().count(), 1);
    }

    #[test]
    fn a_part_of_the_dtd_that_cannot_be_read_is_a_note_and_not_an_error() {
        let v = validate("<!DOCTYPE r SYSTEM 'missing.dtd'><r><x/></r>", 1, "", &mut NoLoader, &mut NoSchemas);
        assert_eq!(v.verdict, Verdict::Valid);
        assert_eq!(codes(&v), [Code::DtdExternalNotRead]);
        assert_eq!((v.error_count(), v.warning_count()), (0, 1));
    }

    #[test]
    fn a_text_that_is_too_large_is_skipped() {
        let big = format!("<r>{}</r>", "a".repeat(MAX_ANALYSED_BYTES));
        assert_eq!(validate(&big, 2, "", &mut NoLoader, &mut NoSchemas).verdict, Verdict::Skipped(Skipped::TooLarge));
    }

    #[test]
    fn the_validator_runs_in_the_background_with_a_loader_of_its_own_per_request() {
        let validator = Validator::new(
            Arc::new(|| {
                let mut files = HashMap::new();
                files.insert("r.dtd".to_owned(), "<!ELEMENT r EMPTY>".to_owned());
                Box::new(Files(files))
            }),
            no_resolvers(),
        );
        let (tx, rx) = std::sync::mpsc::channel();
        validator.on_done(move || {
            let _ = tx.send(());
        });
        validator.submit(1, 1, &Buffer::from_text("<!DOCTYPE r SYSTEM 'r.dtd'><r/>"), None, Vec::new());
        validator.submit(1, 2, &Buffer::from_text("<!DOCTYPE r SYSTEM 'r.dtd'><r>text</r>"), None, Vec::new());
        let mut newest = None;
        while newest.is_none() {
            rx.recv_timeout(std::time::Duration::from_secs(10)).expect("a result arrives");
            newest = validator.poll().into_iter().find(|(d, v)| *d == 1 && v.version == 2).map(|(_, v)| v);
        }
        assert_eq!(newest.map(|v| v.verdict), Some(Verdict::Invalid));
    }

    #[test]
    fn a_panic_is_a_failed_result() {
        struct Panics;
        impl SubsetLoader for Panics {
            #[allow(clippy::panic)]
            fn load(&mut self, _: &LoadRequest<'_>) -> Result<Loaded, String> {
                panic!("a bug in a loader")
            }
        }
        let v = validate_guarded("<!DOCTYPE r SYSTEM 'r.dtd'><r/>", 5, "", &mut Panics, &mut NoSchemas);
        assert_eq!((v.version, v.verdict), (5, Verdict::Skipped(Skipped::Failed)));
    }
}
