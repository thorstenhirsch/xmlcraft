//! A sample JSON, YAML or BSON value made from a JSON Schema: the work behind Generate Sample JSON… and the tool `json_sample`.
//!
//! The sample follows the names the schema declares (the required ones, and the others when `optional` is on), the first value it offers
//! (`enum`, `const`, a default or an example), and the kind of each place: an object from `properties`, one item from `items`, a string,
//! a number, a boolean or null. A recursive schema is cut at `max_depth`, and a sample of more than [`MAX_VALUES`] values is refused (a
//! schema whose objects refer to themselves through several names grows by their number at every level); a place the schema says
//! nothing about is null. The sample is not promised to validate, but it shows the shape the schema describes.

use std::sync::Arc;

use xmlcraft_convert::{ConvertOptions, Format, convert, json_text};
use xmlcraft_json::SchemaSet;
use xmlcraft_json_syntax::{Data, Dialect, Member, Options as JsonOptions, Segment, Value, parse};
use xmlcraft_text::Buffer;

use crate::json_validation::load_schema_from_text;
use crate::validation::{OpenDocuments, ResolverFactory};
use crate::worker::Worker;

/// How deep a sample goes before only the empty value of a place is made.
pub const MAX_DEPTH: usize = 32;
/// The most properties one object in a sample has.
pub const MAX_PROPERTIES: usize = 100;
/// The most values one sample has, all of its objects, arrays and scalars together.
pub const MAX_VALUES: usize = 10_000;

/// How to make a sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// What the sample is written in: JSON, YAML or BSON (as Extended JSON).
    pub format: Format,
    /// Make the properties the schema does not require as well.
    pub optional: bool,
    /// How deep objects and arrays nest before they are made empty.
    pub max_depth: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self { format: Format::Json, optional: true, max_depth: 6 }
    }
}

/// A sample and what could not be kept when it was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Done {
    pub text: String,
    pub warnings: Vec<String>,
}

/// A sample of `schema`, written in the format of `options`.
pub fn sample(schema: &SchemaSet, options: &Options) -> Result<Done, String> {
    let mut maker = Maker { schema, optional: options.optional, max_depth: options.max_depth.min(MAX_DEPTH), made: 0, path: Vec::new() };
    let value = maker.value(0)?.ok_or_else(|| "the schema says nothing that a sample can be made from".to_owned())?;
    let mut warnings = Vec::new();
    let json = json_text(&value, Some(2), &mut warnings).map_err(|e| e.to_string())?;
    let text = if options.format == Format::Json {
        json
    } else {
        let converted = convert(&json, Format::Json, options.format, &ConvertOptions { indent: Some(2), ..ConvertOptions::default() })
            .map_err(|e| e.to_string())?;
        warnings.extend(converted.warnings);
        converted.text
    };
    Ok(Done { text, warnings })
}

/// Makes the values of a sample, with the path of the place being made (one stack, pushed and popped, rather than a copy per place).
struct Maker<'a> {
    schema: &'a SchemaSet,
    optional: bool,
    max_depth: usize,
    /// The values made so far.
    made: usize,
    path: Vec<Segment>,
}

fn empty(data: Data) -> Value {
    Value { span: 0..0, data }
}

