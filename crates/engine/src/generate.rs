//! Generating: a document from a schema, and a schema from documents. The work behind the Generate Sample and Generate Schema dialogs,
//! which run on a worker thread so that the window does not wait, and behind the tool `xml_sample`.

use std::sync::Arc;

use xmlcraft_dtd::{LoadRequest, Loaded, SubsetLoader, read_doctype};
use xmlcraft_infer::{Generated, Options as InferOptions, infer};
use xmlcraft_resolve::Resolver;
use xmlcraft_sample::{Options as SampleOptions, QName, dtd_roots, sample_from_dtd, sample_from_schema, schema_roots};
use xmlcraft_syntax::{Severity, parse};
use xmlcraft_text::Buffer;
use xmlcraft_xsd::{ResolvingLoader, XSD_NAMESPACE, assemble, dom, load};

use crate::validation::{LoaderFactory, OpenDocuments, ResolverFactory};
use crate::worker::Worker;

/// The most problems of a schema that are named when it is refused.
const MAX_REPORTED: usize = 5;

/// What a grammar is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrammarKind {
    Xsd,
    Dtd,
}

impl GrammarKind {
    pub fn name(self) -> &'static str {
        match self {
            GrammarKind::Xsd => "xsd",
            GrammarKind::Dtd => "dtd",
        }
    }
}

/// What kind of grammar `text` is: an XML Schema when it is well-formed and its root element is `schema` of the XML Schema namespace,
/// else a DTD.
pub fn grammar_kind(text: &str) -> GrammarKind {
    let parsed = parse(text);
    if parsed.diagnostics().iter().any(|d| d.severity() == Severity::Error) {
        return GrammarKind::Dtd;
    }
    let tree = dom::build(&parsed.syntax());
    if tree.elements.first().is_some_and(|root| root.local == "schema" && root.namespace.as_deref() == Some(XSD_NAMESPACE)) {
        GrammarKind::Xsd
    } else {
        GrammarKind::Dtd
    }
}

/// A sample document, and what it was made from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleDone {
    pub kind: GrammarKind,
    pub text: String,
    /// What could not be made as asked.
    pub warnings: Vec<String>,
    /// The element the document starts with.
    pub root: Option<String>,
    /// The elements it could start with (a namespaced name is `{namespace}name`).
    pub roots: Vec<String>,
}

/// `{namespace}local` or `local` as a name.
fn qname_of(text: &str) -> QName {
    match text.strip_prefix('{').and_then(|rest| rest.split_once('}')) {
        Some((namespace, local)) => QName::new(Some(namespace), local),
        None => QName::new(None, text),
    }
}

/// Reads the text of a DTD as the external subset of a document the first time it is asked for one, and what `files` has after that (the
/// parameter entities the DTD refers to).
struct OwnDtd<'a> {
    text: Option<String>,
    location: String,
    files: &'a mut dyn SubsetLoader,
}

impl SubsetLoader for OwnDtd<'_> {
    fn load(&mut self, request: &LoadRequest<'_>) -> Result<Loaded, String> {
        match self.text.take() {
            Some(text) => Ok(Loaded { text, location: self.location.clone() }),
            None => self.files.load(request),
        }
    }
}

