//! `json_validate`: a JSON document checked against a JSON Schema (drafts 04 to 2020-12). The schema is a file (`schema`), text
//! (`schema_text`) or the file the document's own `$schema` names; the documents it refers to are files read through the root
//! confinement (relative to the schema). Nothing is downloaded.

use serde_json::{Value, json};
use xmlcraft_engine::language::Language;
use xmlcraft_json::{Draft, Loader, SchemaSet};
use xmlcraft_json_syntax::{Data, Dialect, Options, Severity, parse};
use xmlcraft_platform::file_uri;
use xmlcraft_text::Buffer;

use crate::documents::{fail, json_result, language_source_properties, position_json, severity_name, source, text_arg};
use crate::file_access::Confinement;
use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

/// The most violations one call lists.
const MAX_LISTED: usize = 500;

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "json_validate",
        title: "Validate against a JSON Schema",
        description: "Check a JSON document (an open document `id`, a file `path`, or `text` with `language` json, jsonc, json5 or jsonl) against a JSON Schema: drafts 04, 06, 07, \
                      2019-09 and 2020-12, the draft taken from the schema's `$schema` (else `draft`, else 2020-12). The schema is a file (`schema`), text (`schema_text`), or, when \
                      neither is given, the file the document's own `$schema` names (a relative path or a file: URI). Schemas it refers to with `$ref` are read as files, relative \
                      to the schema; nothing is downloaded. Returns whether it is valid and the violations, each with the place in the instance (a JSON Pointer and a position in \
                      the text), the keyword that was not met and a message; syntax errors of the document are listed too (`syntax_errors`).",
        input_schema: {
            let mut properties = language_source_properties();
            if let Some(map) = properties.as_object_mut() {
                map.insert(
                    "schema".into(),
                    json!({ "type": "string", "description": "A schema file (relative paths from the folder of the document)" }),
                );
                map.insert("schema_text".into(), json!({ "type": "string", "description": "The schema itself, instead of a file" }));
                map.insert(
                    "draft".into(),
                    json!({ "type": "string", "enum": ["04", "06", "07", "2019-09", "2020-12"], "description": "The draft when the schema has no `$schema` (default 2020-12)" }),
                );
            }
            json!({ "type": "object", "properties": properties, "additionalProperties": false })
        },
        read_only: true,
        destructive: false,
        handler: json_validate,
    }]
}

/// Schemas as files: a `file:` URI or a path, through the root confinement.
struct ConfinedSchemas<'a> {
    confinement: &'a Confinement,
}

impl Loader for ConfinedSchemas<'_> {
    fn load(&self, uri: &str) -> Option<String> {
        let path = if uri.contains("://") || uri.starts_with("file:") { file_uri::path_of(uri)? } else { uri.to_owned() };
        let real = self.confinement.resolve_read(&path).ok()?;
        let loaded = xmlcraft_platform::files::read_file(&real).ok()?;
        Some(String::from_utf8_lossy(&loaded.bytes).into_owned())
    }
}

fn dialect_of(language: Language) -> Dialect {
    match language {
        Language::Json(flavor) => flavor.dialect(),
        Language::Xml | Language::Yaml | Language::Bson => Dialect::Json,
    }
}

