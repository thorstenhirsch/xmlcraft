//! What a schema says about a place in a document, for completion and hover: the names that may be written in an object, the values that
//! may be written in a place, and the words the schema has about it.
//!
//! A place is a path of keys and indices from the root of the document. The schemas that apply to it are found by walking the path
//! through `properties`, `patternProperties`, `additionalProperties`, `items` and `prefixItems`, and at each step by following `$ref`,
//! `allOf`, `anyOf`, `oneOf` and `then`/`else` (all of the alternatives: nothing is known of the value that is not yet written, so what
//! any of them allows is offered). Conditions (`if`) are not evaluated. The work is bounded: the schemas followed at a place, the depth
//! of references and the size of what is offered are capped.

use std::collections::HashSet;

use regex::RegexBuilder;
use xmlcraft_json_syntax::{Data, Segment, Value};

use crate::schema::{Place, SchemaSet};
use crate::uri;

/// The most schemas that apply at one place.
pub const MAX_NODES: usize = 128;
/// The most names offered for an object.
pub const MAX_PROPERTIES: usize = 500;
/// The most values offered for a place.
pub const MAX_VALUES: usize = 200;
/// The longest a value taken from a schema may be, written out.
const MAX_LITERAL: usize = 400;
const MAX_LITERAL_DEPTH: usize = 6;
const REGEX_SIZE_LIMIT: usize = 1 << 18;

/// A name that may be written in an object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyHint {
    pub name: String,
    /// A schema that applies to the object lists it in `required`.
    pub required: bool,
    /// What the schema says the value is (`string`, `integer`, …), without repeats.
    pub types: Vec<String>,
    pub description: Option<String>,
    /// The default value as JSON text, when the schema has one.
    pub default: Option<String>,
    pub deprecated: bool,
}

/// A value that may be written in a place, as JSON text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValueHint {
    pub text: String,
    /// Why it is offered: `enum`, `const`, `default`, `example` or `type`.
    pub source: &'static str,
}

/// The words of the schemas that apply to a place.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Docs {
    pub title: Option<String>,
    pub description: Option<String>,
    pub types: Vec<String>,
    /// The values of `enum` and `const` as JSON text.
    pub values: Vec<String>,
    pub default: Option<String>,
    pub deprecated: bool,
}

impl Docs {
    /// Whether the schema has nothing to say.
    pub fn is_empty(&self) -> bool {
        self.title.is_none() && self.description.is_none() && self.types.is_empty() && self.values.is_empty() && self.default.is_none()
    }
}

/// A schema (an object) and the base URI it is read with.
#[derive(Clone)]
struct Node {
    place: Place,
    base: String,
}

fn text(value: &Value, key: &str) -> Option<String> {
    match &value.get(key)?.data {
        Data::String(s) if !s.trim().is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// `value` as compact JSON text; none when it is too deep or long.
fn literal(value: &Value) -> Option<String> {
    let mut out = String::new();
    write_literal(value, &mut out, 0).then_some(out)
}

fn write_literal(value: &Value, out: &mut String, depth: usize) -> bool {
    if depth > MAX_LITERAL_DEPTH || out.len() > MAX_LITERAL {
        return false;
    }
    match &value.data {
        Data::Null => out.push_str("null"),
        Data::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        // (JSON5 spellings such as `0xFF` or `Infinity` are not JSON: a number that is not one is not offered.)
        Data::Number(n) => {
            if n.parse::<f64>().is_err() || !n.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.' | b'e' | b'E')) {
                return false;
            }
            out.push_str(n);
        }
        Data::String(s) => write_string(s, out),
        Data::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                if !write_literal(item, out, depth + 1) {
                    return false;
                }
            }
            out.push(']');
        }
        Data::Object(members) => {
            out.push('{');
            for (i, m) in members.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(&m.key, out);
                out.push(':');
                if !write_literal(&m.value, out, depth + 1) {
                    return false;
                }
            }
            out.push('}');
        }
    }
    out.len() <= MAX_LITERAL
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
        if out.len() > MAX_LITERAL {
            break;
        }
    }
    out.push('"');
}

impl SchemaSet {
    fn value_of(&self, place: &Place) -> Option<&Value> {
        self.docs.get(place.doc).and_then(|d| d.value.pointer(&place.pointer)).filter(|v| matches!(v.data, Data::Object(_)))
    }

    /// The schemas that apply at the root of the document.
    fn root_nodes(&self) -> Vec<Node> {
        match self.docs.first() {
            Some(doc) => vec![Node { place: Place { doc: 0, pointer: String::new() }, base: doc.base.clone() }],
            None => Vec::new(),
        }
    }