/// A document made from the grammar `grammar` (the text of an XML Schema or of a DTD, which is at `location` when it is a file: what
/// relative references in it are taken from). `resolver` finds the schema documents it includes and imports, `subsets` the files a DTD
/// refers to. A grammar with errors is refused, naming the first few.
pub fn sample_document(
    grammar: &str,
    location: &str,
    kind: Option<GrammarKind>,
    root: Option<&str>,
    options: &SampleOptions,
    resolver: &Resolver,
    subsets: &mut dyn SubsetLoader,
) -> Result<SampleDone, String> {
    let kind = kind.unwrap_or_else(|| grammar_kind(grammar));
    match kind {
        GrammarKind::Xsd => {
            let mut loader = ResolvingLoader::new(resolver);
            let documents = load(grammar, location, &mut loader);
            let assembly = assemble(&documents);
            let errors: Vec<String> = documents
                .diagnostics
                .iter()
                .chain(&assembly.diagnostics)
                .filter(|d| d.diagnostic.severity() == Severity::Error)
                .map(|d| d.diagnostic.message())
                .collect();
            if !errors.is_empty() {
                let shown: Vec<&str> = errors.iter().take(MAX_REPORTED).map(String::as_str).collect();
                return Err(format!("the schema has {} error(s), so no document is made from it: {}", errors.len(), shown.join(" ")));
            }
            let wanted = root.map(qname_of);
            let made = sample_from_schema(&assembly.schema, wanted.as_ref(), options).map_err(|e| e.to_string())?;
            let roots: Vec<String> = schema_roots(&assembly.schema).iter().map(|q| q.to_string()).collect();
            Ok(SampleDone {
                kind,
                text: made.text,
                warnings: made.warnings,
                root: wanted.map(|q| q.to_string()).or_else(|| roots.first().cloned()),
                roots,
            })
        }
        GrammarKind::Dtd => {
            let mut loader = OwnDtd {
                text: Some(grammar.to_owned()),
                location: if location.is_empty() { "the DTD".to_owned() } else { location.to_owned() },
                files: subsets,
            };
            let probe = "<!DOCTYPE r SYSTEM \"dtd\"><r/>";
            let parsed = parse(probe);
            let Some((read, _)) = read_doctype(&parsed.syntax(), probe, location, &mut loader) else {
                return Err("the DTD could not be read".to_owned());
            };
            let errors: Vec<String> =
                read.diagnostics.iter().filter(|d| d.diagnostic.severity() == Severity::Error).map(|d| d.diagnostic.message()).collect();
            if !errors.is_empty() {
                let shown: Vec<&str> = errors.iter().take(MAX_REPORTED).map(String::as_str).collect();
                return Err(format!("the DTD has {} error(s), so no document is made from it: {}", errors.len(), shown.join(" ")));
            }
            let made = sample_from_dtd(&read.dtd, root, options).map_err(|e| e.to_string())?;
            let roots = dtd_roots(&read.dtd);
            Ok(SampleDone { kind, text: made.text, warnings: made.warnings, root: root.map(str::to_owned).or_else(|| roots.first().cloned()), roots })
        }
    }
}

// --- the workers ---------------------------------------------------------------------------------------------------------------

/// One request for a schema.
pub struct SchemaJob {
    /// Says which request a result answers.
    pub serial: u64,
    pub documents: Vec<String>,
    pub options: InferOptions,
}

pub struct SchemaOutcome {
    pub serial: u64,
    pub result: Result<Generated, String>,
}

/// Generates schemas on a worker thread. `submit` returns at once; `poll` collects what has finished.
pub struct SchemaGenerator {
    worker: Worker<SchemaJob, SchemaOutcome>,
}

fn run_schema(job: SchemaJob) -> SchemaOutcome {
    let refs: Vec<&str> = job.documents.iter().map(String::as_str).collect();
    let result = match crate::guard(|| infer(&refs, &job.options)) {
        Ok(Ok(generated)) => Ok(generated),
        Ok(Err(e)) => Err(e.to_string()),
        Err(panic) => {
            log::error!("generating a schema failed: {panic}");
            Err("generating the schema failed (a bug); nothing was changed".to_owned())
        }
    };
    SchemaOutcome { serial: job.serial, result }
}

impl SchemaGenerator {
    pub fn new() -> Self {
        Self { worker: Worker::new("schema generation", run_schema) }
    }

    /// A generator with no thread: `submit` generates in the caller's thread (what the web does, and handy for tests).
    pub fn inline() -> Self {
        Self { worker: Worker::inline(run_schema) }
    }

    pub fn on_done(&self, notify: impl Fn() + Send + Sync + 'static) {
        self.worker.on_done(notify);
    }

    pub fn submit(&self, job: SchemaJob) {
        self.worker.submit(0, job);
    }

    pub fn poll(&self) -> Vec<SchemaOutcome> {
        self.worker.poll().into_iter().map(|(_, outcome)| outcome).collect()
    }
}

impl Default for SchemaGenerator {
    fn default() -> Self {
        Self::new()
    }
}

