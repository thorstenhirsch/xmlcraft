//! The taxonomy an instance names: the concepts its schema documents declare (name, type, substitution group, period type, abstract),
//! read from the `schemaRef`s and the schema documents they import and include, and the checks of an instance that need them: does a
//! fact's concept exist, is it a number, does its period type fit its context.
//!
//! Reading is bounded (documents, bytes, type chains) and never panics; what cannot be loaded is a warning, and what the instance uses
//! from a namespace that was not loaded is not judged.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;

use xmlcraft_syntax::{Severity, parse};
use xmlcraft_xsd::dom::{self, Dom, El};

use crate::{Finding, Instance, Kind, Level, Period, XBRLI};

const XS: &str = "http://www.w3.org/2001/XMLSchema";
/// The most schema documents read for one instance.
pub const MAX_DOCUMENTS: usize = 500;
/// The most bytes of schema documents read for one instance.
pub const MAX_BYTES: usize = 64 * 1024 * 1024;
/// How far a chain of types or substitution groups is followed.
const MAX_CHAIN: usize = 64;

/// Whether a concept is of an instant or of a duration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeriodType {
    Instant,
    Duration,
}

/// A concept: a top-level element declaration of a taxonomy schema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Concept {
    pub namespace: String,
    pub name: String,
    /// The type as `{namespace}local`, when it has one.
    pub type_name: Option<String>,
    /// The substitution group as `{namespace}local`.
    pub substitution_group: Option<String>,
    pub period_type: Option<PeriodType>,
    pub abstract_concept: bool,
}

/// Finds a schema document: its resolved location and its text, or why not.
pub trait Loader {
    fn load(&mut self, location: &str, base: &str) -> Result<(String, String), String>;
}

/// What was read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Taxonomy {
    /// By `{namespace}local`.
    pub concepts: HashMap<String, Concept>,
    /// Named types by `{namespace}local` and the type each is derived from.
    types: HashMap<String, String>,
    /// The target namespaces of the schema documents that were read.
    pub namespaces: HashSet<String>,
    /// The documents read, as they were found.
    pub documents: Vec<String>,
    /// The documents that could not be read and why: what was asked for and the reason.
    pub not_loaded: Vec<(String, String)>,
}

fn attr<'a>(el: &'a El, name: &str) -> Option<&'a str> {
    el.attrs.iter().find(|a| a.namespace.is_none() && a.local == name).map(|a| a.value.as_str())
}

fn attr_ns<'a>(el: &'a El, namespace: &str, name: &str) -> Option<&'a str> {
    el.attrs.iter().find(|a| a.namespace.as_deref() == Some(namespace) && a.local == name).map(|a| a.value.as_str())
}

/// `prefix:local` as `{namespace}local` with the declarations in scope at element `at`.
fn expand(tree: &Dom, at: usize, qname: &str) -> Option<String> {
    let (prefix, local) = match qname.trim().split_once(':') {
        Some((p, l)) => (Some(p), l),
        None => (None, qname.trim()),
    };
    let mut current = Some(at);
    let mut steps = 0;
    while let Some(i) = current {
        let el = tree.elements.get(i)?;
        if let Some((_, ns)) = el.declarations.iter().find(|(p, _)| p.as_deref() == prefix) {
            return Some(format!("{{{ns}}}{local}"));
        }
        steps += 1;
        if steps > 10_000 {
            return None;
        }
        current = el.parent;
    }
    // An unprefixed name with no default namespace is in none.
    prefix.is_none().then(|| format!("{{}}{local}"))
}

fn child<'a>(tree: &'a Dom, el: &El, local: &str) -> Option<(usize, &'a El)> {
    el.children.iter().filter_map(|i| Some((*i, tree.elements.get(*i)?))).find(|(_, c)| c.local == local && c.namespace.as_deref() == Some(XS))
}

