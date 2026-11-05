//! A set of schema documents: the one to validate with and the ones it refers to, indexed by `$id`, `$anchor` and `$dynamicAnchor`.

use std::collections::{HashMap, HashSet, VecDeque};

use xmlcraft_json_syntax::{Data, Dialect, Options, Value, parse};

use crate::draft::Draft;
use crate::uri;

/// At most this many schema documents are held (the first one and what it refers to).
pub const MAX_DOCUMENTS: usize = 64;
/// Schemas nested deeper than this are not indexed any further.
const MAX_INDEX_DEPTH: usize = 128;

/// Gives the text of a schema document that is referred to by URI. Nothing is fetched here: what to do for a URI (a file, the cache,
/// the web, a bundled meta-schema) is for the implementor, and `None` means it is not available.
pub trait Loader {
    fn load(&self, uri: &str) -> Option<String>;
}

/// A loader that has nothing.
pub struct NoLoader;

impl Loader for NoLoader {
    fn load(&self, _: &str) -> Option<String> {
        None
    }
}

impl<F: Fn(&str) -> Option<String>> Loader for F {
    fn load(&self, uri: &str) -> Option<String> {
        self(uri)
    }
}

/// A mistake in a schema, found while indexing it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaProblem {
    /// Where, as a JSON Pointer into the first document (the one validated with), or the URI of another document.
    pub document: String,
    pub pointer: String,
    pub message: String,
}

pub(crate) struct Doc {
    pub(crate) value: Value,
    pub(crate) draft: Draft,
    /// The URI it is known by (its `$id`, or the one it was loaded from; may be empty).
    pub(crate) base: String,
}

/// Where a URI points: a document and a JSON Pointer in it.
#[derive(Clone, Debug)]
pub(crate) struct Place {
    pub(crate) doc: usize,
    pub(crate) pointer: String,
}

pub struct SchemaSet {
    pub(crate) docs: Vec<Doc>,
    /// Schema resources by absolute URI without fragment.
    pub(crate) resources: HashMap<String, Place>,
    /// Named places: `uri#name` for `$anchor`, `$dynamicAnchor` and the plain-name `$id` of drafts up to 7.
    pub(crate) anchors: HashMap<String, Place>,
    /// `$dynamicAnchor`s, by `uri#name`.
    pub(crate) dynamic_anchors: HashSet<String>,
    pub problems: Vec<SchemaProblem>,
}

impl SchemaSet {
    /// The set for the schema `root` (known as `base`, which may be empty), reading what it refers to through `loader`. The draft is
    /// that of `$schema` when it names one, else `default_draft`.
    pub fn new(root: Value, base: &str, default_draft: Draft, loader: &dyn Loader) -> SchemaSet {
        let mut set =
            SchemaSet { docs: Vec::new(), resources: HashMap::new(), anchors: HashMap::new(), dynamic_anchors: HashSet::new(), problems: Vec::new() };
        let mut queue: VecDeque<(String, Draft)> = VecDeque::new();
        let mut tried: HashSet<String> = HashSet::new();
        set.add(root, base, default_draft, &mut queue);
        while let Some((uri, draft)) = queue.pop_front() {
            if !tried.insert(uri.clone()) || set.resources.contains_key(&uri) || set.docs.len() >= MAX_DOCUMENTS {
                continue;
            }
            let Some(text) = loader.load(&uri) else { continue };
            // (Schemas are often written with comments and trailing commas, as editors allow; the document being validated is
            // read by the caller, the same way for the schema it names.)
            let read = parse(&text, &Options { dialect: Dialect::Json5, ..Options::default() });
            let Some(value) = read.root.filter(|_| read.diagnostics.iter().all(|d| d.severity() != xmlcraft_json_syntax::Severity::Error)) else {
                set.problems.push(SchemaProblem {
                    document: uri.clone(),
                    pointer: String::new(),
                    message: format!("The schema '{uri}' is not valid JSON."),
                });
                continue;
            };
            set.add(value, &uri, draft, &mut queue);
        }
        set
    }

    fn add(&mut self, value: Value, base: &str, default_draft: Draft, queue: &mut VecDeque<(String, Draft)>) {
        let draft =
            value.get("$schema").and_then(|s| if let Data::String(uri) = &s.data { Draft::from_uri(uri) } else { None }).unwrap_or(default_draft);
        let doc = self.docs.len();
        // A schema that says who it is (`$id`) is known by that as well as by where it was loaded from.
        let own = value.get(draft.id_keyword()).and_then(|v| if let Data::String(s) = &v.data { Some(s.clone()) } else { None });
        let doc_base = match own.as_deref() {
            Some(id) if !id.starts_with('#') => uri::without_fragment(&uri::join(base, id)).to_owned(),
            _ => uri::without_fragment(base).to_owned(),
        };
        let mut found = Vec::new();
        let mut problems = Vec::new();
        self.index(doc, draft, &value, "", &doc_base, 0, &mut found, &mut problems);
        // (The first document is the one with no name too: `#/definitions/x` in it refers to itself.)
        if !doc_base.is_empty() || doc == 0 {
            self.resources.entry(doc_base.clone()).or_insert(Place { doc, pointer: String::new() });
        }
        let loaded_from = uri::without_fragment(base);
        if !loaded_from.is_empty() && loaded_from != doc_base {
            self.resources.entry(loaded_from.to_owned()).or_insert(Place { doc, pointer: String::new() });
        }
        for uri in found {
            queue.push_back((uri, draft));
        }
        self.problems.extend(problems);
        self.docs.push(Doc { value, draft, base: doc_base });
    }