fn json_validate(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let source = source(a, args)?;
    let language = if source.language.is_xml() && args.get("language").is_none() {
        Language::Json(xmlcraft_engine::language::JsonFlavor::Json)
    } else {
        source.language
    };
    if !language.is_json() {
        return fail(
            "json_validate reads JSON: give language json, jsonc, json5 or jsonl for text, or a document with a JSON name (YAML and BSON documents are not checked against a schema yet)",
        );
    }
    let instance_buffer = Buffer::from_text(&source.text);
    let lines = language == Language::Json(xmlcraft_engine::language::JsonFlavor::Lines);
    let options = Options { dialect: dialect_of(language), ..Options::default() };
    // JSON Lines are a list of values: validate each against the schema, with the line number as the first place.
    let reads: Vec<(std::ops::Range<usize>, xmlcraft_json_syntax::Parse)> =
        if lines { xmlcraft_json_syntax::parse_lines(&source.text, &options) } else { vec![(0..source.text.len(), parse(&source.text, &options))] };
    let syntax_errors: Vec<Value> = reads
        .iter()
        .flat_map(|(_, read)| read.diagnostics.iter())
        .filter(|d| d.severity() == Severity::Error)
        .take(MAX_LISTED)
        .map(|d| {
            let (line, column) = position_json(&instance_buffer, d.span.start.min(instance_buffer.len()));
            json!({ "severity": severity_name(d.severity()), "code": d.code.key(), "message": d.message(), "line": line, "column": column })
        })
        .collect();

    // The schema: text, a file, or the one the document names.
    let confinement = &a.confinement;
    let named = reads
        .first()
        .and_then(|(_, r)| r.root.as_ref())
        .and_then(|root| root.get("$schema"))
        .and_then(|v| if let Data::String(s) = &v.data { Some(s.clone()) } else { None });
    let (schema_text, schema_location): (String, String) = if let Some(text) = text_arg(args, "schema_text") {
        (text.to_owned(), String::new())
    } else {
        let wanted = match (text_arg(args, "schema"), &named) {
            (Some(path), _) => path.to_owned(),
            (None, Some(name)) if Draft::from_uri(name).is_none() => name.clone(),
            _ => return fail("no schema: give `schema` (a file) or `schema_text`, or let the document's `$schema` name a file"),
        };
        let relative_to = source.path.as_deref().and_then(|p| p.parent());
        let location = if wanted.starts_with("file:") {
            file_uri::path_of(&wanted).unwrap_or(wanted)
        } else if std::path::Path::new(&wanted).is_absolute() || relative_to.is_none() {
            wanted
        } else {
            relative_to.map(|d| d.join(&wanted).to_string_lossy().into_owned()).unwrap_or(wanted)
        };
        let real = confinement.resolve_read(&location)?;
        let loaded = xmlcraft_platform::files::read_file(&real).map_err(|e| ToolError::Failed(format!("{}: {e}", real.display())))?;
        (String::from_utf8_lossy(&loaded.bytes).into_owned(), real.to_string_lossy().into_owned())
    };
    let schema_read = parse(&schema_text, &Options { dialect: Dialect::Jsonc, ..Options::default() });
    if let Some(problem) = schema_read.diagnostics.iter().find(|d| d.severity() == Severity::Error) {
        let (line, column) = position_json(&Buffer::from_text(&schema_text), problem.span.start.min(schema_text.len()));
        return fail(format!("the schema is not valid JSON (line {line}, column {column}: {})", problem.message()));
    }
    let Some(schema_root) = schema_read.root else { return fail("the schema is empty") };
    let default_draft = match text_arg(args, "draft") {
        Some(name) => {
            Draft::from_name(name).ok_or_else(|| ToolError::InvalidArgs(format!("draft is 04, 06, 07, 2019-09 or 2020-12, not {name:?}")))?
        }
        None => Draft::D2020_12,
    };
    let base = if schema_location.is_empty() { String::new() } else { file_uri::uri_of(&schema_location) };
    let set = SchemaSet::new(schema_root, &base, default_draft, &ConfinedSchemas { confinement });

    let mut listed: Vec<Value> = Vec::new();
    let (mut count, mut truncated, mut valid) = (0usize, false, syntax_errors.is_empty());
    for (_, read) in &reads {
        let Some(root) = &read.root else { continue };
        let outcome = set.validate(root);
        truncated |= outcome.truncated;
        valid &= outcome.is_valid();
        for v in outcome.violations {
            count += 1;
            if listed.len() >= MAX_LISTED {
                truncated = true;
                continue;
            }
            let span = root.span_of_pointer(&v.instance).unwrap_or(root.span.clone());
            let (start, end) = (span.start, span.end);
            let (line, column) = position_json(&instance_buffer, start.min(instance_buffer.len()));
            let (end_line, end_column) = position_json(&instance_buffer, end.min(instance_buffer.len()));
            listed.push(json!({
                "instance_pointer": v.instance, "schema_pointer": v.schema, "keyword": v.keyword, "message": v.message,
                "line": line, "column": column, "end_line": end_line, "end_column": end_column, "start": start, "end": end,
            }));
        }
    }
    json_result(json!({
        "valid": valid,
        "draft": set.draft().name(),
        "violation_count": count,
        "violations": listed,
        "truncated": truncated,
        "syntax_errors": syntax_errors,
        "schema_problems": set.problems.iter().map(|p| json!({ "document": p.document, "pointer": p.pointer, "message": p.message })).collect::<Vec<_>>(),
        "lossy": source.lossy,
    }))
}
