//! A JSON, JSONC, JSON5, JSON Lines or YAML document checked against the JSON Schema it names.
//!
//! A document names its schema with a `$schema` member of its root object (a URI or a path, relative to the document), or, in YAML, with
//! the editor comment `# yaml-language-server: $schema=URI`. A `$schema` that names a draft of JSON Schema (`https://json-schema.org/…`)
//! says that the document is itself a schema: it is checked against the meta-schema of that draft. An OpenAPI description (a root
//! `openapi` or `swagger` member) is checked against the official schema of its version (2.0, 3.0, 3.1 or 3.2) with no `$schema`
//! needed. The meta-schemas and the OpenAPI schemas are bundled (`xmlcraft_schemas::json_schema`). A document with no reference is
//! not checked beyond its syntax. The schema, and the documents it refers to, are read through a [`Resolver`] (the open documents first, so a schema
//! being edited is validated against as it is). The result is the [`Validation`] of the XML validators, so the window shows both alike.

use std::cell::RefCell;
use std::ops::Range;

use xmlcraft_json::{Draft, Loader, SchemaSet};
use xmlcraft_json_syntax::{Data, Dialect, Options, Severity, Value, parse, parse_lines};
use xmlcraft_resolve::{Budget, Request, Resolver};
use xmlcraft_syntax::{Code, Diagnostic};
use xmlcraft_text::encoding::{decode, sniff};

use crate::analysis::{MAX_ANALYSED_BYTES, Skipped};
use crate::language::{JsonFlavor, Language};
use crate::validation::{Validation, ValidityProblem, Verdict, finish};

/// The instance, as the values the text holds (one, or one to a line for JSON Lines, or one to a YAML document).
struct Instance {
    roots: Vec<Value>,
    /// What to check against, and where the document says so: the byte range of the value, and the URI or path (empty when the
    /// document names an OpenAPI version this does not know, with `note` saying so).
    reference: Option<(Range<usize>, String)>,
    note: Option<String>,
    /// A syntax error was found.
    broken: bool,
}

fn read(text: &str, language: Language) -> Option<Instance> {
    let (roots, broken) = match language {
        Language::Json(JsonFlavor::Lines) => {
            let options = Options { dialect: Dialect::Json, ..Options::default() };
            let mut broken = false;
            let mut roots = Vec::new();
            for (_, read) in parse_lines(text, &options) {
                broken |= read.diagnostics.iter().any(|d| d.severity() == Severity::Error);
                roots.extend(read.root);
            }
            (roots, broken)
        }
        Language::Json(flavor) => {
            let read = parse(text, &Options { dialect: flavor.dialect(), ..Options::default() });
            (read.root.into_iter().collect(), read.diagnostics.iter().any(|d| d.severity() == Severity::Error))
        }
        Language::Yaml => {
            let read = xmlcraft_yaml_syntax::parse(text, &xmlcraft_yaml_syntax::Options::default());
            (read.documents, read.diagnostics.iter().any(|d| d.severity() == Severity::Error))
        }
        // A BSON file is shown as Extended JSON, whose types a schema does not know.
        Language::Xml | Language::Bson | Language::Html => return None,
    };
    let first = roots.first();
    let named = first.and_then(member_reference).or_else(|| if language == Language::Yaml { modeline(text) } else { None });
    let (reference, note) = match named.or_else(|| first.and_then(draft_reference)) {
        Some(found) => (Some(found), None),
        None => match first.and_then(openapi_reference) {
            Some((span, Ok(uri))) => (Some((span, uri)), None),
            Some((span, Err(why))) => (Some((span, String::new())), Some(why)),
            None => (None, None),
        },
    };
    Some(Instance { roots, reference, note, broken })
}

/// Whether the text names a schema to be checked against (see the module). Only the beginning of the text is looked at for a YAML
/// modeline; the root object is read.
pub fn names_a_schema(text: &str, language: Language) -> bool {
    // (Both ways of naming a schema contain `$schema`: a text without it is not read at all.)
    text.len() <= MAX_ANALYSED_BYTES
        && (text.contains("$schema") || text.contains("openapi") || text.contains("swagger"))
        && read(text, language).is_some_and(|i| !i.broken && i.reference.is_some())
}