/// One request for a sample document.
pub struct SampleJob {
    pub serial: u64,
    /// The text of the schema or the DTD, and where it is (empty when it has no file).
    pub grammar: String,
    pub location: String,
    /// The documents that are open, by their paths, so that a schema being edited is used as it is.
    pub open: Vec<(String, Buffer)>,
    pub kind: Option<GrammarKind>,
    pub root: Option<String>,
    pub options: SampleOptions,
}

pub struct SampleOutcome {
    pub serial: u64,
    pub result: Result<SampleDone, String>,
}

/// Generates sample documents on a worker thread.
pub struct SampleGenerator {
    worker: Worker<SampleJob, SampleOutcome>,
}

fn run_sample(loaders: &LoaderFactory, resolvers: &ResolverFactory, job: SampleJob) -> SampleOutcome {
    let mut subsets = loaders();
    let resolver = resolvers(Arc::new(OpenDocuments::new(job.open.clone())));
    let result =
        match crate::guard(|| sample_document(&job.grammar, &job.location, job.kind, job.root.as_deref(), &job.options, &resolver, subsets.as_mut()))
        {
            Ok(done) => done,
            Err(panic) => {
                log::error!("generating a sample failed: {panic}");
                Err("generating the sample failed (a bug); nothing was changed".to_owned())
            }
        };
    SampleOutcome { serial: job.serial, result }
}

impl SampleGenerator {
    /// A generator whose requests read the files of a DTD with the loaders `loaders` makes and find schema documents with the resolvers
    /// `resolvers` makes (as [`crate::validation::Validator`] does).
    pub fn new(loaders: LoaderFactory, resolvers: ResolverFactory) -> Self {
        Self { worker: Worker::new("sample generation", move |job| run_sample(&loaders, &resolvers, job)) }
    }

    pub fn inline(loaders: LoaderFactory, resolvers: ResolverFactory) -> Self {
        Self { worker: Worker::inline(move |job| run_sample(&loaders, &resolvers, job)) }
    }

    pub fn on_done(&self, notify: impl Fn() + Send + Sync + 'static) {
        self.worker.on_done(notify);
    }

    pub fn submit(&self, job: SampleJob) {
        self.worker.submit(0, job);
    }

    pub fn poll(&self) -> Vec<SampleOutcome> {
        self.worker.poll().into_iter().map(|(_, outcome)| outcome).collect()
    }
}

#[cfg(test)]
mod tests {
    use xmlcraft_dtd::NoLoader;
    use xmlcraft_infer::{Layout, Output};
    use xmlcraft_resolve::{Environment, FileAccess, FixedClock, Policy};

    use super::*;

    struct NoFiles;

    impl FileAccess for NoFiles {
        fn read(&self, path: &str) -> Result<Vec<u8>, String> {
            Err(format!("{path}: no such file"))
        }
    }

    fn resolvers() -> ResolverFactory {
        Arc::new(|buffers| {
            let env = Environment { buffers, files: Arc::new(NoFiles), fetcher: None, cache: None, clock: Arc::new(FixedClock(0)) };
            Resolver::new(env, Policy { offline: true, ..Policy::default() }, &[])
        })
    }

    fn resolver() -> Resolver {
        resolvers()(Arc::new(OpenDocuments::new(Vec::new())))
    }

    const XSD: &str = "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"><xs:element name=\"a\"><xs:complexType><xs:sequence><xs:element name=\"b\" type=\"xs:int\"/></xs:sequence></xs:complexType></xs:element><xs:element name=\"z\" type=\"xs:string\"/></xs:schema>";
    const DTD: &str = "<!ELEMENT a (b)>\n<!ELEMENT b (#PCDATA)>\n";

    #[test]
    fn a_grammar_is_an_xml_schema_when_it_says_so_and_a_dtd_otherwise() {
        assert_eq!(grammar_kind(XSD), GrammarKind::Xsd);
        assert_eq!(grammar_kind(DTD), GrammarKind::Dtd);
        assert_eq!(grammar_kind("<schema/>"), GrammarKind::Dtd, "a schema element of no namespace is not one");
        assert_eq!(grammar_kind("<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\">"), GrammarKind::Dtd, "not well-formed");
        assert_eq!(grammar_kind(""), GrammarKind::Dtd);
    }