impl Taxonomy {
    fn add_document(&mut self, location: &str, text: &str, queue: &mut VecDeque<(String, String)>) {
        let parsed = parse(text);
        if parsed.diagnostics().iter().any(|d| d.severity() == Severity::Error) {
            self.not_loaded.push((location.to_owned(), "it is not well-formed XML".to_owned()));
            return;
        }
        let tree = dom::build(&parsed.syntax());
        let Some(root) = tree.elements.first().filter(|r| r.local == "schema" && r.namespace.as_deref() == Some(XS)) else {
            self.not_loaded.push((location.to_owned(), "it is not an XML Schema document".to_owned()));
            return;
        };
        let target = attr(root, "targetNamespace").unwrap_or_default().to_owned();
        self.namespaces.insert(target.clone());
        self.documents.push(location.to_owned());
        for &i in &root.children {
            let Some(el) = tree.elements.get(i) else { continue };
            if el.namespace.as_deref() != Some(XS) {
                continue;
            }
            match el.local.as_str() {
                "import" | "include" => {
                    if let Some(l) = attr(el, "schemaLocation").filter(|l| !l.trim().is_empty()) {
                        queue.push_back((l.trim().to_owned(), location.to_owned()));
                    }
                }
                "element" => {
                    let Some(name) = attr(el, "name") else { continue };
                    let period_type = match attr_ns(el, XBRLI, "periodType") {
                        Some("instant") => Some(PeriodType::Instant),
                        Some("duration") => Some(PeriodType::Duration),
                        _ => None,
                    };
                    let concept = Concept {
                        namespace: target.clone(),
                        name: name.to_owned(),
                        type_name: attr(el, "type").and_then(|t| expand(&tree, i, t)),
                        substitution_group: attr(el, "substitutionGroup").and_then(|t| expand(&tree, i, t)),
                        period_type,
                        abstract_concept: attr(el, "abstract") == Some("true"),
                    };
                    self.concepts.insert(format!("{{{target}}}{name}"), concept);
                }
                "complexType" | "simpleType" => {
                    let Some(name) = attr(el, "name") else { continue };
                    // The type it is derived from: `simpleContent` / `restriction` / `extension`, or a simple type's `restriction`.
                    let derivation = match child(&tree, el, "simpleContent") {
                        Some((_, content)) => child(&tree, content, "restriction").or_else(|| child(&tree, content, "extension")),
                        None => child(&tree, el, "restriction"),
                    };
                    if let Some((at, d)) = derivation
                        && let Some(base) = attr(d, "base").and_then(|b| expand(&tree, at, b))
                    {
                        self.types.insert(format!("{{{target}}}{name}"), base);
                    }
                }
                _ => {}
            }
        }
    }

    /// Whether the type `{namespace}local` is a number: `Some(true)` for the numeric types of XBRL and XML Schema (and what is derived
    /// from them), `Some(false)` for the others that the chain reaches, none when the chain ends at a type that was not read.
    pub fn is_numeric_type(&self, type_name: &str) -> Option<bool> {
        let mut current = type_name.to_owned();
        for _ in 0..MAX_CHAIN {
            if let Some(local) = current.strip_prefix(&format!("{{{XBRLI}}}")) {
                return Some(numeric_item_type(local));
            }
            if let Some(local) = current.strip_prefix(&format!("{{{XS}}}")) {
                return Some(numeric_xs_type(local));
            }
            current = self.types.get(&current)?.clone();
        }
        None
    }

    /// Whether the concept is an item (not a tuple): its substitution group chain reaches `xbrli:item`; none when it does not end at one.
    pub fn is_item(&self, concept: &Concept) -> Option<bool> {
        let mut group = concept.substitution_group.clone();
        for _ in 0..MAX_CHAIN {
            let g = group?;
            if g == format!("{{{XBRLI}}}item") {
                return Some(true);
            }
            if g == format!("{{{XBRLI}}}tuple") {
                return Some(false);
            }
            group = self.concepts.get(&g)?.substitution_group.clone();
        }
        None
    }
}

fn numeric_item_type(local: &str) -> bool {
    matches!(
        local,
        "decimalItemType"
            | "floatItemType"
            | "doubleItemType"
            | "monetaryItemType"
            | "sharesItemType"
            | "pureItemType"
            | "integerItemType"
            | "nonPositiveIntegerItemType"
            | "negativeIntegerItemType"
            | "longItemType"
            | "intItemType"
            | "shortItemType"
            | "byteItemType"
            | "nonNegativeIntegerItemType"
            | "unsignedLongItemType"
            | "unsignedIntItemType"
            | "unsignedShortItemType"
            | "unsignedByteItemType"
            | "positiveIntegerItemType"
    )
}

fn numeric_xs_type(local: &str) -> bool {
    matches!(
        local,
        "decimal"
            | "float"
            | "double"
            | "integer"
            | "nonPositiveInteger"
            | "negativeInteger"
            | "long"
            | "int"
            | "short"
            | "byte"
            | "nonNegativeInteger"
            | "unsignedLong"
            | "unsignedInt"
            | "unsignedShort"
            | "unsignedByte"
            | "positiveInteger"
    )
}

