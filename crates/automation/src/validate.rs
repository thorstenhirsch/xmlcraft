//! `xml_validate`: a document checked against the DTD it names, or against an XML Schema (given, or named by `xsi:schemaLocation` /
//! `xsi:noNamespaceSchemaLocation` in a document that has no DOCTYPE). The external parts of a DTD and the schema documents are files
//! read through the root confinement (a relative identifier is taken from the folder of the document, a `file:` URI is a path), and
//! the schemas that ship with the app; nothing is downloaded.

use std::sync::Arc;

use serde_json::{Value, json};
use xmlcraft_dtd::DtdDiagnostic;
use xmlcraft_platform::subset::FileSubsetLoader;
use xmlcraft_resolve::{Buffers, Environment, FileAccess, FixedClock, Policy, Resolver};
use xmlcraft_syntax::{Diagnostic, Severity, SyntaxKind, SyntaxNode, parse};
use xmlcraft_text::Buffer;
use xmlcraft_validate::{SchemaOutcome, check, check_against_schema, schema_hints};
use xmlcraft_xsd::ResolvingLoader;

use crate::documents::{fail, json_result, position_json, severity_name, source, source_properties};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result};

/// The most problems one call lists.
const MAX_PROBLEMS: usize = 500;

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "xml_validate",
        title: "Validate against a DTD or an XML Schema",
        description: "Check a document (an open document `id`, a file `path`, or `text`) for well-formedness and, when it is well-formed, validity. With `schema` (a path of a \
                      schema document, relative paths from the folder of the document) or, in a document with no DOCTYPE, the schema its root element names with \
                      `xsi:schemaLocation` / `xsi:noNamespaceSchemaLocation`, it is validated against that XML Schema 1.0 (the schema documents it includes and imports \
                      are found as files, in the open documents, or among the schemas that ship with the app); otherwise against the DTD its DOCTYPE declares: the internal \
                      subset and the external subset and parameter entities that are files. Nothing is downloaded. A document with neither has nothing to be valid against. \
                      Returns the counts and the problems with their position and message; `kind` says whether a problem is about well-formedness, validity or the schema \
                      (`schema_errors` counts those), and `file` names the external text a problem is in; `schemas` lists the schema documents that were read.",
        input_schema: json!({
            "type": "object",
            "properties": validate_properties(),
            "additionalProperties": false,
        }),
        read_only: true,
        destructive: false,
        handler: xml_validate,
    }]
}

fn validate_properties() -> Value {
    let mut properties = source_properties();
    if let Some(map) = properties.as_object_mut() {
        map.insert(
            "schema".into(),
            json!({ "type": "string", "description": "A schema document to validate against (XML Schema), instead of the DTD or the schema the document names" }),
        );
    }
    properties
}

/// The files a schema may be read from: the ones the root confinement lets through.
struct ConfinedFiles {
    confinement: crate::file_access::Confinement,
}

impl FileAccess for ConfinedFiles {
    fn read(&self, path: &str) -> std::result::Result<Vec<u8>, String> {
        let real = self.confinement.resolve_read(path).map_err(|e| e.to_string())?;
        xmlcraft_platform::files::read_file(&real).map(|loaded| loaded.bytes).map_err(|e| format!("{}: {e}", real.display()))
    }
}

/// The documents that are open, by their paths: a schema that is being edited is validated against as it is.
struct OpenDocuments(Vec<(String, Arc<str>)>);

impl Buffers for OpenDocuments {
    fn text(&self, location: &str) -> Option<Arc<str>> {
        self.0.iter().find(|(l, _)| l == location).map(|(_, t)| t.clone())
    }
}

fn xml_validate(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let source = source(a, args)?;
    if source.text.len() > xmlcraft_engine::analysis::MAX_ANALYSED_BYTES {
        return fail("the text is too large to analyse");
    }
    let parsed = parse(&source.text);
    let schema = args.get("schema").and_then(Value::as_str);
    let root = parsed.syntax();
    let has_doctype = root.children().any(|c| c.kind() == SyntaxKind::Doctype);
    if schema.is_some() || (!has_doctype && !schema_hints(&root).is_empty()) {
        return validate_against_schema(a, &source, &root, parsed.diagnostics(), schema);
    }
    let confinement = a.confinement.clone();
    let mut loader = FileSubsetLoader::confined(move |path| confinement.resolve_read(path).map_err(|e| e.to_string()));
    let location = source.path.as_deref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let found = check(&parsed.syntax(), &source.text, parsed.diagnostics(), &location, &mut loader);
    let own = Buffer::from_text(&source.text);
    let buffers: Vec<Option<(String, Buffer)>> = found
        .sources
        .iter()
        .map(|location| loader.texts().iter().find(|(l, _)| l == location).map(|(l, text)| (l.clone(), Buffer::from_text(text))))
        .collect();
    let describe = |kind: &str, d: &DtdDiagnostic| -> Value {
        let (file, buffer) = match d.source {
            0 => (None, Some(&own)),
            n => match buffers.get(n).and_then(Option::as_ref) {
                Some((file, buffer)) => (Some(file.clone()), Some(buffer)),
                None => (found.sources.get(n).cloned(), None),
            },
        };
        let mut value = json!({
            "kind": kind,
            "severity": severity_name(d.diagnostic.severity()),
            "code": d.diagnostic.code.key(),
            "message": d.diagnostic.message(),
            "file": file,
            "start": d.diagnostic.span.start,
            "end": d.diagnostic.span.end,
        });
        if let Some(buffer) = buffer {
            let (line, column) = position_json(buffer, d.diagnostic.span.start.min(buffer.len()));
            let (end_line, end_column) = position_json(buffer, d.diagnostic.span.end.min(buffer.len()));
            value["line"] = json!(line);
            value["column"] = json!(column);
            value["end_line"] = json!(end_line);
            value["end_column"] = json!(end_column);
        }
        value
    };
    let count = |list: &[DtdDiagnostic], severity: Severity| list.iter().filter(|d| d.diagnostic.severity() == severity).count();
    let well_formed = count(&found.well_formedness, Severity::Error) == 0;
    let invalid = count(&found.validity, Severity::Error);
    let all: Vec<Value> =
        found.well_formedness.iter().map(|d| describe("well-formedness", d)).chain(found.validity.iter().map(|d| describe("validity", d))).collect();
    json_result(json!({
        "well_formed": well_formed,
        "valid": well_formed && invalid == 0,
        "validated": well_formed,
        "errors": count(&found.well_formedness, Severity::Error) + invalid,
        "warnings": count(&found.well_formedness, Severity::Warning) + count(&found.validity, Severity::Warning),
        "problems": all.iter().take(MAX_PROBLEMS).collect::<Vec<_>>(),
        "truncated": all.len() > MAX_PROBLEMS,
        "lossy": source.lossy,
    }))
}

