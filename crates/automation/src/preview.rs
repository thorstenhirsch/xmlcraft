//! `html_preview`: the page the Preview tool window shows for a document, without a window (`xmlcraft_engine::preview`).
//!
//! The document is an open document (`id`), a file (`path`, through the root confinement) or `text` (`language` html or xml). An HTML
//! document is shown as it is; an XML document as the result of its XSLT stylesheet when that writes HTML (`stylesheet`, else the one
//! it names with `<?xml-stylesheet href=…?>`, read from beside it and inside the root), an SVG image as it is, anything else as a tree.
//! The answer also carries the policy the browser view serves the page with. Nothing is changed and nothing is fetched.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use xmlcraft_engine::language::Language;
use xmlcraft_engine::preview::{Allow, DOCUMENT_URL, Input, MAX_BYTES, content_security_policy, prepare, response_headers};
use xmlcraft_engine::xslt::stylesheet_of;

use crate::documents::{fail, json_result, language_source_properties, source};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

/// The most text of the page one answer returns (`bytes` is always the real length).
const MAX_ANSWER_BYTES: usize = 4 * 1024 * 1024;

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut properties = language_source_properties();
    if let Some(map) = properties.as_object_mut() {
        map.insert(
            "stylesheet".into(),
            json!({
                "type": "object",
                "description": "For an XML document: the XSLT stylesheet, an open document (`id`), a file (`path`) or `text`. Without it the one the document names with an xml-stylesheet processing instruction is used, when there is one",
                "properties": { "id": { "type": "integer", "minimum": 0 }, "path": { "type": "string" }, "text": { "type": "string" } },
                "additionalProperties": false
            }),
        );
        map.insert(
            "allow_remote_images".into(),
            json!({ "type": "boolean", "description": "Report the policy with images from https: addresses allowed, as a person can for one document (default false)" }),
        );
        map.insert(
            "allow_scripts".into(),
            json!({ "type": "boolean", "description": "Report the policy with the page's inline scripts allowed, as a person can for one document (default false)" }),
        );
    }
    vec![ToolDef {
        name: "html_preview",
        title: "Preview a document as a browser shows it",
        description: "The page the Preview tool window shows for an HTML or XML document: `html` (the page; cut at 4 MiB, `bytes` is its length, `truncated` \
                      says if it was cut), `shown` (`html`: the HTML document as it is; `xslt`: the HTML its stylesheet made; `svg`: the SVG image as it is; \
                      `tree`: an XML document as a readable tree), `media_type`, `title`, `warnings` (a stylesheet that does not write HTML or could not be \
                      applied, a tree that was cut), and `policy`: the Content-Security-Policy and the other headers the page is served with, and the one \
                      address the view may load. Documents over 8 MiB are refused. Nothing is changed, nothing is fetched.",
        input_schema: json!({ "type": "object", "properties": properties, "additionalProperties": false }),
        read_only: true,
        destructive: false,
        handler: html_preview,
    }]
}

/// The stylesheet for an XML document: the one given, else the one it names (`None` when it names none; a warning when that cannot be
/// read).
fn stylesheet_text(a: &Automation, args: &Value, doc_text: &str, doc_path: Option<&Path>, warnings: &mut Vec<String>) -> Result<Option<String>> {
    if let Some(sheet) = args.get("stylesheet") {
        if !sheet.is_object() {
            return Err(ToolError::InvalidArgs("stylesheet is an object with an id, a path or text".into()));
        }
        return Ok(Some(source(a, sheet)?.text));
    }
    let Some(href) = stylesheet_of(doc_text) else { return Ok(None) };
    let wanted = match doc_path.and_then(Path::parent) {
        Some(dir) => dir.join(&href),
        None => PathBuf::from(&href),
    };
    match source(a, &json!({ "path": wanted.to_string_lossy() })) {
        Ok(sheet) => Ok(Some(sheet.text)),
        Err(e) => {
            warnings.push(format!("The stylesheet the document names ({href}) could not be read: {e}"));
            Ok(None)
        }
    }
}

fn flag(args: &Value, name: &str) -> Result<bool> {
    match args.get(name) {
        None => Ok(false),
        Some(v) => v.as_bool().ok_or_else(|| ToolError::InvalidArgs(format!("{name} is true or false"))),
    }
}

fn html_preview(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let allow = Allow { remote_images: flag(args, "allow_remote_images")?, scripts: flag(args, "allow_scripts")? };
    let document = source(a, args)?;
    let mut warnings = Vec::new();
    let sheet = match document.language {
        Language::Html => None,
        language if language.is_xml() => stylesheet_text(a, args, &document.text, document.path.as_deref(), &mut warnings)?,
        _ => return fail("html_preview shows HTML and XML documents"),
    };
    let input = match document.language {
        Language::Html => Input::Html(&document.text),
        _ => Input::Xml { text: &document.text, stylesheet: sheet.as_deref() },
    };
    let page = match xmlcraft_engine::guard(|| prepare(input)) {
        Ok(Ok(page)) => page,
        Ok(Err(e)) => return fail(format!("{e} (the cap is {MAX_BYTES} bytes)")),
        Err(_) => return fail("the preview failed (a bug); nothing was changed"),
    };
    warnings.extend(page.warnings.iter().cloned());
    let truncated = page.body.len() > MAX_ANSWER_BYTES;
    let mut end = page.body.len().min(MAX_ANSWER_BYTES);
    while !page.body.is_char_boundary(end) {
        end -= 1;
    }
    let headers: serde_json::Map<String, Value> = response_headers(allow).into_iter().map(|(n, v)| (n.to_owned(), Value::String(v))).collect();
    json_result(json!({
        "shown": page.shown.name(),
        "media_type": page.media_type,
        "title": page.title,
        "warnings": warnings,
        "html": page.body.get(..end).unwrap_or_default(),
        "bytes": page.body.len(),
        "truncated": truncated,
        "policy": {
            "content_security_policy": content_security_policy(allow),
            "headers": headers,
            "document_url": DOCUMENT_URL,
            "navigation": "only the document itself and its anchors",
        },
    }))
}
