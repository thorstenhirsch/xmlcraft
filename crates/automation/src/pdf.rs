//! `pdf_files`: the files embedded in a PDF (the XML of an invoice, say), and the text of one of them.
//!
//! The PDF is a file read through the root confinement. Nothing is written.

use serde_json::{Value, json};
use xmlcraft_pdf::{EmbeddedFile, Pdf};
use xmlcraft_text::encoding::{decode, sniff};

use crate::documents::{fail, json_result};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

/// The most bytes of an embedded file that are returned as text.
const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;

/// Whether a file is text: by its media type (`text/*`, XML and JSON), else by the extension of its name.
fn looks_textual(file: &EmbeddedFile) -> bool {
    if let Some(mime) = file.mime.as_deref().map(str::to_ascii_lowercase) {
        let textual = mime.starts_with("text/")
            || matches!(mime.as_str(), "application/xml" | "application/json" | "application/xhtml+xml" | "application/xslt+xml")
            || mime.ends_with("+xml")
            || mime.ends_with("+json");
        if textual {
            return true;
        }
        // A media type that names something else (an image, a font, `application/octet-stream`) is believed unless it is the generic one.
        if mime != "application/octet-stream" {
            return false;
        }
    }
    let extension = file.name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    matches!(
        extension.as_deref(),
        Some("xml" | "xsd" | "xsl" | "xslt" | "json" | "txt" | "csv" | "html" | "htm" | "md" | "yaml" | "yml" | "svg" | "rdf")
    )
}

/// The largest PDF the tool reads.
const MAX_BYTES: usize = 256 * 1024 * 1024;

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "pdf_files",
        title: "List and read the files embedded in a PDF",
        description: "Read a PDF file and list the files embedded in it (the `/EmbeddedFiles` name tree and the associated files `/AF`: the XML of a Factur-X, ZUGFeRD \
                      or XRechnung PDF, say): each with its `name`, `relationship` (Data, Source, Alternative, …), `mime` type, declared `size` and the number of \
                      its stream. With `extract` (a file name, or its index from 0) the file's bytes are decoded and returned as `text` when they are text (an \
                      XML file is read in the encoding it declares). Encrypted PDFs are refused. A PDF whose cross-reference is damaged is read by scanning it \
                      and `recovered` says so. Nothing is written.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "The PDF file (inside the root)" },
                "extract": { "description": "The name of an embedded file, or its index from 0", "oneOf": [{ "type": "string" }, { "type": "integer", "minimum": 0 }] }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
        read_only: true,
        destructive: false,
        handler: pdf_files,
    }]
}

fn describe(index: usize, f: &EmbeddedFile) -> Value {
    json!({
        "index": index,
        "name": f.name,
        "description": f.description,
        "relationship": f.relationship,
        "mime": f.mime,
        "size": f.size,
        "stream": f.stream.number,
    })
}

fn pdf_files(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let path = args.get("path").and_then(Value::as_str).ok_or_else(|| ToolError::InvalidArgs("`path` is required".into()))?;
    let path = a.confinement.resolve_read(path)?;
    let loaded = xmlcraft_platform::files::read_file(&path).map_err(|e| ToolError::Failed(format!("{}: {e}", path.display())))?;
    if loaded.bytes.len() > MAX_BYTES {
        return fail(format!("the file is larger than {} MB", MAX_BYTES / (1024 * 1024)));
    }
    let pdf = match Pdf::open(loaded.bytes) {
        Ok(pdf) => pdf,
        Err(e) => return fail(format!("{}: {e}", path.display())),
    };
    let files = pdf.embedded_files();
    let listed: Vec<Value> = files.iter().enumerate().map(|(i, f)| describe(i, f)).collect();
    let mut result = json!({ "recovered": pdf.recovered(), "count": files.len(), "files": listed });
    if let Some(wanted) = args.get("extract") {
        let file = match wanted {
            Value::Number(n) => {
                let index = n
                    .as_u64()
                    .and_then(|i| usize::try_from(i).ok())
                    .ok_or_else(|| ToolError::InvalidArgs("extract is a file name or an index from 0".into()))?;
                files.get(index)
            }
            Value::String(name) => files.iter().find(|f| &f.name == name),
            _ => return Err(ToolError::InvalidArgs("extract is a file name or an index".into())),
        };
        let Some(file) = file else { return fail(format!("the PDF has no embedded file {wanted}")) };
        let bytes = pdf.embedded_data(file).map_err(|e| ToolError::Failed(e.to_string()))?;
        result["extracted"] = json!({ "name": file.name, "bytes": bytes.len() });
        // Text is returned for a file that is text by its media type or its name (valid UTF-8 alone proves nothing: unmarked bytes
        // are guessed to be UTF-8), and not beyond a size a response can carry.
        let textual = looks_textual(file);
        result["extracted"]["textual"] = json!(textual);
        if textual && bytes.len() > MAX_TEXT_BYTES {
            result["extracted"]["text_omitted"] = json!(format!("the file is larger than {} MB", MAX_TEXT_BYTES / (1024 * 1024)));
        } else if textual {
            let decoded = decode(&bytes, &sniff(&bytes));
            if decoded.lossy {
                result["extracted"]["text_omitted"] = json!("its bytes are not valid in its encoding");
            } else {
                result["extracted"]["text"] = json!(decoded.text);
            }
        }
    }
    json_result(result)
}