/// One problem as the tool reports it: where it is in the text it is about (`buffer`, when that text is known) and which text.
fn describe_problem(kind: &str, diagnostic: &Diagnostic, file: Option<String>, buffer: Option<&Buffer>) -> Value {
    let mut value = json!({
        "kind": kind,
        "severity": severity_name(diagnostic.severity()),
        "code": diagnostic.code.key(),
        "message": diagnostic.message(),
        "file": file,
        "start": diagnostic.span.start,
        "end": diagnostic.span.end,
    });
    if let Some(buffer) = buffer {
        let (line, column) = position_json(buffer, diagnostic.span.start.min(buffer.len()));
        let (end_line, end_column) = position_json(buffer, diagnostic.span.end.min(buffer.len()));
        value["line"] = json!(line);
        value["column"] = json!(column);
        value["end_line"] = json!(end_line);
        value["end_column"] = json!(end_column);
    }
    value
}

/// The check against an XML Schema.
fn validate_against_schema(
    a: &mut Automation,
    source: &crate::documents::Source,
    root: &SyntaxNode,
    syntax: &[Diagnostic],
    schema: Option<&str>,
) -> Result<Vec<Content>> {
    let location = source.path.as_deref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let open: Vec<(String, Arc<str>)> = a
        .session()
        .documents()
        .iter()
        .filter_map(|d| d.path().map(|p| (p.to_string_lossy().into_owned(), Arc::from(d.editor().text().as_str()))))
        .collect();
    let env = Environment {
        buffers: Arc::new(OpenDocuments(open)),
        files: Arc::new(ConfinedFiles { confinement: a.confinement.clone() }),
        fetcher: None,
        cache: None,
        clock: Arc::new(FixedClock(0)),
    };
    let resolver = Resolver::new(env, Policy { auto_download: false, offline: true, ..Policy::default() }, &[]);
    let mut loader = ResolvingLoader::new(&resolver);
    // A schema given by a relative path is taken from the folder of the document, as the ones the document names are.
    let found = check_against_schema(root, syntax, &location, schema, &mut loader);
    let own = Buffer::from_text(&source.text);
    let buffers: Vec<Option<Buffer>> =
        found.sources.iter().map(|l| loader.read.iter().find(|(read, _)| read == l).map(|(_, text)| Buffer::from_text(text))).collect();
    let mut problems: Vec<Value> = Vec::new();
    problems.extend(found.well_formedness.iter().map(|d| describe_problem("well-formedness", d, None, Some(&own))));
    problems.extend(found.validity.iter().map(|d| describe_problem("validity", d, None, Some(&own))));
    problems.extend(found.schema_problems.iter().map(|p| {
        let file = found.sources.get(p.source).cloned();
        let buffer = buffers.get(p.source).and_then(Option::as_ref);
        describe_problem("schema", &p.diagnostic, file, buffer)
    }));
    let well_formed = !found.well_formedness.iter().any(|d| d.severity() == Severity::Error);
    let validated = found.outcome == SchemaOutcome::Validated;
    let invalid = found.validity.iter().filter(|d| d.severity() == Severity::Error).count();
    let note = match &found.outcome {
        SchemaOutcome::NoSchema(why) => Some(why.clone()),
        _ => None,
    };
    json_result(json!({
        "kind": "xsd",
        "well_formed": well_formed,
        "valid": validated && invalid == 0,
        "validated": validated,
        "errors": found.well_formedness.iter().filter(|d| d.severity() == Severity::Error).count() + invalid,
        "warnings": found.well_formedness.iter().chain(&found.validity).filter(|d| d.severity() == Severity::Warning).count(),
        "schema_errors": found.schema_errors(),
        "schemas": found.sources,
        "note": note,
        "problems": problems.iter().take(MAX_PROBLEMS).collect::<Vec<_>>(),
        "truncated": problems.len() > MAX_PROBLEMS,
        "lossy": source.lossy,
    }))
}
