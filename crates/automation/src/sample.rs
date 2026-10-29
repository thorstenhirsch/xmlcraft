//! `xml_sample`: an XML document generated from an XML Schema or a DTD.
//!
//! The grammar is an open document, a file (read through the root confinement) or text. The schema documents it includes and imports are
//! found among the open documents, the schemas that ship with the app and files; nothing is downloaded. Nothing is written: the document
//! is returned.

use serde_json::{Value, json};
use xmlcraft_dtd::{LoadRequest, Loaded, SubsetLoader, read_doctype};
use xmlcraft_platform::subset::FileSubsetLoader;
use xmlcraft_sample::{ChoicePolicy, Fill, Options, QName, dtd_roots, sample_from_dtd, sample_from_schema, schema_roots};
use xmlcraft_syntax::{Severity, parse};
use xmlcraft_xsd::dom;
use xmlcraft_xsd::{ResolvingLoader, XSD_NAMESPACE, assemble, load};

use crate::documents::{fail, json_result, source, source_properties};
use crate::tools::ToolDef;
use crate::validate::schema_resolver;
use crate::{Automation, Content, Result, ToolError};

/// The most schema problems named in a refusal.
const MAX_REPORTED: usize = 5;

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut properties = source_properties();
    if let Some(map) = properties.as_object_mut() {
        map.insert("kind".into(), json!({ "type": "string", "enum": ["xsd", "dtd"], "description": "What the document is (default: an XML Schema when its root element is `schema`, else a DTD)" }));
        map.insert(
            "root".into(),
            json!({ "type": "string", "description": "The element to start with: its name, or `{namespace}name` (default: the first that can be made)" }),
        );
        map.insert("optional".into(), json!({ "type": "boolean", "description": "Make the optional elements and attributes (default true)" }));
        map.insert("repeat".into(), json!({ "type": "integer", "minimum": 0, "maximum": 100, "description": "How often an element that may occur more than once occurs, within its bounds (default 1)" }));
        map.insert("choice".into(), json!({ "type": "string", "enum": ["first", "rotate"], "description": "Which branch of a choice: the first, or each in turn (default first)" }));
        map.insert("fill".into(), json!({ "type": "string", "enum": ["typed", "named"], "description": "typed: `string`, `1`, `2000-01-01`; named: a string is the name of its element or attribute (default typed)" }));
        map.insert("max_depth".into(), json!({ "type": "integer", "minimum": 0, "maximum": 50, "description": "How deep elements nest before only what is required is made (default 6)" }));
        map.insert("schema_location".into(), json!({ "type": "string", "description": "Where the schema is: said in the document with xsi:schemaLocation or xsi:noNamespaceSchemaLocation" }));
    }
    vec![ToolDef {
        name: "xml_sample",
        title: "Generate a sample XML document from an XSD or a DTD",
        description: "Make an XML document that is valid against an XML Schema or a DTD, given as an open document `id`, a file `path` or `text`. The content \
                      models are followed (a choice takes a branch, an element repeats `repeat` times within its bounds); values come from fixed values, defaults, \
                      enumerations, patterns and the kind of each type, each checked by the type itself; recursion stops at `max_depth` with only what is required. \
                      Returns `text`, the `warnings` (a value or an element that could not be made as asked), the `root` used and the `roots` that could be. \
                      Nothing is written to disk.",
        input_schema: json!({ "type": "object", "properties": properties, "additionalProperties": false }),
        read_only: true,
        destructive: false,
        handler: xml_sample,
    }]
}

fn options_of(args: &Value) -> Result<Options> {
    let mut options = Options::default();
    if let Some(optional) = args.get("optional").and_then(Value::as_bool) {
        options.optional = optional;
    }
    if let Some(repeat) = args.get("repeat") {
        options.repeat = repeat
            .as_u64()
            .filter(|n| *n <= 100)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| ToolError::InvalidArgs("repeat is a whole number from 0 to 100".into()))?;
    }
    options.choice = match args.get("choice").and_then(Value::as_str) {
        None | Some("first") => ChoicePolicy::First,
        Some("rotate") => ChoicePolicy::Rotate,
        Some(other) => return Err(ToolError::InvalidArgs(format!("choice is first or rotate, not {other:?}"))),
    };
    options.fill = match args.get("fill").and_then(Value::as_str) {
        None | Some("typed") => Fill::Typed,
        Some("named") => Fill::Named,
        Some(other) => return Err(ToolError::InvalidArgs(format!("fill is typed or named, not {other:?}"))),
    };
    if let Some(depth) = args.get("max_depth") {
        options.max_depth = depth
            .as_u64()
            .filter(|n| *n <= 50)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| ToolError::InvalidArgs("max_depth is a whole number from 0 to 50".into()))?;
    }
    options.schema_location = args.get("schema_location").and_then(Value::as_str).map(str::to_owned);
    Ok(options)
}

