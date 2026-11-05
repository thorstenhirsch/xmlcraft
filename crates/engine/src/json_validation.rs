//! A JSON, JSONC, JSON5, JSON Lines or YAML document checked against the JSON Schema it names.
//!
//! A document names its schema with a `$schema` member of its root object (a URI or a path, relative to the document), or, in YAML, with
//! the editor comment `# yaml-language-server: $schema=URI`. A `$schema` that names a draft of JSON Schema (`https://json-schema.org/…`)
//! says what the document is, not what to check it against, and is not a reference. A document with no reference is not checked
//! beyond its syntax. The schema, and the documents it refers to, are read through a [`Resolver`] (the open documents first, so a schema
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
    /// Where `$schema` was written: the byte range of its value, and the value.
    reference: Option<(Range<usize>, String)>,
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
        Language::Xml | Language::Bson => return None,
    };
    let reference = roots.first().and_then(member_reference).or_else(|| if language == Language::Yaml { modeline(text) } else { None });
    Some(Instance { roots, reference, broken })
}

/// Whether the text names a schema to be checked against (see the module). Only the beginning of the text is looked at for a YAML
/// modeline; the root object is read.
pub fn names_a_schema(text: &str, language: Language) -> bool {
    // (Both ways of naming a schema contain `$schema`: a text without it is not read at all.)
    text.len() <= MAX_ANALYSED_BYTES && text.contains("$schema") && read(text, language).is_some_and(|i| !i.broken && i.reference.is_some())
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

/// Validate `text` (version `version` of a document of this `language` at `location`, which a relative `$schema` is taken from).
pub fn validate_data(text: &str, language: Language, version: u64, location: &str, resolver: &Resolver) -> Validation {
    if text.len() > MAX_ANALYSED_BYTES {
        return Validation { version, verdict: Verdict::Skipped(Skipped::TooLarge), problems: Vec::new(), truncated: false };
    }
    let Some(instance) = read(text, language) else { return finish(version, Vec::new()) };
    if instance.broken {
        return Validation { version, verdict: Verdict::NotWellFormed, problems: Vec::new(), truncated: false };
    }
    let Some((span, reference)) = instance.reference else { return finish(version, Vec::new()) };
    let documents = Documents { resolver, budget: RefCell::new(Budget::default()), failed: RefCell::new(Vec::new()) };
    let own = |code: Code, span: Range<usize>, args: Vec<String>| ValidityProblem { diagnostic: Diagnostic::new(code, span, args), file: None };
    let (schema_location, schema_text) = match documents.fetch(&reference, location) {
        Ok(found) => found,
        Err(why) => return finish(version, vec![own(Code::JsonSchemaUnavailable, span, vec![reference, why])]),
    };
    // The schema may have comments and trailing commas (JSONC), as editors allow.
    let schema = parse(&schema_text, &Options { dialect: Dialect::Json5, ..Options::default() });
    if let Some(d) = schema.diagnostics.iter().find(|d| d.severity() == Severity::Error) {
        let note = format!("{schema_location} is not valid JSON: {}", d.message());
        return finish(version, vec![own(Code::JsonSchemaUnavailable, span, vec![reference, note])]);
    }
    let Some(root) = schema.root else {
        return finish(version, vec![own(Code::JsonSchemaUnavailable, span, vec![reference, "the schema has no value".to_owned()])]);
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
    fn no_schema_is_no_problem_a_draft_is_not_a_schema_and_a_missing_schema_is_a_warning() {
        assert_eq!(check(r#"{"a": 1}"#, Language::Json(JsonFlavor::Json)).verdict, Verdict::Valid);
        let draft = r#"{"$schema": "https://json-schema.org/draft/2020-12/schema", "type": 5}"#;
        let v = check(draft, Language::Json(JsonFlavor::Json));
        assert_eq!((v.verdict, v.problems.len()), (Verdict::Valid, 0), "a draft names the dialect, not a schema to check against");
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
}