    /// Record the resources and anchors in `schema` and the documents it refers to.
    #[allow(clippy::too_many_arguments)]
    fn index(
        &mut self,
        doc: usize,
        draft: Draft,
        schema: &Value,
        pointer: &str,
        base: &str,
        depth: usize,
        refs: &mut Vec<String>,
        problems: &mut Vec<SchemaProblem>,
    ) {
        let Data::Object(_) = &schema.data else { return };
        if depth > MAX_INDEX_DEPTH {
            return;
        }
        let text = |key: &str| schema.get(key).and_then(|v| if let Data::String(s) = &v.data { Some(s.as_str()) } else { None });
        let mut base = base.to_owned();
        let hidden = draft.ref_hides_siblings() && schema.get("$ref").is_some();
        if let Some(id) = text(draft.id_keyword()).filter(|_| !hidden) {
            if let Some(name) = id.strip_prefix('#') {
                if draft <= Draft::D07 && !name.is_empty() {
                    self.anchors.entry(format!("{base}#{name}")).or_insert(Place { doc, pointer: pointer.to_owned() });
                }
            } else {
                base = uri::without_fragment(&uri::join(&base, id)).to_owned();
                self.resources.entry(base.clone()).or_insert(Place { doc, pointer: pointer.to_owned() });
            }
        }
        if draft >= Draft::D2019_09 {
            if let Some(name) = text("$anchor") {
                self.anchors.entry(format!("{base}#{name}")).or_insert(Place { doc, pointer: pointer.to_owned() });
            }
            if draft >= Draft::D2020_12
                && let Some(name) = text("$dynamicAnchor")
            {
                let key = format!("{base}#{name}");
                self.anchors.entry(key.clone()).or_insert(Place { doc, pointer: pointer.to_owned() });
                self.dynamic_anchors.insert(key);
            }
        }
        if let Some(reference) = text("$ref") {
            let target = uri::join(&base, reference);
            let target = uri::without_fragment(&target);
            if !target.is_empty() {
                refs.push(target.to_owned());
            }
        }
        if draft >= Draft::D2019_09
            && let Some(reference) = text("$recursiveRef").or_else(|| text("$dynamicRef"))
        {
            let target = uri::join(&base, reference);
            let target = uri::without_fragment(&target);
            if !target.is_empty() {
                refs.push(target.to_owned());
            }
        }
        let child = |this: &mut Self, value: &Value, suffix: String, refs: &mut Vec<String>, problems: &mut Vec<SchemaProblem>| {
            let at = format!("{pointer}{suffix}");
            this.index(doc, draft, value, &at, &base, depth + 1, refs, problems);
        };
        let Data::Object(members) = &schema.data else { return };
        // (The first member of a name counts: that is the one a lookup finds.)
        let mut seen: HashSet<&str> = HashSet::new();
        for member in members {
            if !seen.insert(member.key.as_str()) {
                continue;
            }
            let key = member.key.as_str();
            let enc = uri::escape_token(key);
            match (key, &member.value.data) {
                (
                    "additionalProperties"
                    | "additionalItems"
                    | "contains"
                    | "propertyNames"
                    | "not"
                    | "if"
                    | "then"
                    | "else"
                    | "unevaluatedItems"
                    | "unevaluatedProperties"
                    | "contentSchema",
                    Data::Object(_),
                ) => child(self, &member.value, format!("/{enc}"), refs, problems),
                ("items", Data::Object(_)) => child(self, &member.value, "/items".to_owned(), refs, problems),
                ("allOf" | "anyOf" | "oneOf" | "prefixItems" | "items", Data::Array(items)) => {
                    for (i, item) in items.iter().enumerate() {
                        child(self, item, format!("/{enc}/{i}"), refs, problems);
                    }
                }
                ("properties" | "patternProperties" | "dependentSchemas" | "$defs" | "definitions" | "dependencies", Data::Object(inner)) => {
                    let mut seen_inner: HashSet<&str> = HashSet::new();
                    for m in inner {
                        if seen_inner.insert(m.key.as_str()) {
                            child(self, &m.value, format!("/{enc}/{}", uri::escape_token(&m.key)), refs, problems);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// The draft of the first document.
    pub fn draft(&self) -> Draft {
        self.docs.first().map_or(Draft::D2020_12, |d| d.draft)
    }

    /// The first document: the schema that is validated with.
    pub fn root(&self) -> Option<&Value> {
        self.docs.first().map(|d| &d.value)
    }
}