/// `{namespace}local` or `local` as a name.
fn qname_of(text: &str) -> QName {
    match text.strip_prefix('{').and_then(|rest| rest.split_once('}')) {
        Some((namespace, local)) => QName::new(Some(namespace), local),
        None => QName::new(None, text),
    }
}

/// Reads the text of a DTD as the external subset of a document the first time it is asked for one, and files after that (the parameter
/// entities the DTD refers to).
struct OwnDtd {
    text: Option<String>,
    location: String,
    files: FileSubsetLoader,
}

impl SubsetLoader for OwnDtd {
    fn load(&mut self, request: &LoadRequest<'_>) -> std::result::Result<Loaded, String> {
        match self.text.take() {
            Some(text) => Ok(Loaded { text, location: self.location.clone() }),
            None => self.files.load(request),
        }
    }
}

fn is_xml_schema(text: &str) -> bool {
    let parsed = parse(text);
    if parsed.diagnostics().iter().any(|d| d.severity() == Severity::Error) {
        return false;
    }
    let tree = dom::build(&parsed.syntax());
    tree.elements.first().is_some_and(|root| root.local == "schema" && root.namespace.as_deref() == Some(XSD_NAMESPACE))
}

fn xml_sample(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let options = options_of(args)?;
    let grammar = source(a, args)?;
    let kind = match args.get("kind").and_then(Value::as_str) {
        Some(k) => k,
        None if is_xml_schema(&grammar.text) => "xsd",
        None => "dtd",
    };
    let root_name = args.get("root").and_then(Value::as_str);
    let location = grammar.path.as_deref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    if kind == "xsd" {
        let resolver = schema_resolver(a);
        let mut loader = ResolvingLoader::new(&resolver);
        let documents = load(&grammar.text, &location, &mut loader);
        let assembly = assemble(&documents);
        let errors: Vec<String> = documents
            .diagnostics
            .iter()
            .chain(&assembly.diagnostics)
            .filter(|d| d.diagnostic.severity() == Severity::Error)
            .map(|d| d.diagnostic.message())
            .collect();
        if !errors.is_empty() {
            let shown: Vec<&str> = errors.iter().take(MAX_REPORTED).map(String::as_str).collect();
            return fail(format!("the schema has {} error(s), so no document is made from it: {}", errors.len(), shown.join(" ")));
        }
        let wanted = root_name.map(qname_of);
        let made = sample_from_schema(&assembly.schema, wanted.as_ref(), &options).map_err(|e| ToolError::Failed(e.to_string()))?;
        let roots: Vec<String> = schema_roots(&assembly.schema).iter().map(|q| q.to_string()).collect();
        return json_result(json!({
            "kind": "xsd",
            "text": made.text,
            "warnings": made.warnings,
            "root": wanted.map(|q| q.to_string()).or_else(|| roots.first().cloned()),
            "roots": roots,
        }));
    }
    // A DTD: read as the external subset of a document that has a DOCTYPE with a system identifier.
    let confinement = a.confinement.clone();
    let mut loader = OwnDtd {
        text: Some(grammar.text.clone()),
        location: if location.is_empty() { "the DTD".to_owned() } else { location.clone() },
        files: FileSubsetLoader::confined(move |path| confinement.resolve_read(path).map_err(|e| e.to_string())),
    };
    let probe = "<!DOCTYPE r SYSTEM \"dtd\"><r/>";
    let parsed = parse(probe);
    let Some((read, _)) = read_doctype(&parsed.syntax(), probe, &location, &mut loader) else { return fail("the DTD could not be read") };
    let errors: Vec<String> =
        read.diagnostics.iter().filter(|d| d.diagnostic.severity() == Severity::Error).map(|d| d.diagnostic.message()).collect();
    if !errors.is_empty() {
        let shown: Vec<&str> = errors.iter().take(MAX_REPORTED).map(String::as_str).collect();
        return fail(format!("the DTD has {} error(s), so no document is made from it: {}", errors.len(), shown.join(" ")));
    }
    let made = sample_from_dtd(&read.dtd, root_name, &options).map_err(|e| ToolError::Failed(e.to_string()))?;
    let roots = dtd_roots(&read.dtd);
    json_result(json!({
        "kind": "dtd",
        "text": made.text,
        "warnings": made.warnings,
        "root": root_name.map(str::to_owned).or_else(|| roots.first().cloned()),
        "roots": roots,
    }))
}