/// Read the taxonomy the `schema_refs` of an instance (found at `base`) name, and the schema documents those import and include.
pub fn load(schema_refs: &[String], base: &str, loader: &mut dyn Loader) -> Taxonomy {
    let mut taxonomy = Taxonomy::default();
    let mut queue: VecDeque<(String, String)> = schema_refs.iter().map(|r| (r.clone(), base.to_owned())).collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut bytes = 0usize;
    while let Some((location, from)) = queue.pop_front() {
        if taxonomy.documents.len() >= MAX_DOCUMENTS || bytes >= MAX_BYTES {
            taxonomy.not_loaded.push((location, "the taxonomy is too large to read here".to_owned()));
            break;
        }
        match loader.load(&location, &from) {
            Ok((found, text)) => {
                if !seen.insert(found.clone()) {
                    continue;
                }
                bytes = bytes.saturating_add(text.len());
                taxonomy.add_document(&found, &text, &mut queue);
            }
            Err(why) => taxonomy.not_loaded.push((location, why)),
        }
    }
    taxonomy
}

fn namespace_of(concept_key: &str) -> &str {
    concept_key.strip_prefix('{').and_then(|r| r.split_once('}')).map_or("", |(ns, _)| ns)
}

/// What is wrong with the facts of `instance` that the `taxonomy` can say: a concept it does not declare, an abstract concept used for a
/// fact, a number with no unit (or a unit on what is not a number), an instant concept in a duration context (or the other way
/// round), a number with no `decimals` or `precision`; and what could not be read of the taxonomy.
pub fn check_taxonomy(instance: &Instance, taxonomy: &Taxonomy) -> Vec<Finding> {
    let mut found: Vec<Finding> = Vec::new();
    let mut add = |code: &'static str, level: Level, message: String, span: Range<usize>| found.push(Finding { code, level, message, span });
    for (location, why) in &taxonomy.not_loaded {
        add("xbrl.taxonomy-not-loaded", Level::Warning, format!("The taxonomy document {location} could not be read: {why}."), 0..0);
    }
    if instance.schema_refs.is_empty() && instance.kind == Kind::Instance {
        return found;
    }
    let contexts: HashMap<&str, &crate::Context> = instance.contexts.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut missing_namespaces: HashSet<String> = HashSet::new();
    for fact in &instance.facts {
        let ns = namespace_of(&fact.concept);
        if !taxonomy.namespaces.contains(ns) {
            // A namespace the taxonomy does not declare: not judged, but said once (the instance's own namespaces for XBRL itself are
            // not concepts).
            // (When no schema document could be read at all, the warning above says so: that is not repeated for every namespace.)
            if ns != XBRLI && !taxonomy.documents.is_empty() && missing_namespaces.insert(ns.to_owned()) {
                add(
                    "xbrl.taxonomy-namespace-missing",
                    Level::Warning,
                    format!("No schema document that was read declares the namespace {ns}: the concepts in it are not checked."),
                    fact.span.clone(),
                );
            }
            continue;
        }
        let Some(concept) = taxonomy.concepts.get(&fact.concept) else {
            add("xbrl.unknown-concept", Level::Error, format!("The taxonomy does not declare the concept {}.", fact.name), fact.span.clone());
            continue;
        };
        if concept.abstract_concept {
            add("xbrl.abstract-fact", Level::Error, format!("The concept {} is abstract: it cannot have a fact.", fact.name), fact.span.clone());
        }
        if taxonomy.is_item(concept) == Some(false) {
            continue;
        }
        let numeric = concept.type_name.as_deref().and_then(|t| taxonomy.is_numeric_type(t));
        if let Some(numeric) = numeric
            && !fact.nil
        {
            match (numeric, fact.unit_ref.is_some()) {
                (true, false) => {
                    add("xbrl.numeric-needs-unit", Level::Error, format!("The fact {} is a number and has no unitRef.", fact.name), fact.span.clone())
                }
                (false, true) => add(
                    "xbrl.unit-on-non-numeric",
                    Level::Error,
                    format!("The fact {} is not a number and has a unitRef.", fact.name),
                    fact.span.clone(),
                ),
                _ => {}
            }
            if numeric && fact.decimals.is_none() && fact.precision.is_none() {
                add(
                    "xbrl.numeric-needs-precision",
                    Level::Error,
                    format!("The fact {} is a number and has neither decimals nor precision.", fact.name),
                    fact.span.clone(),
                );
            }
        }
        if let (Some(period_type), Some(context)) = (concept.period_type, fact.context_ref.as_deref().and_then(|c| contexts.get(c))) {
            let fits = matches!(
                (&context.period, period_type),
                (Some(Period::Instant(_)), PeriodType::Instant) | (Some(Period::Duration(..) | Period::Forever), PeriodType::Duration) | (None, _)
            );
            if !fits {
                let wanted = if period_type == PeriodType::Instant { "an instant" } else { "a duration" };
                add(
                    "xbrl.period-type",
                    Level::Error,
                    format!(
                        "The concept {} is for {wanted}, and the context {} has the period {}.",
                        fact.name,
                        context.id,
                        context.period.as_ref().map_or_else(String::new, Period::label)
                    ),
                    fact.span.clone(),
                );
            }
        }
    }
    found
}
