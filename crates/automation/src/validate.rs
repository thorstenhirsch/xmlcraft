//! `xml_validate`: a document checked against the DTD it names. The external parts of the DTD are files read through the root
//! confinement (a relative system identifier is taken from the folder of the document, a `file:` URI is a path); nothing is downloaded.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use xmlcraft_dtd::{DtdDiagnostic, LoadRequest, Loaded, SubsetLoader};
use xmlcraft_syntax::{Severity, parse};
use xmlcraft_text::Buffer;
use xmlcraft_text::encoding::{decode, sniff};
use xmlcraft_validate::check;

use crate::catalog::path_of;
use crate::documents::{fail, json_result, position_json, severity_name, source, source_properties};
use crate::file_access::Confinement;
use crate::tools::ToolDef;
use crate::{Automation, Content, Result};

/// The most problems one call lists.
const MAX_PROBLEMS: usize = 500;
/// The largest external text read.
const MAX_EXTERNAL_BYTES: usize = 16 * 1024 * 1024;

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

/// Reads the external texts of a DTD from files, through the confinement, and keeps what it read to say where the problems in them are.
struct FileLoader<'a> {
    confinement: &'a Confinement,
    /// (where it was read from, the text)
    read: Vec<(String, String)>,
}

impl SubsetLoader for FileLoader<'_> {
    fn load(&mut self, request: &LoadRequest<'_>) -> std::result::Result<Loaded, String> {
        let system = request.system;
        // A file: URI is a path; any other scheme is not read.
        let wanted: PathBuf = if let Some(path) = path_of(system) {
            PathBuf::from(path)
        } else if has_scheme(system) {
            return Err("only files are read, nothing is downloaded".to_owned());
        } else {
            let base = path_of(request.base).unwrap_or_else(|| request.base.to_owned());
            Path::new(&base).parent().unwrap_or(Path::new("")).join(system)
        };
        let path = self.confinement.resolve_read(&wanted.to_string_lossy()).map_err(|e| e.to_string())?;
        let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
        if size > MAX_EXTERNAL_BYTES as u64 {
            return Err(format!("over {MAX_EXTERNAL_BYTES} bytes"));
        }
        let loaded = xmlcraft_platform::files::read_file(&path).map_err(|e| e.to_string())?;
        let decoded = decode(&loaded.bytes, &sniff(&loaded.bytes));
        if decoded.lossy {
            return Err("it has bytes that are not valid in its encoding".to_owned());
        }
        let location = path.to_string_lossy().into_owned();
        self.read.push((location.clone(), decoded.text.clone()));
        Ok(Loaded { text: decoded.text, location })
    }
}

/// Whether a system identifier starts with a URI scheme (`http:`, `ftp:`, …) other than `file:`: it names no file. (A single letter before
/// the colon is a drive letter of Windows.)
fn has_scheme(system: &str) -> bool {
    let Some((scheme, _)) = system.split_once(':') else { return false };
    let mut chars = scheme.chars();
    scheme.len() > 1
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn xml_validate(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let source = source(a, args)?;
    if source.text.len() > xmlcraft_engine::analysis::MAX_ANALYSED_BYTES {
        return fail("the text is too large to analyse");
    }
    let parsed = parse(&source.text);
    let mut loader = FileLoader { confinement: &a.confinement, read: Vec::new() };
    let location = source.path.as_deref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let found = check(&parsed.syntax(), &source.text, parsed.diagnostics(), &location, &mut loader);
    let own = Buffer::from_text(&source.text);
    let buffers: Vec<Option<(String, Buffer)>> = found
        .sources
        .iter()
        .map(|location| loader.read.iter().find(|(l, _)| l == location).map(|(l, text)| (l.clone(), Buffer::from_text(text))))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scheme_is_not_a_drive_letter_or_a_folder() {
        for yes in ["http://x/y.dtd", "ftp:y", "x+y-z.1:a", "HTTP://X"] {
            assert!(has_scheme(yes), "{yes}");
        }
        for no in ["y.dtd", "dir/y.dtd", "C:\\dir\\y.dtd", "C:/dir/y.dtd", "a/b:c", ":a", "1x:a", ""] {
            assert!(!has_scheme(no), "{no}");
        }
    }
}
