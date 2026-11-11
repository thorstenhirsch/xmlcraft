//! A JSON Schema learned from JSON, YAML and BSON documents: the objects, arrays, types and names the documents show.
//!
//! The result describes what was seen, not what was meant: a name is required when every object seen has it, an item is the shape of
//! every item seen, and a string is an enumeration when the values repeat enough (the same rule the XML schema inference uses). It is a
//! starting point to edit, not a proof of anything about documents that were not read.

use std::collections::BTreeMap;

use serde_json::{Map, Value as Json, json};
use xmlcraft_json_syntax::{Data, Options as ParseOptions, Value, parse, parse_lines};

use crate::language::{JsonFlavor, Language};
use crate::worker::Worker;

/// The most documents one call reads.
pub const MAX_DOCUMENTS: usize = 1000;
/// The most bytes of all the documents together.
pub const MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;
/// The most values observed.
const MAX_OBSERVATIONS: usize = 1_000_000;
/// The most different strings kept for the enumeration rule.
const MAX_STRING_VALUES: usize = 10_000;
/// The deepest JSON or YAML value followed (the readers cut deeper ones off themselves).
const MAX_DEPTH: usize = 256;

/// How strings become an enumeration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnumerationRule {
    /// At most this many different values.
    pub max_values: usize,
    /// Each seen at least this often.
    pub min_occurrences: u64,
}

impl Default for EnumerationRule {
    fn default() -> EnumerationRule {
        EnumerationRule { max_values: 10, min_occurrences: 2 }
    }
}

/// What to infer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// `None`: no enumerations.
    pub enumerations: Option<EnumerationRule>,
}

impl Default for Options {
    fn default() -> Options {
        Options { enumerations: Some(EnumerationRule::default()) }
    }
}

/// A schema and what it was learned from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inferred {
    pub schema: String,
    pub documents: usize,
}

#[derive(Default)]
struct Shape {
    /// How many values this shape describes.
    count: u64,
    nulls: u64,
    booleans: u64,
    numbers: u64,
    integers: u64,
    strings: u64,
    arrays: u64,
    objects: u64,
    string_values: BTreeMap<String, u64>,
    /// More different strings than the rule can hold.
    many_strings: bool,
    items: Option<Box<Shape>>,
    properties: BTreeMap<String, Shape>,
}

impl Shape {
    fn add_value(&mut self, value: &Value, depth: usize, budget: &mut Budget) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err(format!("a value is nested deeper than {MAX_DEPTH}"));
        }
        budget.observations = budget.observations.saturating_add(1);
        if budget.observations > MAX_OBSERVATIONS {
            return Err(format!("more than {MAX_OBSERVATIONS} values to learn from"));
        }
        self.count = self.count.saturating_add(1);
        match &value.data {
            Data::Null => self.nulls = self.nulls.saturating_add(1),
            Data::Bool(_) => self.booleans = self.booleans.saturating_add(1),
            Data::Number(number) => {
                self.numbers = self.numbers.saturating_add(1);
                if number.parse::<f64>().is_ok_and(|n| n.is_finite() && n.fract() == 0.0) {
                    self.integers = self.integers.saturating_add(1);
                }
            }
            Data::String(text) => {
                self.strings = self.strings.saturating_add(1);
                if let Some(count) = self.string_values.get_mut(text) {
                    *count = count.saturating_add(1);
                } else if self.string_values.len() < MAX_STRING_VALUES {
                    self.string_values.insert(text.clone(), 1);
                } else {
                    self.many_strings = true;
                }
            }
            Data::Array(items) => {
                self.arrays = self.arrays.saturating_add(1);
                for item in items {
                    let shape = self.items.get_or_insert_with(|| Box::new(Shape::default()));
                    shape.add_value(item, depth + 1, budget)?;
                }
            }
            Data::Object(members) => {
                self.objects = self.objects.saturating_add(1);
                for member in members {
                    let property = self.properties.entry(member.key.clone()).or_default();
                    property.add_value(&member.value, depth + 1, budget)?;
                }
            }
        }
        Ok(())
    }

    fn types(&self) -> Vec<&'static str> {
        let mut types = Vec::new();
        if self.nulls > 0 {
            types.push("null");
        }
        if self.booleans > 0 {
            types.push("boolean");
        }
        if self.numbers > 0 {
            types.push(if self.integers == self.numbers { "integer" } else { "number" });
        }
        if self.strings > 0 {
            types.push("string");
        }
        if self.arrays > 0 {
            types.push("array");
        }
        if self.objects > 0 {
            types.push("object");
        }
        types
    }

    fn enumeration(&self, rule: EnumerationRule) -> Option<Vec<&str>> {
        if self.many_strings || self.strings == 0 || self.strings != self.count {
            return None;
        }
        if self.string_values.len() > rule.max_values || self.string_values.values().any(|count| *count < rule.min_occurrences) {
            return None;
        }
        Some(self.string_values.keys().map(String::as_str).collect())
    }

    fn schema(&self, options: &Options) -> Json {
        let mut schema = Map::new();
        let types = self.types();
        match types.as_slice() {
            [] => {}
            [one] => {
                schema.insert("type".into(), json!(one));
            }
            many => {
                schema.insert("type".into(), json!(many));
            }
        }
        if self.objects > 0 && !self.properties.is_empty() {
            let properties: Map<String, Json> = self.properties.iter().map(|(name, shape)| (name.clone(), shape.schema(options))).collect();
            schema.insert("properties".into(), Json::Object(properties));
            let required: Vec<&str> =
                self.properties.iter().filter(|(_, shape)| shape.count == self.objects).map(|(name, _)| name.as_str()).collect();
            if !required.is_empty() {
                schema.insert("required".into(), json!(required));
            }
        }
        if let Some(items) = &self.items
            && self.arrays > 0
            && items.count > 0
        {
            schema.insert("items".into(), items.schema(options));
        }
        if let Some(rule) = options.enumerations
            && let Some(values) = self.enumeration(rule)
        {
            schema.insert("enum".into(), json!(values));
        }
        Json::Object(schema)
    }
}