/// The bundled meta-schema a `$schema` that names a draft stands for: the document is a schema of that draft.
fn draft_reference(root: &Value) -> Option<(Range<usize>, String)> {
    let Data::Object(members) = &root.data else { return None };
    let member = members.iter().find(|m| m.key == "$schema")?;
    let Data::String(uri) = &member.value.data else { return None };
    let canonical = match Draft::from_uri(uri)? {
        Draft::D04 => "http://json-schema.org/draft-04/schema",
        Draft::D06 => "http://json-schema.org/draft-06/schema",
        Draft::D07 => "http://json-schema.org/draft-07/schema",
        Draft::D2019_09 => "https://json-schema.org/draft/2019-09/schema",
        Draft::D2020_12 => "https://json-schema.org/draft/2020-12/schema",
    };
    Some((member.value.span.clone(), canonical.to_owned()))
}

/// The bundled schema of the OpenAPI version a description says it is (`swagger: "2.0"`, `openapi: 3.1.0`), with the place of the
/// version; an error says why a version is not known.
fn openapi_reference(root: &Value) -> Option<(Range<usize>, Result<String, String>)> {
    let Data::Object(members) = &root.data else { return None };
    let member = members.iter().find(|m| m.key == "openapi").or_else(|| members.iter().find(|m| m.key == "swagger"))?;
    let version = match &member.value.data {
        Data::String(s) => s.clone(),
        Data::Number(n) => n.clone(),
        _ => return None,
    };
    let uri = match (member.key.as_str(), version.as_str()) {
        ("swagger", "2.0") => Some("http://swagger.io/v2/schema.json"),
        ("openapi", v) if v.starts_with("3.0.") => Some("https://spec.openapis.org/oas/3.0/schema/2024-10-18"),
        ("openapi", v) if v.starts_with("3.1.") => Some("https://spec.openapis.org/oas/3.1/schema-base/2025-09-15"),
        ("openapi", v) if v.starts_with("3.2.") => Some("https://spec.openapis.org/oas/3.2/schema-base/2025-09-17"),
        _ => None,
    };
    let result = uri.map(str::to_owned).ok_or_else(|| format!("OpenAPI version {version} is not known; 2.0 (swagger), 3.0, 3.1 and 3.2 are"));
    Some((member.value.span.clone(), result))
}

/// The `$schema` of an object, when it is a string and is not a draft.
fn member_reference(root: &Value) -> Option<(Range<usize>, String)> {
    let Data::Object(members) = &root.data else { return None };
    let member = members.iter().find(|m| m.key == "$schema")?;
    match &member.value.data {
        Data::String(uri) if Draft::from_uri(uri).is_none() && !uri.trim().is_empty() => Some((member.value.span.clone(), uri.clone())),
        _ => None,
    }
}

/// `# yaml-language-server: $schema=URI` in a comment line before the first value.
fn modeline(text: &str) -> Option<(Range<usize>, String)> {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let start = offset + (line.len() - trimmed.len());
        offset += line.len();
        let Some(comment) = trimmed.strip_prefix('#') else {
            if trimmed.trim().is_empty() {
                continue;
            }
            return None;
        };
        if let Some(at) = comment.find("$schema=") {
            let value = &comment[at + "$schema=".len()..];
            let value = value.trim_end();
            if comment.trim_start().starts_with("yaml-language-server:") && !value.is_empty() {
                let begin = start + 1 + at + "$schema=".len();
                return Some((begin..begin + value.len(), value.to_owned()));
            }
        }
    }
    None
}

/// Schema documents through the resolver, one budget for the whole check.
struct Documents<'r> {
    resolver: &'r Resolver,
    budget: RefCell<Budget>,
    /// The documents the schema refers to that could not be read, with why.
    failed: RefCell<Vec<(String, String)>>,
}

impl Documents<'_> {
    fn fetch(&self, reference: &str, base: &str) -> Result<(String, String), String> {
        // The meta-schemas and the OpenAPI schemas are bundled: read from there first, and never from the web.
        if let Some(text) = xmlcraft_schemas::json_schema(reference) {
            return Ok((reference.trim_end_matches('#').to_owned(), text));
        }
        let found = self.resolver.resolve(&Request::uri(reference, base), &mut self.budget.borrow_mut()).map_err(|f| f.message)?;
        let decoded = decode(&found.bytes, &sniff(&found.bytes));
        if decoded.lossy {
            return Err(format!("{}: it has bytes that are not valid in its encoding", found.location));
        }
        Ok((found.location, decoded.text))
    }
}

