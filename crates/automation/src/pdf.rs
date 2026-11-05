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

pub(crate) fn replace_tools() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "pdf_replace_file",
        title: "Replace the XML embedded in a PDF",
        description: "Write a copy of a PDF in which an embedded file (the XML of an invoice, say) has new content. The change is an incremental update appended after \
                      the original bytes: every byte of the original stays, the file's `/Params` (size, modification date, MD5 checksum) and the second half of the file \
                      identifier are made right, and a new cross-reference section is added, as a table or a stream like the one the file had. Refused (and nothing \
                      written): encrypted, signed or certified PDFs, a PDF whose cross-reference is damaged, a file inside an object stream. `file` is the file's name or \
                      index (default 0); `text` is the new content, written as UTF-8 (an XML declaration for another encoding is refused). `output` is the new PDF; it must \
                      not exist unless `overwrite` is true (which may name the input itself).",
        input_schema: json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "The PDF (inside the root)" },
                "file": { "description": "The name of the embedded file, or its index from 0 (default 0)", "oneOf": [{ "type": "string" }, { "type": "integer", "minimum": 0 }] },
                "text": { "type": "string", "description": "The new content of the embedded file" },
                "output": { "type": "string", "description": "Where to write the changed PDF (inside the root)" },
                "overwrite": { "type": "boolean", "description": "Replace `output` if it exists (default false)" }
            },
            "required": ["path", "text", "output"],
            "additionalProperties": false
        }),
        read_only: false,
        destructive: true,
        handler: pdf_replace_file,
    }]
}

/// The encoding an XML declaration at the start of `text` names, if it names one.
fn declared_encoding(text: &str) -> Option<String> {
    let head = text.trim_start_matches('\u{feff}').strip_prefix("<?xml")?;
    let end = head.find("?>")?;
    let declaration = head.get(..end)?;
    let at = declaration.find("encoding")?;
    let rest = declaration.get(at + "encoding".len()..)?.trim_start().strip_prefix('=')?.trim_start();
    let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let value = rest.get(1..)?;
    value.split(quote).next().map(str::to_owned)
}

fn pdf_replace_file(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let text_arg = |key: &str| args.get(key).and_then(Value::as_str).ok_or_else(|| ToolError::InvalidArgs(format!("`{key}` is required")));
    let (path, text, output) = (text_arg("path")?, text_arg("text")?, text_arg("output")?);
    let overwrite = match args.get("overwrite") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(ToolError::InvalidArgs("overwrite is true or false".into())),
    };
    if let Some(encoding) = declared_encoding(text)
        && !encoding.eq_ignore_ascii_case("utf-8")
        && !encoding.eq_ignore_ascii_case("utf8")
    {
        return fail(format!("the text declares the encoding {encoding}; it is written as UTF-8, so change the declaration or remove it"));
    }
    let input = a.confinement.resolve_read(path)?;
    let target = a.confinement.resolve_write(output)?;
    if target.exists() && !overwrite {
        return fail(format!("{} exists: pass overwrite: true to replace it", target.display()));
    }
    let loaded = xmlcraft_platform::files::read_file(&input).map_err(|e| ToolError::Failed(format!("{}: {e}", input.display())))?;
    if loaded.bytes.len() > MAX_BYTES {
        return fail(format!("the file is larger than {} MB", MAX_BYTES / (1024 * 1024)));
    }
    let pdf = Pdf::open(loaded.bytes).map_err(|e| ToolError::Failed(format!("{}: {e}", input.display())))?;
    let files = pdf.embedded_files();
    let file = match args.get("file") {
        None => files.first(),
        Some(Value::Number(n)) => {
            let index =
                n.as_u64().and_then(|i| usize::try_from(i).ok()).ok_or_else(|| ToolError::InvalidArgs("file is a name or an index from 0".into()))?;
            files.get(index)
        }
        Some(Value::String(name)) => files.iter().find(|f| &f.name == name),
        Some(_) => return Err(ToolError::InvalidArgs("file is a name or an index".into())),
    };
    let Some(file) = file else { return fail("the PDF has no such embedded file".to_owned()) };
    let now = xmlcraft_resolve::Clock::now(&xmlcraft_platform::network::SystemClock);
    let changed = pdf.replace_embedded(file, text.as_bytes(), &xmlcraft_pdf::pdf_date(now)).map_err(|e| ToolError::Failed(e.to_string()))?;
    xmlcraft_platform::files::write_file(&target, &changed).map_err(|e| ToolError::Failed(format!("{}: {e}", target.display())))?;
    json_result(
        json!({ "written": target.display().to_string(), "bytes": changed.len(), "added": changed.len().saturating_sub(pdf.bytes().len()), "file": file.name }),
    )
}
