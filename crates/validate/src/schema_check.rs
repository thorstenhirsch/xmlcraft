//! The whole check of a document against a schema: the syntax, the schema the document names (or the one it is told to use), found
//! through a [`SchemaLoader`], assembled, and the document validated against it.

use xmlcraft_syntax::{Diagnostic, Severity, SyntaxNode};
use xmlcraft_xsd::{LoadKind, LoadRequest, SchemaLoader, SourcedDiagnostic, XSD_NAMESPACE, assemble, load};

use crate::xsd_document::{SchemaHint, schema_hints, validate_against_schema};

/// What came of checking a document against a schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaOutcome {
    /// The document was validated (the schema may have had problems of its own).
    Validated,
    /// The document is not well-formed, so it was not validated.
    NotWellFormed,
    /// There was no schema to validate against, and why.
    NoSchema(String),
}

/// What is wrong with a document and the schema it was checked against.
#[derive(Clone, Debug)]
pub struct SchemaCheck {
    pub outcome: SchemaOutcome,
    /// The problems of the document's syntax, as the parser found them.
    pub well_formedness: Vec<Diagnostic>,
    /// The validity problems of the document, in order of position.
    pub validity: Vec<Diagnostic>,
    /// The problems of the schema; `source` indexes [`SchemaCheck::sources`].
    pub schema_problems: Vec<SourcedDiagnostic>,
    /// The locations of the schema documents that were read.
    pub sources: Vec<String>,
    /// The schemas the document was checked against, as asked for.
    pub schemas: Vec<SchemaHint>,
}

impl SchemaCheck {
    fn without_schema(syntax: &[Diagnostic], outcome: SchemaOutcome, schemas: Vec<SchemaHint>) -> Self {
        Self { outcome, well_formedness: syntax.to_vec(), validity: Vec::new(), schema_problems: Vec::new(), sources: Vec::new(), schemas }
    }

    pub fn schema_errors(&self) -> usize {
        self.schema_problems.iter().filter(|p| p.diagnostic.severity() == Severity::Error).count()
    }
}

/// Check a parsed document against a schema. `explicit` is the location of the schema to use (relative to `location`, the place the
/// document is); without it, the schemas the document names itself are used. `loader` finds the schema documents.
pub fn check_against_schema(
    root: &SyntaxNode,
    syntax: &[Diagnostic],
    location: &str,
    explicit: Option<&str>,
    loader: &mut dyn SchemaLoader,
) -> SchemaCheck {
    if syntax.iter().any(|d| d.severity() == Severity::Error) {
        return SchemaCheck::without_schema(syntax, SchemaOutcome::NotWellFormed, Vec::new());
    }
    let schemas = match explicit {
        Some(location) => vec![SchemaHint { namespace: None, location: location.to_owned() }],
        None => schema_hints(root),
    };
    let documents = match schemas.as_slice() {
        [] => {
            let why = "the document names no schema (xsi:schemaLocation or xsi:noNamespaceSchemaLocation) and none was given".to_owned();
            return SchemaCheck::without_schema(syntax, SchemaOutcome::NoSchema(why), schemas);
        }
        // One schema is the root of the load.
        [only] => {
            let request =
                LoadRequest { kind: LoadKind::Include, namespace: only.namespace.as_deref(), location: Some(&only.location), base: location };
            match loader.load(&request) {
                Ok(loaded) => load(&loaded.text, &loaded.location, loader),
                Err(why) => {
                    let why = format!("the schema '{}' could not be loaded: {why}", only.location);
                    return SchemaCheck::without_schema(syntax, SchemaOutcome::NoSchema(why), schemas);
                }
            }
        }
        // Several are imported by a schema of no namespace that is made here.
        many => {
            let imports: String = many
                .iter()
                .map(|h| match &h.namespace {
                    Some(ns) => format!("<xs:import namespace=\"{}\" schemaLocation=\"{}\"/>", escape(ns), escape(&h.location)),
                    None => format!("<xs:include schemaLocation=\"{}\"/>", escape(&h.location)),
                })
                .collect();
            let wrapper = format!("<xs:schema xmlns:xs=\"{XSD_NAMESPACE}\">{imports}</xs:schema>");
            load(&wrapper, location, loader)
        }
    };
    let assembly = assemble(&documents);
    let mut schema_problems: Vec<SourcedDiagnostic> = documents.diagnostics.clone();
    schema_problems.extend(assembly.diagnostics);
    let validity = validate_against_schema(root, &assembly.schema);
    SchemaCheck {
        outcome: SchemaOutcome::Validated,
        well_formedness: syntax.to_vec(),
        validity,
        schema_problems,
        sources: documents.list.iter().map(|d| d.location.clone()).collect(),
        schemas,
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;")
}