#[derive(Default)]
struct Budget {
    observations: usize,
}

/// One document to learn from: its text and what it is written in.
pub struct Document<'a> {
    pub text: &'a str,
    pub language: Language,
}

/// A JSON Schema (draft 2020-12) learned from `documents`.
pub fn infer(documents: &[Document<'_>], options: &Options) -> Result<Inferred, String> {
    if documents.is_empty() {
        return Err("there is no document to learn from".into());
    }
    if documents.len() > MAX_DOCUMENTS {
        return Err(format!("at most {MAX_DOCUMENTS} documents are read"));
    }
    let bytes = documents.iter().fold(0usize, |sum, d| sum.saturating_add(d.text.len()));
    if bytes > MAX_TOTAL_BYTES {
        return Err(format!("the documents have more than {MAX_TOTAL_BYTES} bytes together"));
    }
    let mut shape = Shape::default();
    let mut budget = Budget::default();
    for (index, document) in documents.iter().enumerate() {
        let values = read(document.text, document.language).map_err(|e| format!("document {}: {e}", index + 1))?;
        if values.is_empty() {
            return Err(format!("document {} has no value", index + 1));
        }
        for value in values {
            shape.add_value(&value, 0, &mut budget)?;
        }
    }
    let mut schema = Map::new();
    schema.insert("$schema".into(), json!("https://json-schema.org/draft/2020-12/schema"));
    if let Json::Object(shape) = shape.schema(options) {
        schema.extend(shape);
    }
    let schema = serde_json::to_string_pretty(&Json::Object(schema)).map_err(|e| e.to_string())?;
    Ok(Inferred { schema: format!("{schema}\n"), documents: documents.len() })
}

// --- the worker ----------------------------------------------------------------------------------------------------------------

/// One request for a schema (Generate JSON Schema…): the texts and their languages.
pub struct InferJob {
    /// Says which request a result answers.
    pub serial: u64,
    pub documents: Vec<(String, Language)>,
    pub options: Options,
}

pub struct InferOutcome {
    pub serial: u64,
    pub result: Result<Inferred, String>,
}

fn run(job: InferJob) -> InferOutcome {
    let documents: Vec<Document<'_>> = job.documents.iter().map(|(text, language)| Document { text, language: *language }).collect();
    let result = crate::guard(|| infer(&documents, &job.options)).unwrap_or_else(|panic| {
        log::error!("inferring a JSON Schema failed: {panic}");
        Err("generating the schema failed (a bug); nothing was changed".to_owned())
    });
    InferOutcome { serial: job.serial, result }
}

/// Learns JSON Schemas on a worker thread, so the window does not wait for a large document. `submit` returns at once; `poll`
/// collects what has finished.
pub struct JsonSchemaGenerator {
    worker: Worker<InferJob, InferOutcome>,
}

impl JsonSchemaGenerator {
    pub fn new() -> Self {
        Self { worker: Worker::new("json schema inference", run) }
    }

    /// A generator with no thread: `submit` works in the caller's thread (what the web does, and handy for tests).
    pub fn inline() -> Self {
        Self { worker: Worker::inline(run) }
    }

    pub fn on_done(&self, notify: impl Fn() + Send + Sync + 'static) {
        self.worker.on_done(notify);
    }

    pub fn submit(&self, job: InferJob) {
        self.worker.submit(0, job);
    }

    pub fn poll(&self) -> Vec<InferOutcome> {
        self.worker.poll().into_iter().map(|(_, outcome)| outcome).collect()
    }
}

impl Default for JsonSchemaGenerator {
    fn default() -> Self {
        Self::new()
    }
}

/// The values of a document: one for JSON, one to a line for JSON Lines and a BSON dump, one per YAML document.
fn read(text: &str, language: Language) -> Result<Vec<Value>, String> {
    match language {
        Language::Json(JsonFlavor::Lines) => lines(text),
        Language::Json(flavor) => {
            let parsed = parse(text, &ParseOptions { dialect: flavor.dialect(), ..ParseOptions::default() });
            problems(&parsed)?;
            Ok(parsed.root.into_iter().collect())
        }
        Language::Bson => match xmlcraft_bson::layout_of_text(text) {
            xmlcraft_bson::Layout::Lines => lines(text),
            xmlcraft_bson::Layout::Single => {
                let parsed = parse(text, &ParseOptions::default());
                problems(&parsed)?;
                Ok(parsed.root.into_iter().collect())
            }
        },
        Language::Yaml => {
            let parsed = xmlcraft_yaml_syntax::parse(text, &xmlcraft_yaml_syntax::Options::default());
            if let Some(problem) = parsed.diagnostics.iter().find(|d| d.severity() == xmlcraft_json_syntax::Severity::Error) {
                return Err(problem.message());
            }
            Ok(parsed.documents)
        }
        Language::Xml | Language::Html => Err("a JSON Schema is learned from JSON, YAML or BSON, not XML or HTML".into()),
    }
}

fn lines(text: &str) -> Result<Vec<Value>, String> {
    let mut values = Vec::new();
    for (_, parsed) in parse_lines(text, &ParseOptions::default()) {
        problems(&parsed)?;
        if let Some(root) = parsed.root {
            values.push(root);
        }
    }
    Ok(values)
}

fn problems(parsed: &xmlcraft_json_syntax::Parse) -> Result<(), String> {
    match parsed.diagnostics.iter().find(|d| d.severity() == xmlcraft_json_syntax::Severity::Error) {
        Some(problem) => Err(problem.message()),
        None => Ok(()),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn infer_one(text: &str) -> Json {
        let inferred = infer(&[Document { text, language: Language::Json(JsonFlavor::Json) }], &Options::default()).expect("a schema");
        serde_json::from_str(&inferred.schema).expect("JSON")
    }

    #[test]
    fn objects_keep_their_names_and_the_names_every_object_has_are_required() {
        let schema = infer_one(r#"{"name": "a", "age": 3, "home": {"city": "x"}}"#);
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["name"]["type"], "string");
        assert_eq!(schema["properties"]["age"]["type"], "integer");
        assert_eq!(schema["properties"]["home"]["properties"]["city"]["type"], "string");
        assert_eq!(schema["required"], json!(["age", "home", "name"]));
        let inferred = infer(
            &[
                Document { text: r#"{"name": "a", "age": 1}"#, language: Language::Json(JsonFlavor::Json) },
                Document { text: r#"{"name": "b", "home": {"city": "y"}}"#, language: Language::Json(JsonFlavor::Json) },
            ],
            &Options::default(),
        )
        .expect("a schema");
        let schema: Json = serde_json::from_str(&inferred.schema).expect("JSON");
        assert_eq!(schema["required"], json!(["name"]));
        assert_eq!(inferred.documents, 2);
    }

    #[test]
    fn arrays_merge_every_item_and_a_number_is_a_number_when_a_fraction_is_seen() {
        let schema = infer_one(r#"{"items": [{"n": 1}, {"n": 2.5, "kind": "x"}], "tags": ["a", "b"]}"#);
        assert_eq!(schema["properties"]["items"]["type"], "array");
        assert_eq!(schema["properties"]["items"]["items"]["properties"]["n"]["type"], "number");
        assert_eq!(schema["properties"]["tags"]["items"]["type"], "string");
    }

    #[test]
    fn strings_that_repeat_are_an_enumeration_and_the_rule_can_turn_it_off() {
        let schema = infer_one(r#"["red", "green", "red", "green"]"#);
        assert_eq!(schema["items"]["enum"], json!(["green", "red"]));
        let inferred =
            infer(&[Document { text: r#"["red", "green"]"#, language: Language::Json(JsonFlavor::Json) }], &Options { enumerations: None })
                .expect("a schema");
        let schema: Json = serde_json::from_str(&inferred.schema).expect("JSON");
        assert_eq!(schema["items"].get("enum"), None);
    }

    #[test]
    fn yaml_and_bson_text_are_read_and_a_broken_document_is_refused() {
        let yaml = infer(&[Document { text: "name: a\nitems:\n  - 1\n  - 2\n", language: Language::Yaml }], &Options::default()).expect("a schema");
        let schema: Json = serde_json::from_str(&yaml.schema).expect("JSON");
        assert_eq!(schema["properties"]["items"]["items"]["type"], "integer");
        let bson = infer(&[Document { text: "{\"a\": 1}\n{\"a\": 2}\n", language: Language::Bson }], &Options::default()).expect("a schema");
        let schema: Json = serde_json::from_str(&bson.schema).expect("JSON");
        assert_eq!(schema["required"], json!(["a"]));
        assert!(infer(&[Document { text: "{", language: Language::Json(JsonFlavor::Json) }], &Options::default()).is_err());
        assert!(infer(&[Document { text: "<a/>", language: Language::Xml }], &Options::default()).is_err());
    }

    #[test]
    fn too_many_documents_or_bytes_are_refused_before_anything_is_read() {
        let many: Vec<Document<'_>> = (0..=MAX_DOCUMENTS).map(|_| Document { text: "{}", language: Language::Json(JsonFlavor::Json) }).collect();
        assert!(infer(&many, &Options::default()).is_err_and(|e| e.contains("at most")));
        let big = " ".repeat(MAX_TOTAL_BYTES / 2 + 1);
        let two = [
            Document { text: &big, language: Language::Json(JsonFlavor::Json) },
            Document { text: &big, language: Language::Json(JsonFlavor::Json) },
        ];
        assert!(infer(&two, &Options::default()).is_err_and(|e| e.contains("bytes")));
    }

    #[test]
    fn the_generator_answers_the_request_it_was_given() {
        let generator = JsonSchemaGenerator::inline();
        generator.submit(InferJob {
            serial: 7,
            documents: vec![("{\"a\": 1}".to_owned(), Language::Json(JsonFlavor::Json))],
            options: Options::default(),
        });
        let outcomes = generator.poll();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].serial, 7);
        assert!(outcomes[0].result.as_ref().is_ok_and(|i| i.schema.contains("\"a\"")));
    }

    #[test]
    fn a_null_and_a_string_together_are_both_types() {
        let schema = infer_one(r#"{"a": null, "b": "x"}"#);
        assert_eq!(schema["properties"]["a"]["type"], "null");
        let inferred = infer(
            &[
                Document { text: r#"{"a": null}"#, language: Language::Json(JsonFlavor::Json) },
                Document { text: r#"{"a": "x"}"#, language: Language::Json(JsonFlavor::Json) },
            ],
            &Options::default(),
        )
        .expect("a schema");
        let schema: Json = serde_json::from_str(&inferred.schema).expect("JSON");
        assert_eq!(schema["properties"]["a"]["type"], json!(["null", "string"]));
    }
}
