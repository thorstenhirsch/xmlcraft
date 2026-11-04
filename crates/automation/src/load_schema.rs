//! `xml_load_schema`: the schema or DTD an XML document names, found the way the app finds it (Load Schema).
//!
//! The document is an open document, a file (read through the root confinement) or text. The grammar is found among the open documents,
//! the catalogs, the schemas that ship with the app, files and, when the program gave the session a network that downloads and it is not
//! offline, the web through the cache. Nothing is written; with `open` the text is added as a document.

use serde_json::{Value, json};
use xmlcraft_engine::schema_fetch::fetch;
use xmlcraft_engine::schema_link::{SchemaLink, schema_link};

use crate::documents::{analysis_of, fail, json_result, source, source_properties};
use crate::tools::ToolDef;
use crate::validate::schema_resolver;
use crate::{Automation, Content, Result};

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut properties = source_properties();
    if let Some(map) = properties.as_object_mut() {
        map.insert("open".into(), json!({ "type": "boolean", "description": "Add the grammar as a document (an unsaved one, unless it is a file that is open already or can be opened) and return its id" }));
    }
    vec![ToolDef {
        name: "xml_load_schema",
        title: "Find the schema or DTD an XML document names",
        description: "Find the grammar a document names: the external identifiers of its DOCTYPE, else the first location of its root element's \
                      xsi:schemaLocation or xsi:noNamespaceSchemaLocation (a namespace alone, or an internal subset alone, names nothing). The document is \
                      an open document `id`, a file `path` or `text`. Returns `link` (what the document says), `location` and `origin` (open document, \
                      bundle, file, cache or network) of what was found, and its `text`. With `open`, the text is also added as a document and its \
                      `document` returned. Fails, saying why, when the document names nothing or the grammar cannot be found (offline and not cached, \
                      a missing file, an error from the server).",
        input_schema: json!({ "type": "object", "properties": properties, "additionalProperties": false }),
        read_only: false,
        destructive: false,
        handler: xml_load_schema,
    }]
}

fn link_json(link: &SchemaLink) -> Value {
    match link {
        SchemaLink::ExternalId { public, system } => json!({ "kind": "doctype", "public": public, "system": system }),
        SchemaLink::Uri { location, namespace } => json!({ "kind": "schema_location", "location": location, "namespace": namespace }),
    }
}

fn xml_load_schema(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let document = source(a, args)?;
    let analysis = analysis_of(&document)?;
    let Some(link) = schema_link(&analysis) else { return fail("the document names no schema or DTD") };
    let base = document.path.as_deref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let resolver = schema_resolver(a);
    let found = fetch(&resolver, &link, &base).map_err(crate::ToolError::Failed)?;
    let mut value = json!({
        "link": link_json(&link),
        "location": found.location,
        "origin": found.origin_name(),
        "text": found.text,
        "lossy": found.lossy,
    });
    if args.get("open").and_then(Value::as_bool) == Some(true) {
        let index = a.session.new_document();
        if let Some(doc) = a.session.document_mut(index) {
            doc.name_untitled(&link.label());
            doc.editor_mut().load(&found.text);
            value["document"] = crate::documents::describe(doc);
        }
    }
    json_result(value)
}
