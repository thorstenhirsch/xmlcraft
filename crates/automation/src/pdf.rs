//! `pdf_files`: the files embedded in a PDF (the XML of an invoice, say), and the text of one of them.
//!
//! The PDF is a file read through the root confinement. Nothing is written.

use serde_json::{Value, json};
use xmlcraft_pdf::{EmbeddedFile, Pdf};
use xmlcraft_text::encoding::{decode, sniff};

use crate::documents::{fail, json_result};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

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
        let decoded = decode(&bytes, &sniff(&bytes));
        // A file that is not text (an image, a font) has bytes that are not valid in the encoding sniffed for it.
        if !decoded.lossy {
            result["extracted"]["text"] = json!(decoded.text);
        }
    }
    json_result(result)
}