    #[test]
    fn a_sample_is_made_from_either_kind_and_names_its_roots() {
        let options = SampleOptions::default();
        let xsd = sample_document(XSD, "", None, None, &options, &resolver(), &mut NoLoader).expect("a sample");
        assert_eq!((xsd.kind, xsd.root.as_deref(), xsd.roots.clone()), (GrammarKind::Xsd, Some("a"), vec!["a".to_owned(), "z".to_owned()]));
        assert!(xsd.text.contains("<b>1</b>"), "{}", xsd.text);
        let z = sample_document(XSD, "", None, Some("z"), &options, &resolver(), &mut NoLoader).expect("a sample");
        assert_eq!(z.root.as_deref(), Some("z"));
        let dtd = sample_document(DTD, "", None, None, &options, &resolver(), &mut NoLoader).expect("a sample");
        assert_eq!((dtd.kind, dtd.root.as_deref(), dtd.roots), (GrammarKind::Dtd, Some("a"), vec!["a".to_owned(), "b".to_owned()]));
        // The kind can be told, which decides what the text is read as.
        assert!(sample_document(XSD, "", Some(GrammarKind::Dtd), None, &options, &resolver(), &mut NoLoader).is_err());
    }

    #[test]
    fn a_grammar_with_errors_is_refused_and_names_them() {
        let broken = "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"><xs:element name=\"a\" type=\"nothing\"/></xs:schema>";
        let err = sample_document(broken, "", None, None, &SampleOptions::default(), &resolver(), &mut NoLoader).expect_err("broken");
        assert!(err.contains("1 error") && err.contains("nothing"), "{err}");
        let err = sample_document("<!ELEMENT a (b", "", Some(GrammarKind::Dtd), None, &SampleOptions::default(), &resolver(), &mut NoLoader)
            .expect_err("broken");
        assert!(err.contains("error"), "{err}");
    }

    #[test]
    fn a_schema_generator_answers_the_request_it_was_given() {
        let generator = SchemaGenerator::inline();
        let options = InferOptions { output: Output::Xsd { layout: Layout::VenetianBlind }, ..InferOptions::default() };
        generator.submit(SchemaJob { serial: 7, documents: vec!["<a><b/></a>".to_owned()], options });
        let got = generator.poll();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].serial, 7);
        let files = &got[0].result.as_ref().expect("generated").files;
        assert!(files[0].text.contains("aType"), "{}", files[0].text);
        generator.submit(SchemaJob { serial: 8, documents: vec!["<a>".to_owned()], options: InferOptions::default() });
        let got = generator.poll();
        assert_eq!(got[0].serial, 8);
        assert!(got[0].result.as_ref().expect_err("not well-formed").contains("Document 1 is not well-formed"));
    }

    #[test]
    fn a_sample_generator_uses_an_open_schema_and_says_which_request_it_answers() {
        let generator = SampleGenerator::inline(Arc::new(|| Box::new(NoLoader)), resolvers());
        let main = "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"><xs:include schemaLocation=\"part.xsd\"/><xs:element name=\"r\" type=\"t\"/></xs:schema>";
        let part = "<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\"><xs:simpleType name=\"t\"><xs:restriction base=\"xs:string\"><xs:enumeration value=\"from the open part\"/></xs:restriction></xs:simpleType></xs:schema>";
        let job = |serial: u64, open: Vec<(String, Buffer)>| SampleJob {
            serial,
            grammar: main.to_owned(),
            location: "/work/main.xsd".to_owned(),
            open,
            kind: None,
            root: None,
            options: SampleOptions::default(),
        };
        generator.submit(job(3, vec![("/work/part.xsd".to_owned(), Buffer::from_text(part))]));
        let got = generator.poll();
        assert_eq!(got[0].serial, 3);
        assert!(got[0].result.as_ref().expect("made").text.contains("from the open part"));
        // Without the part open, it is not found, which the answer says.
        generator.submit(job(4, Vec::new()));
        let got = generator.poll();
        assert_eq!(got[0].serial, 4);
        assert!(got[0].result.as_ref().expect_err("part missing").contains("error"));
    }
}
