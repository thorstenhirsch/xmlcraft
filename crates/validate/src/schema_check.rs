//! The whole check of a document against a schema: the syntax, the schema the document names (or the one it is told to use), found
//! through a [`SchemaLoader`], assembled, and the document validated against it.

use xmlcraft_syntax::{Diagnostic, Severity, SyntaxNode};
use xmlcraft_xsd::{Assembly, LoadKind, LoadRequest, SchemaLoader, SourcedDiagnostic, XSD_NAMESPACE, assemble, load};

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

/// The schema a document names (or is told to use), loaded and assembled.
#[derive(Debug)]
pub struct LoadedSchema {
    pub assembly: Assembly,
    /// The problems of the schema documents (reading, loading and assembling); `source` indexes [`LoadedSchema::sources`].
    pub problems: Vec<SourcedDiagnostic>,
    /// The locations of the schema documents that were read.
    pub sources: Vec<String>,
    /// The schemas that were asked for.
    pub hints: Vec<SchemaHint>,
}

/// Why there is no schema to check or complete against, and which were asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoSchema {
    pub why: String,
    pub hints: Vec<SchemaHint>,
}

/// Find, load and assemble the schema a parsed document names with `xsi:schemaLocation` and `xsi:noNamespaceSchemaLocation`, or `explicit`
/// (a location relative to `location`, the place the document is) instead of them. `loader` finds the schema documents.
pub fn load_schema(root: &SyntaxNode, location: &str, explicit: Option<&str>, loader: &mut dyn SchemaLoader) -> Result<LoadedSchema, NoSchema> {
    let hints = match explicit {
        Some(location) => vec![SchemaHint { namespace: None, location: location.to_owned() }],
        None => schema_hints(root),
    };
    load_hinted(hints, location, loader)
}

/// Load and assemble the schemas `hints` ask for (as a document at `location` names them). One is the root of the load; several are
/// imported by a schema of no namespace that is made here; none is `NoSchema`.
pub fn load_hinted(hints: Vec<SchemaHint>, location: &str, loader: &mut dyn SchemaLoader) -> Result<LoadedSchema, NoSchema> {
    let documents = match hints.as_slice() {
        [] => {
            let why = "the document names no schema (xsi:schemaLocation or xsi:noNamespaceSchemaLocation) and none was given".to_owned();
            return Err(NoSchema { why, hints });
        }
        // One schema is the root of the load.
        [only] => {
            let request =
                LoadRequest { kind: LoadKind::Include, namespace: only.namespace.as_deref(), location: Some(&only.location), base: location };
            match loader.load(&request) {
                Ok(loaded) => load(&loaded.text, &loaded.location, loader),
                Err(why) => {
                    // (A hint with no location is a namespace to be looked up.)
                    let name = if only.location.is_empty() { only.namespace.as_deref().unwrap_or_default() } else { only.location.as_str() };
                    let why = format!("the schema '{name}' could not be loaded: {why}");
                    return Err(NoSchema { why, hints });
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
    let mut assembly = assemble(&documents);
    let mut problems: Vec<SourcedDiagnostic> = documents.diagnostics.clone();
    problems.append(&mut assembly.diagnostics);
    let sources = documents.list.iter().map(|d| d.location.clone()).collect();
    Ok(LoadedSchema { assembly, problems, sources, hints })
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
    let loaded = match load_schema(root, location, explicit, loader) {
        Ok(loaded) => loaded,
        Err(NoSchema { why, hints }) => return SchemaCheck::without_schema(syntax, SchemaOutcome::NoSchema(why), hints),
    };
    let validity = validate_against_schema(root, &loaded.assembly.schema);
    SchemaCheck {
        outcome: SchemaOutcome::Validated,
        well_formedness: syntax.to_vec(),
        validity,
        schema_problems: loaded.problems,
        sources: loaded.sources,
        schemas: loaded.hints,
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;")
}
