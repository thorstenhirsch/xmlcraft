//! `xslt_transform`: a stylesheet applied to a document (XSLT on the `xee` engine).
//!
//! The document is an open document (`id`), a file (`path`, through the root confinement) or `text`. The stylesheet is `stylesheet` (an
//! object with an `id`, a `path` or `text`); without it the one the document names with `<?xml-stylesheet href=…?>` is read, relative
//! to the document. Nothing is changed: the result is returned (open it with `doc_new` and `doc_set_text`, or save it).

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use xmlcraft_engine::xslt::stylesheet_of;
use xmlcraft_query::transform;

use crate::documents::{fail, json_result, source, source_properties};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

/// The most text one answer returns (the length is always the real one).
const MAX_ANSWER_BYTES: usize = 4 * 1024 * 1024;

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut properties = source_properties();
    if let Some(map) = properties.as_object_mut() {
        map.insert(
            "stylesheet".into(),
            json!({
                "type": "object",
                "description": "The XSLT stylesheet: an open document (`id`), a file (`path`) or `text`. Without it the stylesheet the document names with an xml-stylesheet processing instruction is used",
                "properties": { "id": { "type": "integer", "minimum": 0 }, "path": { "type": "string" }, "text": { "type": "string" } },
                "additionalProperties": false
            }),
        );
    }
    vec![ToolDef {
        name: "xslt_transform",
        title: "Transform a document with XSLT",
        description: "Apply an XSLT stylesheet to an XML document and return the result as `text` (the length is `bytes`; `truncated` says if the answer was cut). The engine is `xee`, \
                      which implements part of XSLT 3.0 (templates and apply-templates, for-each, value-of, literal result elements, …): what it does not implement \
                      (xsl:variable and more, as of this version) is an error that says so, never a crash. Nothing is changed.",
        input_schema: json!({ "type": "object", "properties": properties, "additionalProperties": false }),
        read_only: true,
        destructive: false,
        handler: xslt_transform,
    }]
}

fn stylesheet_text(a: &Automation, args: &Value, doc_text: &str, doc_path: Option<&Path>) -> Result<String> {
    if let Some(sheet) = args.get("stylesheet") {
        if !sheet.is_object() {
            return Err(ToolError::InvalidArgs("stylesheet is an object with an id, a path or text".into()));
        }
        return Ok(source(a, sheet)?.text);
    }
    let Some(href) = stylesheet_of(doc_text) else {
        return fail("give `stylesheet`, or name one in the document with <?xml-stylesheet type=\"text/xsl\" href=\"…\"?>");
    };
    let wanted = match doc_path.and_then(Path::parent) {
        Some(dir) => dir.join(&href),
        None => PathBuf::from(&href),
    };
    Ok(source(a, &json!({ "path": wanted.to_string_lossy() }))?.text)
}

fn xslt_transform(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let document = source(a, args)?;
    if !document.language.is_xml() {
        return fail("xslt_transform reads XML documents");
    }
    let sheet = stylesheet_text(a, args, &document.text, document.path.as_deref())?;
    match transform(&document.text, &sheet) {
        Ok(done) => {
            let truncated = done.text.len() > MAX_ANSWER_BYTES;
            let mut end = done.text.len().min(MAX_ANSWER_BYTES);
            while !done.text.is_char_boundary(end) {
                end -= 1;
            }
            json_result(
                json!({ "text": done.text.get(..end).unwrap_or_default(), "bytes": done.text.len(), "items": done.items, "truncated": truncated }),
            )
        }
        Err(e) => fail(e.to_string()),
    }
}
