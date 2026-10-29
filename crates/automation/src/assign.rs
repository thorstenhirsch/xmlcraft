//! `xml_assign_schema`: a DTD or an XML Schema assigned to a document, as minimal edits (a DOCTYPE, or `xsi:` attributes on the root
//! element). For an open document the edits are made in it (one undoable step); for a file or text the new text is returned.

use serde_json::{Value, json};
use xmlcraft_engine::assign::{Assignment, assign, is_dtd_location, schema_namespace};

use crate::documents::{describe, index_of, json_result, source, source_properties};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut properties = source_properties();
    if let Some(map) = properties.as_object_mut() {
        map.insert(
            "schema".into(),
            json!({ "type": "string", "description": "The location to write: a path (relative paths are from the folder of the document) or a URL" }),
        );
        map.insert("kind".into(), json!({ "type": "string", "enum": ["xsd", "dtd"], "description": "A schema or a DTD (default: a DTD when the location ends in .dtd, .ent or .mod)" }));
        map.insert(
            "namespace".into(),
            json!({ "type": "string", "description": "The target namespace of the schema (default: read from the schema document when it is a file that can be read)" }),
        );
    }
    vec![ToolDef {
        name: "xml_assign_schema",
        title: "Assign a schema or a DTD to a document",
        description: "Say which schema or DTD a document is to be checked against, as a minimal edit: a DTD is `<!DOCTYPE root SYSTEM \"location\">` before \
                      the root element (an existing DOCTYPE keeps its name and internal subset); an XML Schema is `xsi:noNamespaceSchemaLocation` or \
                      `xsi:schemaLocation` on the root element, with the declaration of the `xsi` prefix when it is not there, or the location changed when \
                      one is. Nothing else of the text changes. For an open document `id` the edit is made in it; for a `path` or `text` the new text is \
                      returned and nothing is written. A document that is not well-formed, or has a DOCTYPE when a schema is assigned, is refused.",
        input_schema: json!({ "type": "object", "properties": properties, "required": ["schema"], "additionalProperties": false }),
        read_only: false,
        destructive: false,
        handler: xml_assign_schema,
    }]
}

fn xml_assign_schema(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let location = args
        .get("schema")
        .and_then(Value::as_str)
        .filter(|l| !l.is_empty())
        .ok_or_else(|| ToolError::InvalidArgs("schema is the location to write".into()))?;
    let document = source(a, args)?;
    let is_dtd = match args.get("kind").and_then(Value::as_str) {
        Some("dtd") => true,
        Some("xsd") => false,
        Some(other) => return Err(ToolError::InvalidArgs(format!("kind is xsd or dtd, not {other:?}"))),
        None => is_dtd_location(location),
    };
    // The namespace the schema is for: told, or read from the schema document when it can be (through the confinement).
    let mut namespace: Option<String> = args.get("namespace").and_then(Value::as_str).map(str::to_owned);
    let mut read = false;
    if !is_dtd && namespace.is_none() {
        let base = document.path.as_deref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let wanted = xmlcraft_resolve::join(&base, location);
        if let Ok(path) = a.confinement.resolve_read(&wanted)
            && let Ok(loaded) = xmlcraft_platform::files::read_file(&path)
        {
            read = true;
            namespace = schema_namespace(&xmlcraft_text::encoding::decode(&loaded.bytes, &xmlcraft_text::encoding::sniff(&loaded.bytes)).text);
        }
    }
    let what = if is_dtd { Assignment::Dtd { location } } else { Assignment::Schema { namespace: namespace.as_deref(), location } };
    let edits = assign(&document.text, &what).map_err(|e| ToolError::Failed(e.to_string()))?;
    match document.id {
        Some(id) => {
            let index = index_of(a, id)?;
            let Some(doc) = a.session.document_mut(index) else { return Err(ToolError::Failed("the document disappeared".into())) };
            // No edits when it is assigned already: the editor leaves the document alone (no step to undo, not modified).
            let changed = doc.editor_mut().apply_edits(&edits);
            json_result(
                json!({ "changed": changed, "edits": edits.len(), "namespace": namespace, "namespace_read": read, "document": describe(doc) }),
            )
        }
        None => {
            let text = xmlcraft_text::apply_edits(&document.text, &edits);
            json_result(
                json!({ "changed": !edits.is_empty(), "edits": edits.len(), "text": text, "namespace": namespace, "namespace_read": read, "lossy": document.lossy }),
            )
        }
    }
}
