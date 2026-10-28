//! `xml_validate`: a document checked against the DTD it names. The external parts of the DTD are files read through the root
//! confinement (a relative system identifier is taken from the folder of the document, a `file:` URI is a path); nothing is downloaded.

use serde_json::{Value, json};
use xmlcraft_dtd::DtdDiagnostic;
use xmlcraft_platform::subset::FileSubsetLoader;
use xmlcraft_syntax::{Severity, parse};
use xmlcraft_text::Buffer;
use xmlcraft_validate::check;

use crate::documents::{fail, json_result, position_json, severity_name, source, source_properties};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result};

/// The most problems one call lists.
const MAX_PROBLEMS: usize = 500;

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "xml_validate",
        title: "Validate against a DTD",
        description: "Check a document (an open document `id`, a file `path`, or `text`) for well-formedness and, when it is well-formed, validity against the DTD its DOCTYPE \
                      declares: the internal subset and the external subset and parameter entities that are files (a relative system identifier is taken from the folder \
                      of the document; nothing is downloaded). A document with no DOCTYPE has nothing to be valid against. Returns the counts and the problems with their \
                      position and message; `kind` says whether a problem is about well-formedness or validity, and `file` names an external text a problem is in.",
        input_schema: json!({
            "type": "object",
            "properties": source_properties(),
            "additionalProperties": false,
        }),
        read_only: true,
        destructive: false,
        handler: xml_validate,
    }]
}

fn xml_validate(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let source = source(a, args)?;
    if source.text.len() > xmlcraft_engine::analysis::MAX_ANALYSED_BYTES {
        return fail("the text is too large to analyse");
    }
    let parsed = parse(&source.text);
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