impl Loader for Documents<'_> {
    fn load(&self, uri: &str) -> Option<String> {
        match self.fetch(uri, "") {
            Ok((_, text)) => Some(text),
            Err(why) => {
                self.failed.borrow_mut().push((uri.to_owned(), why));
                None
            }
        }
    }
}

/// The schema `reference` names (relative to `location`) as a value, and where it was found; the error says why it is not available.
fn load_schema(documents: &Documents<'_>, reference: &str, location: &str) -> Result<(String, Value), String> {
    let (schema_location, schema_text) = documents.fetch(reference, location)?;
    // The schema may have comments and trailing commas (JSONC), as editors allow.
    let schema = parse(&schema_text, &Options { dialect: Dialect::Json5, ..Options::default() });
    if let Some(d) = schema.diagnostics.iter().find(|d| d.severity() == Severity::Error) {
        return Err(format!("{schema_location} is not valid JSON: {}", d.message()));
    }
    let root = schema.root.ok_or_else(|| "the schema has no value".to_owned())?;
    Ok((schema_location, root))
}

/// The schema a document names (a `$schema` member, a yaml-language-server modeline, or the OpenAPI version it says it is) as the
/// reference to read it by; none when it names none. Cheap to refuse: a text without `$schema`, `openapi` or `swagger` is not read.
pub fn schema_reference(text: &str, language: Language) -> Option<String> {
    if text.len() > MAX_ANALYSED_BYTES || !(text.contains("$schema") || text.contains("openapi") || text.contains("swagger")) {
        return None;
    }
    if language == Language::Bson {
        // A BSON file's text is Extended JSON, read here only for the schema it names (its values are not validated).
        let first = match xmlcraft_bson::layout_of_text(text) {
            xmlcraft_bson::Layout::Lines => parse_lines(text, &Options::default()).into_iter().find_map(|(_, read)| read.root),
            xmlcraft_bson::Layout::Single => parse(text, &Options::default()).root,
        }?;
        return member_reference(&first).map(|(_, reference)| reference).filter(|r| !r.is_empty());
    }
    read(text, language)?.reference.map(|(_, reference)| reference).filter(|r| !r.is_empty())
}

/// The schema `reference` names, relative to `location`, read through `resolver` together with the documents it refers to.
pub fn load_schema_set(reference: &str, location: &str, resolver: &Resolver) -> Result<SchemaSet, String> {
    let documents = Documents { resolver, budget: RefCell::new(Budget::default()), failed: RefCell::new(Vec::new()) };
    let (schema_location, root) = load_schema(&documents, reference, location)?;
    Ok(SchemaSet::new(root, &schema_location, Draft::D2020_12, &documents))
}

/// The schema in `text` (a file at `location` when it is one: what relative references in it are taken from), read through
/// `resolver` with the documents it refers to. The text may have comments and trailing commas, as editors allow.
pub fn load_schema_from_text(text: &str, location: &str, resolver: &Resolver) -> Result<SchemaSet, String> {
    if text.len() > MAX_ANALYSED_BYTES {
        return Err("the schema is too large to read".to_owned());
    }
    let documents = Documents { resolver, budget: RefCell::new(Budget::default()), failed: RefCell::new(Vec::new()) };
    let schema = parse(text, &Options { dialect: Dialect::Json5, ..Options::default() });
    if let Some(d) = schema.diagnostics.iter().find(|d| d.severity() == Severity::Error) {
        return Err(format!("the schema is not valid JSON: {}", d.message()));
    }
    let root = schema.root.ok_or_else(|| "the schema has no value".to_owned())?;
    Ok(SchemaSet::new(root, location, Draft::D2020_12, &documents))
}

/// The schema a document names, read through `resolver`, for completion and hover: none when the document names none or it cannot be
/// read.
pub(crate) fn schema_set_for(text: &str, language: Language, location: &str, resolver: &Resolver) -> Option<SchemaSet> {
    load_schema_set(&schema_reference(text, language)?, location, resolver).ok()
}