    /// `nodes` and every schema they lead to by a reference or a combination, each once.
    fn expand(&self, nodes: Vec<Node>) -> Vec<Node> {
        let mut seen: HashSet<(usize, String)> = HashSet::new();
        let mut out: Vec<Node> = Vec::new();
        let mut queue: std::collections::VecDeque<Node> = nodes.into();
        while let Some(mut node) = queue.pop_front() {
            if out.len() >= MAX_NODES || !seen.insert((node.place.doc, node.place.pointer.clone())) {
                continue;
            }
            let Some(schema) = self.value_of(&node.place) else { continue };
            let draft = self.docs.get(node.place.doc).map_or_else(|| self.draft(), |d| d.draft);
            let hidden = draft.ref_hides_siblings() && schema.get("$ref").is_some();
            if !hidden && let Some(id) = text(schema, draft.id_keyword()).filter(|id| !id.starts_with('#')) {
                node.base = uri::without_fragment(&uri::join(&node.base, &id)).to_owned();
            }
            let at = |suffix: &str| Node {
                place: Place { doc: node.place.doc, pointer: format!("{}{suffix}", node.place.pointer) },
                base: node.base.clone(),
            };
            let mut next: Vec<Node> = Vec::new();
            if let Some(reference) = text(schema, "$ref").or_else(|| text(schema, "$dynamicRef")).or_else(|| text(schema, "$recursiveRef"))
                && let Some((place, base)) = self.resolve(&uri::join(&node.base, &reference))
            {
                next.push(Node { place, base });
            }
            if !hidden {
                for key in ["allOf", "anyOf", "oneOf"] {
                    if let Some(Data::Array(items)) = schema.get(key).map(|v| &v.data) {
                        next.extend((0..items.len()).take(MAX_NODES).map(|i| at(&format!("/{key}/{i}"))));
                    }
                }
                for key in ["then", "else"] {
                    if matches!(schema.get(key).map(|v| &v.data), Some(Data::Object(_))) {
                        next.push(at(&format!("/{key}")));
                    }
                }
            }
            out.push(node);
            queue.extend(next);
        }
        out
    }

    /// The schemas for the member `key` of an object that `nodes` describe, or for the item `index` of an array.
    fn step(&self, nodes: &[Node], segment: &Segment) -> Vec<Node> {
        let mut next: Vec<Node> = Vec::new();
        for node in nodes {
            let Some(schema) = self.value_of(&node.place) else { continue };
            let child = |suffix: String| Node {
                place: Place { doc: node.place.doc, pointer: format!("{}{suffix}", node.place.pointer) },
                base: node.base.clone(),
            };
            let is_schema = |v: Option<&Value>| v.is_some_and(|v| matches!(v.data, Data::Object(_)));
            match segment {
                Segment::Key(key) => {
                    let mut found = false;
                    if is_schema(schema.get("properties").and_then(|p| p.get(key))) {
                        next.push(child(format!("/properties/{}", uri::escape_token(key))));
                        found = true;
                    }
                    if let Some(Data::Object(patterns)) = schema.get("patternProperties").map(|v| &v.data) {
                        for m in patterns.iter().take(MAX_NODES) {
                            let matches = RegexBuilder::new(&m.key).size_limit(REGEX_SIZE_LIMIT).build().is_ok_and(|re| re.is_match(key));
                            if matches && matches!(m.value.data, Data::Object(_)) {
                                next.push(child(format!("/patternProperties/{}", uri::escape_token(&m.key))));
                                found = true;
                            }
                        }
                    }
                    let declared = schema.get("properties").and_then(|p| p.get(key)).is_some();
                    if !found && !declared {
                        for keyword in ["additionalProperties", "unevaluatedProperties"] {
                            if is_schema(schema.get(keyword)) {
                                next.push(child(format!("/{keyword}")));
                                break;
                            }
                        }
                    }
                }
                Segment::Index(index) => match (schema.get("prefixItems").map(|v| &v.data), schema.get("items").map(|v| &v.data)) {
                    (Some(Data::Array(prefix)), _) if *index < prefix.len() => next.push(child(format!("/prefixItems/{index}"))),
                    (_, Some(Data::Object(_))) => next.push(child("/items".to_owned())),
                    (_, Some(Data::Array(tuple))) if *index < tuple.len() => next.push(child(format!("/items/{index}"))),
                    (_, Some(Data::Array(_))) if is_schema(schema.get("additionalItems")) => next.push(child("/additionalItems".to_owned())),
                    _ => {
                        if is_schema(schema.get("unevaluatedItems")) {
                            next.push(child("/unevaluatedItems".to_owned()));
                        }
                    }
                },
            }
            if next.len() >= MAX_NODES {
                break;
            }
        }
        next.truncate(MAX_NODES);
        next
    }

    /// The schemas that apply at `path` (empty: the root).
    fn nodes_at(&self, path: &[Segment]) -> Vec<Node> {
        let mut nodes = self.expand(self.root_nodes());
        for segment in path {
            nodes = self.expand(self.step(&nodes, segment));
            if nodes.is_empty() {
                break;
            }
        }
        nodes
    }

