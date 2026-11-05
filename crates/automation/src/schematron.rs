//! `schematron_validate`: a document checked against Schematron rules.
//!
//! The document is an open document (`id`), a file (`path`, through the root confinement) or `text`. The rules are `schema` (an object
//! with an `id`, a `path` or `text`); without it, the rules the document names with `<?xml-model href=… schematypens=…?>` are used.
//! `include`s are read from files through the root confinement, relative to the rules. Nothing is changed.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use xmlcraft_engine::schematron::named;
use xmlcraft_schematron::{CheckKind, Schema, Severity};
use xmlcraft_syntax::parse;
use xmlcraft_text::Buffer;
use xmlcraft_text::encoding::{decode, sniff};

use crate::documents::{fail, json_result, language_source_properties, position_json, source};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

/// The most findings one answer lists (the counts are complete).
const MAX_LISTED: usize = 500;

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut properties = language_source_properties();
    if let Some(map) = properties.as_object_mut() {
        map.insert(
            "schema".into(),
            json!({
                "type": "object",
                "description": "The Schematron rules: an open document (`id`), a file (`path`) or `text`. Without it the rules the document names with an xml-model processing instruction are used",
                "properties": {
                    "id": { "type": "integer", "minimum": 0 },
                    "path": { "type": "string" },
                    "text": { "type": "string" }
                },
                "additionalProperties": false
            }),
        );
        map.insert(
            "phase".into(),
            json!({ "type": "string", "description": "The phase to run (`#ALL` is every pattern; default: the schema's defaultPhase, else all)" }),
        );
    }
    vec![ToolDef {
        name: "schematron_validate",
        title: "Check a document against Schematron rules",
        description: "Fire ISO Schematron rules on an XML document (XPath 3.1 is the query language). Returns the `findings`: each with its `kind` (assert, \
                      report), `severity` (error, warning, info: from the flag or role), `message` (with name and value-of filled in), the check's `id`, `flag` and \
                      `role`, the `test` and rule `context`, the `path` of the node that fired it and its `line` and `column`; the counts (`count`, `errors`, \
                      `fired_rules`, `checks`); and `problems` (rules whose XPath could not be evaluated: the other rules still run). A node is matched by the first \
                      rule of a pattern whose context it is in. Abstract rules and patterns, phases, lets, namespaces and includes are supported.",
        input_schema: json!({ "type": "object", "properties": properties, "additionalProperties": false }),
        read_only: true,
        destructive: false,
        handler: schematron_validate,
    }]
}

/// The text of the rules and where they are (for `include`s), from the `schema` argument or the document's `xml-model` instruction.
fn rules_of(a: &Automation, args: &Value, doc_text: &str, doc_path: Option<&Path>) -> Result<(String, Option<PathBuf>)> {
    if let Some(schema) = args.get("schema") {
        if !schema.is_object() {
            return Err(ToolError::InvalidArgs("schema is an object with an id, a path or text".into()));
        }
        let s = source(a, schema)?;
        return Ok((s.text, s.path));
    }
    let parsed = parse(doc_text);
    let rules = named(&parsed.syntax(), doc_text);
    let Some(first) = rules.first() else {
        return fail(
            "give `schema`, or name rules in the document with <?xml-model href=\"…\" schematypens=\"http://purl.oclc.org/dsdl/schematron\"?>",
        );
    };
    let wanted = match doc_path.and_then(Path::parent) {
        Some(dir) => dir.join(&first.href),
        None => PathBuf::from(&first.href),
    };
    let s = source(a, &json!({ "path": wanted.to_string_lossy() }))?;
    Ok((s.text, s.path))
}

fn schematron_validate(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let document = source(a, args)?;
    if !document.language.is_xml() {
        return fail("schematron_validate reads XML documents");
    }
    let (rules_text, rules_path) = rules_of(a, args, &document.text, document.path.as_deref())?;
    let phase = match args.get("phase") {
        None => None,
        Some(Value::String(p)) => Some(p.as_str()),
        Some(_) => return Err(ToolError::InvalidArgs("phase is a string".into())),
    };
    let base = rules_path.as_deref().and_then(Path::parent).map(Path::to_path_buf);
    let include = |href: &str| -> Option<String> {
        let wanted = match &base {
            Some(dir) => dir.join(href),
            None => PathBuf::from(href),
        };
        let path = a.confinement.resolve_read(&wanted.to_string_lossy()).ok()?;
        let loaded = xmlcraft_platform::files::read_file(&path).ok()?;
        Some(decode(&loaded.bytes, &sniff(&loaded.bytes)).text)
    };
    let schema = match Schema::parse(&rules_text, &include) {
        Ok(schema) => schema,
        Err(e) => return fail(format!("the rules cannot be read: {e}")),
    };
    let report = match schema.validate(&document.text, phase) {
        Ok(report) => report,
        Err(e) => return fail(e.to_string()),
    };
    let buffer = Buffer::from_text(&document.text);
    let findings: Vec<Value> = report
        .findings
        .iter()
        .take(MAX_LISTED)
        .map(|f| {
            let mut v = json!({
                "kind": match f.kind { CheckKind::Assert => "assert", CheckKind::Report => "report" },
                "severity": match f.severity { Severity::Error => "error", Severity::Warning => "warning", Severity::Info => "info" },
                "message": f.message,
                "id": f.id,
                "flag": f.flag,
                "role": f.role,
                "test": f.test,
                "context": f.context,
                "path": f.path,
                "pattern": f.pattern,
            });
            if let Some(span) = &f.span {
                let (line, column) = position_json(&buffer, span.start.min(buffer.len()));
                v["line"] = json!(line);
                v["column"] = json!(column);
            }
            v
        })
        .collect();
    json_result(json!({
        "valid": report.error_count() == 0 && report.problems.is_empty(),
        "count": report.count,
        "errors": report.error_count(),
        "truncated": report.count > findings.len(),
        "fired_rules": report.fired_rules,
        "checks": report.checks,
        "findings": findings,
        "problems": report.problems,
        "title": schema.title,
    }))
}