/// Validate `text` (version `version` of a document of this `language` at `location`, which a relative `$schema` is taken from).
pub fn validate_data(text: &str, language: Language, version: u64, location: &str, resolver: &Resolver) -> Validation {
    if text.len() > MAX_ANALYSED_BYTES {
        return Validation { version, verdict: Verdict::Skipped(Skipped::TooLarge), problems: Vec::new(), truncated: false, invoice: None };
    }
    let Some(instance) = read(text, language) else { return finish(version, Vec::new()) };
    if instance.broken {
        return Validation { version, verdict: Verdict::NotWellFormed, problems: Vec::new(), truncated: false, invoice: None };
    }
    let Some((span, reference)) = instance.reference else { return finish(version, Vec::new()) };
    if let Some(why) = instance.note {
        return finish(version, vec![ValidityProblem { diagnostic: Diagnostic::new(Code::JsonSchemaInvalid, span, vec![why]), file: None }]);
    }
    let documents = Documents { resolver, budget: RefCell::new(Budget::default()), failed: RefCell::new(Vec::new()) };
    let own = |code: Code, span: Range<usize>, args: Vec<String>| ValidityProblem { diagnostic: Diagnostic::new(code, span, args), file: None };
    let (schema_location, root) = match load_schema(&documents, &reference, location) {
        Ok(found) => found,
        Err(why) => return finish(version, vec![own(Code::JsonSchemaUnavailable, span, vec![reference, why])]),
    };
    let set = SchemaSet::new(root.clone(), &schema_location, Draft::D2020_12, &documents);
    let mut problems: Vec<ValidityProblem> = set
        .problems
        .iter()
        .map(|p| {
            let at = if p.document.is_empty() { root.span_of_pointer(&p.pointer).unwrap_or(0..0) } else { 0..0 };
            let named = if p.document.is_empty() { schema_location.clone() } else { p.document.clone() };
            ValidityProblem { diagnostic: Diagnostic::new(Code::JsonSchemaInvalid, at, vec![p.message.clone()]), file: Some(named) }
        })
        .collect();
    // A document the schema refers to that could not be read is a problem of the schema, said once at its start; what then cannot be
    // resolved is not the instance's fault and is not listed against it.
    let failed = documents.failed.borrow().clone();
    for (uri, why) in &failed {
        problems.push(ValidityProblem {
            diagnostic: Diagnostic::new(Code::JsonSchemaRefUnavailable, 0..0, vec![uri.clone(), why.clone()]),
            file: Some(schema_location.clone()),
        });
    }
    let mut truncated = false;
    for value in &instance.roots {
        let outcome = set.validate(value);
        truncated |= outcome.truncated;
        for v in outcome.violations {
            // A reference that cannot be resolved because its document could not be read is said above, not against the instance.
            if v.keyword == "$ref" && v.message.contains("cannot be resolved") && !failed.is_empty() {
                continue;
            }
            let at = value.span_of_pointer(&v.instance).unwrap_or_else(|| value.span.clone());
            let args = vec![format!("{} ({})", v.message, v.keyword)];
            problems.push(own(Code::JsonSchemaViolation, at, args));
        }
    }
    let mut done = finish(version, problems);
    done.truncated |= truncated;
    done
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use xmlcraft_resolve::{Environment, FileAccess, FixedClock, Policy};

    use super::*;

    /// Files by path.
    struct Files(HashMap<String, Vec<u8>>);

    impl FileAccess for Files {
        fn read(&self, path: &str) -> Result<Vec<u8>, String> {
            self.0.get(path).cloned().ok_or_else(|| format!("{path}: no such file"))
        }
    }

    struct NoBuffers;

    impl xmlcraft_resolve::Buffers for NoBuffers {
        fn text(&self, _: &str) -> Option<Arc<str>> {
            None
        }
    }

    fn resolver(files: &[(&str, &str)]) -> Resolver {
        let files = Files(files.iter().map(|(p, t)| ((*p).to_owned(), t.as_bytes().to_vec())).collect());
        let env = Environment { buffers: Arc::new(NoBuffers), files: Arc::new(files), fetcher: None, cache: None, clock: Arc::new(FixedClock(0)) };
        Resolver::new(env, Policy { offline: true, ..Policy::default() }, &[])
    }

    const SCHEMA: &str = r#"{"type":"object","properties":{"name":{"type":"string"},"n":{"type":"integer","minimum":1}},"required":["name"]}"#;

    fn check(text: &str, language: Language) -> Validation {
        validate_data(text, language, 1, "/d/doc.json", &resolver(&[("/d/s.json", SCHEMA)]))
    }

    fn at<'t>(text: &'t str, v: &Validation, i: usize) -> &'t str {
        &text[v.problems[i].diagnostic.span.clone()]
    }

    #[test]
    fn a_json_document_is_checked_against_the_schema_it_names_and_each_problem_is_where_it_is() {
        let ok = r#"{"$schema": "s.json", "name": "x", "n": 2}"#;
        let v = check(ok, Language::Json(JsonFlavor::Json));
        assert_eq!((v.verdict, v.problems.len()), (Verdict::Valid, 0), "{:?}", v.problems);
        let bad = r#"{"$schema": "s.json", "n": 0, "extra": 1}"#;
        let v = check(bad, Language::Json(JsonFlavor::Json));
        assert_eq!(v.verdict, Verdict::Invalid);
        let found: Vec<(&str, Code)> = (0..v.problems.len()).map(|i| (at(bad, &v, i), v.problems[i].diagnostic.code)).collect();
        // `n` is below its minimum (at its name), `name` is required (at the object).
        assert!(found.contains(&("\"n\"", Code::JsonSchemaViolation)), "{found:?}");
        assert!(found.iter().any(|(s, c)| *c == Code::JsonSchemaViolation && s.starts_with('{')), "{found:?}");
        assert!(v.problems.iter().all(|p| p.file.is_none()));
    }

    #[test]
    fn yaml_names_its_schema_by_a_member_or_a_modeline() {
        let member = "$schema: s.json\nn: 0\n";
        let v = check(member, Language::Yaml);
        assert_eq!(v.verdict, Verdict::Invalid, "{:?}", v.problems);
        let places: Vec<&str> = (0..v.problems.len()).map(|i| at(member, &v, i)).collect();
        assert!(places.contains(&"n"), "the minimum, at the key: {places:?}");
        let line = "# yaml-language-server: $schema=s.json\nname: x\nn: 0\n";
        let v = check(line, Language::Yaml);
        assert_eq!(v.verdict, Verdict::Invalid);
        assert!((0..v.problems.len()).any(|i| at(line, &v, i) == "n"), "{:?}", v.problems);
        let fine = "# yaml-language-server: $schema=s.json\nname: x\nn: 3\n";
        assert_eq!(check(fine, Language::Yaml).verdict, Verdict::Valid);
    }

    #[test]
    fn json_lines_are_checked_line_by_line() {
        let text = "{\"$schema\":\"s.json\",\"name\":\"a\"}\n{\"name\":1}\n{\"name\":\"c\"}\n";
        let v = check(text, Language::Json(JsonFlavor::Lines));
        assert_eq!(v.verdict, Verdict::Invalid);
        assert_eq!(v.problems.len(), 1, "{:?}", v.problems);
        assert_eq!(at(text, &v, 0), "\"name\"", "the second line's name, found in the whole text");
        assert!(v.problems[0].diagnostic.span.start > text.find('\n').unwrap_or(0));
    }

    #[test]
    fn no_schema_is_no_problem_and_a_missing_schema_is_a_warning() {
        assert_eq!(check(r#"{"a": 1}"#, Language::Json(JsonFlavor::Json)).verdict, Verdict::Valid);
        // A document that names a draft is a schema of that draft, and is checked as one (an instance of the meta-schema).
        let draft = r#"{"$schema": "https://json-schema.org/draft/2020-12/schema", "type": 5}"#;
        let v = check(draft, Language::Json(JsonFlavor::Json));
        assert_eq!(v.verdict, Verdict::Invalid, "{:?}", v.problems);
        let missing = r#"{"$schema": "nope.json"}"#;
        let v = check(missing, Language::Json(JsonFlavor::Json));
        assert_eq!(v.verdict, Verdict::Valid, "a schema that cannot be read is said, not an error of the document");
        assert_eq!(v.problems[0].diagnostic.code, Code::JsonSchemaUnavailable);
        assert_eq!(at(missing, &v, 0), "\"nope.json\"");
    }

    #[test]
    fn a_text_with_a_syntax_error_is_not_checked_and_bson_and_xml_are_left_alone() {
        let v = check(r#"{"$schema": "s.json", "name": "#, Language::Json(JsonFlavor::Json));
        assert_eq!(v.verdict, Verdict::NotWellFormed);
        assert_eq!(check("{}", Language::Bson).verdict, Verdict::Valid);
        assert_eq!(check("<a/>", Language::Xml).verdict, Verdict::Valid);
        assert_eq!(schema_reference(r#"{"$schema": "s.json"}"#, Language::Bson).as_deref(), Some("s.json"));
        assert_eq!(
            schema_reference("{\"a\": 1}\n{\"$schema\": \"s.json\"}\n", Language::Bson).as_deref(),
            None,
            "the first document names it, as in JSON Lines"
        );
    }

    #[test]
    fn a_broken_schema_is_a_warning_and_a_schema_document_that_cannot_be_read_is_a_problem_of_the_schema() {
        let broken =
            validate_data(r#"{"$schema": "s.json"}"#, Language::Json(JsonFlavor::Json), 1, "/d/doc.json", &resolver(&[("/d/s.json", "{\"type\": ")]));
        assert_eq!(broken.problems[0].diagnostic.code, Code::JsonSchemaUnavailable);
        assert!(broken.problems[0].file.is_none(), "it is the document's warning");
        let missing = validate_data(
            r#"{"$schema": "s.json", "a": 1}"#,
            Language::Json(JsonFlavor::Json),
            1,
            "/d/doc.json",
            &resolver(&[("/d/s.json", r#"{"properties": {"a": {"$ref": "gone.json"}}}"#)]),
        );
        assert_eq!(missing.verdict, Verdict::Valid, "the instance is not at fault: {:?}", missing.problems);
        assert_eq!(missing.problems.len(), 1);
        assert_eq!(missing.problems[0].diagnostic.code, Code::JsonSchemaRefUnavailable);
        assert!(missing.problems[0].file.as_deref().is_some_and(|f| f.ends_with("s.json")), "{:?}", missing.problems[0].file);
        assert!(missing.problems[0].diagnostic.args.iter().any(|a| a.contains("gone.json")), "{:?}", missing.problems[0].diagnostic.args);
    }

    #[test]
    fn a_schema_may_refer_to_other_documents_beside_it() {
        let schemas = [("/d/s.json", r#"{"properties": {"p": {"$ref": "part.json"}}}"#), ("/d/part.json", r#"{"type": "string"}"#)];
        let v = validate_data(r#"{"$schema": "s.json", "p": 3}"#, Language::Json(JsonFlavor::Json), 1, "/d/doc.json", &resolver(&schemas));
        assert_eq!(v.verdict, Verdict::Invalid, "{:?}", v.problems);
    }

    #[test]
    fn a_referred_document_that_is_not_json_is_a_problem_of_that_file_and_a_plain_comment_is_no_modeline() {
        let schemas = [("/d/s.json", r#"{"properties": {"p": {"$ref": "part.json"}}}"#), ("/d/part.json", "{\"type\": ")];
        let v = validate_data(r#"{"$schema": "s.json", "p": 3}"#, Language::Json(JsonFlavor::Json), 1, "/d/doc.json", &resolver(&schemas));
        let invalid: Vec<_> = v.problems.iter().filter(|p| p.diagnostic.code == Code::JsonSchemaInvalid).collect();
        assert_eq!(invalid.len(), 1, "{:?}", v.problems);
        assert!(invalid[0].file.as_deref().is_some_and(|f| f.ends_with("part.json")), "{:?}", invalid[0].file);
        // A comment that mentions `$schema=` but is not the editor comment names nothing.
        let text = "# see $schema=s.json for the shape\nn: 0\n";
        assert_eq!(check(text, Language::Yaml).verdict, Verdict::Valid);
        assert!(!names_a_schema(text, Language::Yaml));
        assert!(names_a_schema("# yaml-language-server: $schema=s.json\nn: 0\n", Language::Yaml));
        assert!(!names_a_schema("{\"a\": ", Language::Json(JsonFlavor::Json)));
        assert!(!names_a_schema("{\"a\": 1}", Language::Json(JsonFlavor::Json)), "no `$schema` at all");
    }

    #[test]
    fn schemas_may_have_comments_in_the_documents_they_refer_to_too() {
        let schemas = [
            ("/d/s.json", "// the schema\n{\"properties\": {\"p\": {\"$ref\": \"part.json\"}}}"),
            ("/d/part.json", "{\n  // a string\n  \"type\": \"string\",\n}"),
        ];
        let v = validate_data(r#"{"$schema": "s.json", "p": 3}"#, Language::Json(JsonFlavor::Json), 1, "/d/doc.json", &resolver(&schemas));
        assert_eq!(v.verdict, Verdict::Invalid, "the string type is enforced, the comment is no problem: {:?}", v.problems);
        assert!(v.problems.iter().all(|p| p.diagnostic.code == Code::JsonSchemaViolation), "{:?}", v.problems);
    }

    fn run(text: &str, language: Language) -> Validation {
        validate_data(text, language, 1, "/d/doc", &resolver(&[]))
    }

    fn places<'t>(text: &'t str, v: &Validation) -> Vec<&'t str> {
        v.problems.iter().map(|p| text.get(p.diagnostic.span.clone()).unwrap_or("?")).collect()
    }

    #[test]
    fn a_document_that_names_a_draft_is_a_schema_and_is_checked_against_the_meta_schema_of_that_draft() {
        for (uri, bad) in [
            ("http://json-schema.org/draft-04/schema#", r#"{"$schema": "http://json-schema.org/draft-04/schema#", "type": "strin"}"#),
            ("http://json-schema.org/draft-06/schema#", r#"{"$schema": "http://json-schema.org/draft-06/schema#", "type": "strin"}"#),
            ("http://json-schema.org/draft-07/schema#", r#"{"$schema": "http://json-schema.org/draft-07/schema#", "type": "strin"}"#),
            ("https://json-schema.org/draft/2019-09/schema", r#"{"$schema": "https://json-schema.org/draft/2019-09/schema", "type": "strin"}"#),
            ("https://json-schema.org/draft/2020-12/schema", r#"{"$schema": "https://json-schema.org/draft/2020-12/schema", "type": "strin"}"#),
        ] {
            let v = run(bad, Language::Json(JsonFlavor::Json));
            assert_eq!(v.verdict, Verdict::Invalid, "{uri}: {:?}", v.problems);
            assert!(places(bad, &v).contains(&"\"type\""), "{uri}: the keyword is where the problem is: {:?}", places(bad, &v));
            let good = bad.replace("strin", "string");
            let v = run(&good, Language::Json(JsonFlavor::Json));
            assert_eq!(v.verdict, Verdict::Valid, "{uri}: {:?}", v.problems);
        }
        // The https spelling of an old draft is the same draft.
        let v = run(r#"{"$schema": "https://json-schema.org/draft-07/schema", "properties": 5}"#, Language::Json(JsonFlavor::Json));
        assert_eq!(v.verdict, Verdict::Invalid);
        // A YAML schema is checked too.
        let v = run("$schema: https://json-schema.org/draft/2020-12/schema\nrequired: yes\n", Language::Yaml);
        assert_eq!(v.verdict, Verdict::Invalid, "{:?}", v.problems);
    }

    #[test]
    fn an_openapi_description_is_checked_against_the_schema_of_its_version() {
        let doc = |head: &str, rest: &str| format!(r#"{{{head}, "info": {{"title": "t", "version": "1"}}{rest}}}"#);
        for (head, rest) in [
            (r#""swagger": "2.0""#, r#", "paths": {}"#),
            (r#""openapi": "3.0.3""#, r#", "paths": {}"#),
            (r#""openapi": "3.1.0""#, r#", "paths": {}"#),
            (r#""openapi": "3.2.0""#, r#", "paths": {}"#),
        ] {
            let good = doc(head, rest);
            let v = run(&good, Language::Json(JsonFlavor::Json));
            assert_eq!((v.verdict, v.problems.len()), (Verdict::Valid, 0), "{head}: {:?}", v.problems);
            // The same without `info`, which every version requires.
            let bad = format!("{{{head}{rest}}}");
            let v = run(&bad, Language::Json(JsonFlavor::Json));
            assert_eq!(v.verdict, Verdict::Invalid, "{head}: no info");
            // A wrong type for the title, at the title.
            let wrong = good.replace(r#""title": "t""#, r#""title": 5"#);
            let v = run(&wrong, Language::Json(JsonFlavor::Json));
            assert_eq!(v.verdict, Verdict::Invalid, "{head}");
            assert!(places(&wrong, &v).contains(&"\"title\""), "{head}: {:?}", places(&wrong, &v));
        }
        // YAML, with the version unquoted.
        let yaml = "openapi: 3.1.0\ninfo:\n  title: t\n  version: '1'\npaths: {}\n";
        assert_eq!(run(yaml, Language::Yaml).verdict, Verdict::Valid);
        assert_eq!(run("openapi: 3.1.0\ninfo: {}\npaths: {}\n", Language::Yaml).verdict, Verdict::Invalid);
        // A version that is not known is a problem at the version.
        let unknown = r#"{"openapi": "4.0.0", "info": {"title": "t", "version": "1"}}"#;
        let v = run(unknown, Language::Json(JsonFlavor::Json));
        assert_eq!((v.verdict, places(unknown, &v)), (Verdict::Invalid, vec!["\"4.0.0\""]));
        assert!(v.problems[0].diagnostic.args[0].contains("not known"), "{:?}", v.problems);
        // Schema Objects are validated for 3.1 and 3.2 (their dialect): a `type` that is no type is found inside a schema.
        let nested = r#"{"openapi": "3.1.0", "info": {"title": "t", "version": "1"}, "components": {"schemas": {"A": {"type": "strin"}}}}"#;
        let v = run(nested, Language::Json(JsonFlavor::Json));
        assert_eq!(v.verdict, Verdict::Invalid, "{:?}", v.problems);
        assert!(places(nested, &v).contains(&"\"type\""), "{:?}", places(nested, &v));
    }

    #[test]
    fn documents_that_name_an_openapi_version_or_a_draft_are_documents_that_name_a_schema() {
        assert!(names_a_schema(r#"{"openapi": "3.0.0", "info": {}}"#, Language::Json(JsonFlavor::Json)));
        assert!(names_a_schema(r#"{"swagger": "2.0"}"#, Language::Json(JsonFlavor::Json)));
        assert!(names_a_schema(r#"{"$schema": "http://json-schema.org/draft-07/schema#"}"#, Language::Json(JsonFlavor::Json)));
        assert!(!names_a_schema(r#"{"openapi": true}"#, Language::Json(JsonFlavor::Json)), "not a version");
        assert!(!names_a_schema(r#"{"note": "openapi"}"#, Language::Json(JsonFlavor::Json)));
        assert!(!names_a_schema("openapi: 3.0.0\ninfo: {\n", Language::Yaml), "broken text");
    }

    #[test]
    fn what_a_draft_adds_is_checked_by_that_draft_and_numbers_and_new_versions_are_found_too() {
        // `if` is a keyword of draft-07 and later, not of draft-06: only the meta-schema of 07 refuses `"if": 5`.
        let seven = r#"{"$schema": "http://json-schema.org/draft-07/schema#", "if": 5}"#;
        assert_eq!(run(seven, Language::Json(JsonFlavor::Json)).verdict, Verdict::Invalid);
        let six = r#"{"$schema": "http://json-schema.org/draft-06/schema#", "if": 5}"#;
        assert_eq!(run(six, Language::Json(JsonFlavor::Json)).verdict, Verdict::Valid, "draft-06 does not know `if`");
        // `swagger: 2.0` is a number in YAML.
        assert_eq!(run("swagger: 2.0\npaths: {}\n", Language::Yaml).verdict, Verdict::Invalid, "no info");
        // 3.2 validates its Schema Objects with its dialect too.
        let nested = r#"{"openapi": "3.2.0", "info": {"title": "t", "version": "1"}, "components": {"schemas": {"A": {"type": "strin"}}}}"#;
        let v = run(nested, Language::Json(JsonFlavor::Json));
        assert_eq!(v.verdict, Verdict::Invalid, "{:?}", v.problems);
        // 3.0 checks its Schema Objects with the schema's own definition of them.
        let old = r#"{"openapi": "3.0.3", "info": {"title": "t", "version": "1"}, "paths": {}, "components": {"schemas": {"A": {"type": "strin"}}}}"#;
        assert_eq!(run(old, Language::Json(JsonFlavor::Json)).verdict, Verdict::Invalid);
    }
}