impl Maker<'_> {
    /// The value at `self.path`; `None` only at the top, where a schema that describes nothing makes no sample.
    fn value(&mut self, depth: usize) -> Result<Option<Value>, String> {
        self.made = self.made.saturating_add(1);
        if self.made > MAX_VALUES {
            return Err(format!(
                "the sample would be too large (more than {MAX_VALUES} values): make it less deep, or leave out the names that are not required"
            ));
        }
        let docs = self.schema.describe_at(&self.path);
        if docs.is_some()
            && let Some(value) = first_offered(self.schema, &self.path)
        {
            return Ok(Some(value));
        }
        let properties = self.schema.properties_at(&self.path, &[]);
        let types: &[String] = docs.as_ref().map_or(&[], |d| &d.types);
        let object = types.iter().any(|t| t == "object") || (types.is_empty() && !properties.is_empty());
        if object {
            if depth >= self.max_depth {
                return Ok(Some(empty(Data::Object(Vec::new()))));
            }
            let mut members = Vec::new();
            let optional = self.optional;
            for hint in properties.iter().filter(|p| optional || p.required).take(MAX_PROPERTIES) {
                self.path.push(Segment::Key(hint.name.clone()));
                let made = self.value(depth.saturating_add(1));
                self.path.pop();
                let value = made?.unwrap_or_else(|| empty(Data::Null));
                members.push(Member { key: hint.name.clone(), key_span: 0..0, value });
            }
            return Ok(Some(empty(Data::Object(members))));
        }
        self.path.push(Segment::Index(0));
        let array = types.iter().any(|t| t == "array") || (types.is_empty() && self.schema.describe_at(&self.path).is_some());
        let items = if !array || depth >= self.max_depth { Ok(None) } else { self.value(depth.saturating_add(1)) };
        self.path.pop();
        if array {
            return Ok(Some(empty(Data::Array(items?.into_iter().collect()))));
        }
        let chosen = types.iter().find(|t| t.as_str() != "null").or_else(|| types.first());
        Ok(Some(match chosen.map(String::as_str) {
            Some("boolean") => empty(Data::Bool(true)),
            Some("integer" | "number") => empty(Data::Number("0".to_owned())),
            Some("string") => empty(Data::String("string".to_owned())),
            Some("object") => empty(Data::Object(Vec::new())),
            Some("array") => empty(Data::Array(Vec::new())),
            _ => empty(Data::Null),
        }))
    }
}

/// The first value the schema offers at `path` (`enum`, `const`, a default or an example), read as JSON.
fn first_offered(schema: &SchemaSet, path: &[Segment]) -> Option<Value> {
    let hint = schema.values_at(path).into_iter().next()?;
    let read = parse(&hint.text, &JsonOptions { dialect: Dialect::Json5, ..JsonOptions::default() });
    read.root.filter(|_| read.diagnostics.iter().all(|d| d.severity() != xmlcraft_json_syntax::Severity::Error))
}

// --- the worker ----------------------------------------------------------------------------------------------------------------

/// One request for a sample (Generate Sample JSON…).
pub struct SampleJob {
    /// Says which request a result answers.
    pub serial: u64,
    /// The text of the schema and where it is (empty when it has no file): what its relative references are read from.
    pub schema: String,
    pub location: String,
    /// The documents that are open, by their paths, so that a schema being edited is used as it is.
    pub open: Vec<(String, Buffer)>,
    pub options: Options,
}

pub struct SampleOutcome {
    pub serial: u64,
    pub result: Result<Done, String>,
}

fn run(resolvers: &ResolverFactory, job: SampleJob) -> SampleOutcome {
    let resolver = resolvers(Arc::new(OpenDocuments::new(job.open)));
    let result = crate::guard(|| load_schema_from_text(&job.schema, &job.location, &resolver).and_then(|set| sample(&set, &job.options)))
        .unwrap_or_else(|panic| {
            log::error!("making a sample from a JSON Schema failed: {panic}");
            Err("generating the sample failed (a bug); nothing was changed".to_owned())
        });
    SampleOutcome { serial: job.serial, result }
}

/// Makes samples on a worker thread: the schema's references are read through the resolver, which may download.
pub struct JsonSampleGenerator {
    worker: Worker<SampleJob, SampleOutcome>,
}

impl JsonSampleGenerator {
    /// A generator whose requests read the documents a schema refers to with the resolvers `resolvers` makes.
    pub fn new(resolvers: ResolverFactory) -> Self {
        Self { worker: Worker::new("json sample generation", move |job| run(&resolvers, job)) }
    }

    /// A generator with no thread: `submit` works in the caller's thread (what the web does, and handy for tests).
    pub fn inline(resolvers: ResolverFactory) -> Self {
        Self { worker: Worker::inline(move |job| run(&resolvers, job)) }
    }

    pub fn on_done(&self, notify: impl Fn() + Send + Sync + 'static) {
        self.worker.on_done(notify);
    }