    fn docs_of(&self, nodes: &[Node]) -> Docs {
        let mut docs = Docs::default();
        for node in nodes {
            let Some(schema) = self.value_of(&node.place) else { continue };
            docs.title = docs.title.take().or_else(|| text(schema, "title"));
            docs.description = docs.description.take().or_else(|| text(schema, "description"));
            match schema.get("type").map(|v| &v.data) {
                Some(Data::String(t)) => push_new(&mut docs.types, t.clone()),
                Some(Data::Array(list)) => {
                    for t in list {
                        if let Data::String(t) = &t.data {
                            push_new(&mut docs.types, t.clone());
                        }
                    }
                }
                _ => {}
            }
            if let Some(Data::Array(list)) = schema.get("enum").map(|v| &v.data) {
                for v in list.iter().take(MAX_VALUES) {
                    if let Some(l) = literal(v) {
                        push_new(&mut docs.values, l);
                    }
                }
            }
            if let Some(l) = schema.get("const").and_then(literal) {
                push_new(&mut docs.values, l);
            }
            if docs.default.is_none() {
                docs.default = schema.get("default").and_then(literal);
            }
            docs.deprecated |= matches!(schema.get("deprecated").map(|v| &v.data), Some(Data::Bool(true)));
        }
        docs
    }

    /// What the schemas say about the place `path`, or none when no schema reaches it.
    pub fn describe_at(&self, path: &[Segment]) -> Option<Docs> {
        let nodes = self.nodes_at(path);
        if nodes.is_empty() {
            return None;
        }
        let docs = self.docs_of(&nodes);
        (!docs.is_empty() || docs.deprecated).then_some(docs)
    }

    /// The names that may be written in the object at `path`, except the `present` ones: those the schemas that apply to it declare in
    /// `properties`, the required ones first.
    pub fn properties_at(&self, path: &[Segment], present: &[String]) -> Vec<PropertyHint> {
        let nodes = self.nodes_at(path);
        let mut names: Vec<String> = Vec::new();
        let mut required: HashSet<String> = HashSet::new();
        for node in &nodes {
            let Some(schema) = self.value_of(&node.place) else { continue };
            if let Some(Data::Object(members)) = schema.get("properties").map(|v| &v.data) {
                for m in members {
                    if names.len() < MAX_PROPERTIES {
                        push_new(&mut names, m.key.clone());
                    }
                }
            }
            if let Some(Data::Array(list)) = schema.get("required").map(|v| &v.data) {
                for r in list {
                    if let Data::String(r) = &r.data {
                        required.insert(r.clone());
                    }
                }
            }
        }
        // (A required name the schema does not describe is still a name to write.)
        for name in &required {
            if names.len() < MAX_PROPERTIES && !names.contains(name) {
                names.push(name.clone());
            }
        }
        let mut hints: Vec<PropertyHint> = names
            .into_iter()
            .filter(|name| !present.contains(name))
            .map(|name| {
                let child = self.expand(self.step(&nodes, &Segment::Key(name.clone())));
                let docs = self.docs_of(&child);
                PropertyHint {
                    required: required.contains(&name),
                    types: docs.types,
                    description: docs.description.or(docs.title),
                    default: docs.default,
                    deprecated: docs.deprecated,
                    name,
                }
            })
            .collect();
        // (A stable sort: the order of the schema is kept within the required and within the others.)
        hints.sort_by_key(|h| !h.required);
        hints
    }

    /// The values that may be written at `path`: those of `enum` and `const`, `true` and `false` for a boolean, `null`, the default
    /// and the examples.
    pub fn values_at(&self, path: &[Segment]) -> Vec<ValueHint> {
        let nodes = self.nodes_at(path);
        let docs = self.docs_of(&nodes);
        let mut out: Vec<ValueHint> = Vec::new();
        let mut add = |text: String, source: &'static str| {
            if out.len() < MAX_VALUES && !out.iter().any(|v| v.text == text) {
                out.push(ValueHint { text, source });
            }
        };
        for v in &docs.values {
            add(v.clone(), "enum");
        }
        if let Some(d) = &docs.default {
            add(d.clone(), "default");
        }
        for node in &nodes {
            if let Some(Data::Array(list)) = self.value_of(&node.place).and_then(|s| s.get("examples")).map(|v| &v.data) {
                for e in list.iter().take(MAX_VALUES) {
                    if let Some(l) = literal(e) {
                        add(l, "example");
                    }
                }
            }
        }
        if docs.values.is_empty() {
            if docs.types.iter().any(|t| t == "boolean") {
                add("true".to_owned(), "type");
                add("false".to_owned(), "type");
            }
            if docs.types.iter().any(|t| t == "null") {
                add("null".to_owned(), "type");
            }
        }
        out
    }
}

fn push_new(list: &mut Vec<String>, item: String) {
    if !list.contains(&item) {
        list.push(item);
    }
}