    pub fn submit(&self, job: SampleJob) {
        self.worker.submit(0, job);
    }

    pub fn poll(&self) -> Vec<SampleOutcome> {
        self.worker.poll().into_iter().map(|(_, outcome)| outcome).collect()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::time::{Duration, Instant};

    use xmlcraft_json::{Draft, NoLoader};

    use super::*;

    fn set(text: &str) -> SchemaSet {
        let read = parse(text, &JsonOptions::default());
        assert!(read.diagnostics.iter().all(|d| d.severity() != xmlcraft_json_syntax::Severity::Error));
        SchemaSet::new(read.root.expect("a schema"), "", Draft::D2020_12, &NoLoader)
    }

    #[test]
    fn required_names_and_the_kind_of_each_place_make_the_shape() {
        let schema = set(
            r#"{"type":"object","required":["name"],"properties":{"name":{"type":"string"},"age":{"type":"integer"},"tags":{"type":"array","items":{"type":"string"}}}}"#,
        );
        let made = sample(&schema, &Options { optional: false, ..Options::default() }).expect("a sample");
        let value: serde_json::Value = serde_json::from_str(&made.text).expect("JSON");
        assert_eq!(value, serde_json::json!({ "name": "string" }));
        let made = sample(&schema, &Options::default()).expect("a sample");
        let value: serde_json::Value = serde_json::from_str(&made.text).expect("JSON");
        assert_eq!(value, serde_json::json!({ "name": "string", "age": 0, "tags": ["string"] }));
    }

    #[test]
    fn an_offered_value_is_used_before_the_kind() {
        let schema = set(
            r#"{"type":"object","properties":{"color":{"type":"string","enum":["red","blue"]},"size":{"type":"integer","default":3},"flag":{"type":"boolean","const":false}}}"#,
        );
        let made = sample(&schema, &Options::default()).expect("a sample");
        let value: serde_json::Value = serde_json::from_str(&made.text).expect("JSON");
        assert_eq!(value, serde_json::json!({ "color": "red", "size": 3, "flag": false }));
    }

    #[test]
    fn a_recursive_schema_stops_at_the_depth_and_the_formats_are_written() {
        let schema = set(r##"{"type":"object","properties":{"child":{"$ref":"#"}}}"##);
        let made = sample(&schema, &Options { max_depth: 2, ..Options::default() }).expect("a sample");
        let value: serde_json::Value = serde_json::from_str(&made.text).expect("JSON");
        assert_eq!(value, serde_json::json!({ "child": { "child": {} } }), "the sample is cut at the depth");
        let yaml = sample(&schema, &Options { format: Format::Yaml, max_depth: 1, ..Options::default() }).expect("YAML");
        assert!(yaml.text.contains("child:"), "{}", yaml.text);
        let bson = sample(&schema, &Options { format: Format::Bson, max_depth: 1, ..Options::default() }).expect("BSON");
        assert!(bson.text.contains("\"child\""), "{}", bson.text);
    }

    #[test]
    fn a_schema_that_refers_to_itself_through_many_names_is_refused_quickly_rather_than_made_huge() {
        // Twelve names that are each the whole schema: twelve to the power of the depth places, unless the budget stops it.
        let names: Vec<String> = ('a'..='l').map(|n| format!("\"{n}\":{{\"$ref\":\"#\"}}")).collect();
        let schema = set(&format!("{{\"properties\":{{{}}}}}", names.join(",")));
        let started = Instant::now();
        let made = sample(&schema, &Options { max_depth: MAX_DEPTH, ..Options::default() });
        assert!(made.as_ref().is_err_and(|e| e.contains("too large")), "{made:?}");
        assert!(started.elapsed() < Duration::from_secs(20), "it stops at the budget: {:?}", started.elapsed());
        // Shallow enough, the same schema makes a sample within the budget.
        let made = sample(&schema, &Options { max_depth: 2, ..Options::default() }).expect("a bounded sample");
        let value: serde_json::Value = serde_json::from_str(&made.text).expect("JSON");
        assert_eq!(value["a"]["b"], serde_json::json!({}));
    }
}
