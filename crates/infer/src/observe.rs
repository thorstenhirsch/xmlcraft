//! What is learned from the documents: for each element its attributes and values, its text, and the order and number of its children.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use xmlcraft_syntax::{Severity, parse};
use xmlcraft_xsd::dom::{self, Dom};

use crate::values::{Types, ValueSet};
use crate::{InferError, Output};

/// The XML namespace and the schema-instance namespace: attributes in them are not declared in a schema.
pub const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
pub const XSI_NS: &str = "http://www.w3.org/2001/XMLSchema-instance";

/// The most documents text one run reads, in bytes (each document is also capped by the editor's own analysis limit).
pub const MAX_BYTES: usize = 256 * 1024 * 1024;
/// An element with more distinct child names than this is given a free order of its children.
pub const MAX_ORDERED_SYMBOLS: usize = 64;
/// The most distinct counts of children (how often each child name occurs in one element) kept for an element.
pub const MAX_SIGNATURES: usize = 10_000;

/// A name: for XSD the namespace and the local name; for a DTD the name as written, with no namespace.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name {
    pub namespace: Option<String>,
    pub local: String,
}

impl Name {
    pub fn new(namespace: Option<&str>, local: &str) -> Self {
        Self { namespace: namespace.map(str::to_owned), local: local.to_owned() }
    }
}

#[derive(Debug, Default)]
pub struct AttributeStats {
    /// In how many elements it was given.
    pub count: usize,
    pub values: ValueSet,
}

/// The order and number of the children of one kind of element.
#[derive(Debug, Default)]
pub struct ChildStats {
    /// The child names, in the order first seen.
    pub symbols: Vec<Name>,
    index: HashMap<Name, usize>,
    /// `(a, b)` with `a != b`: an `a` comes before a `b` in some element. (That a name repeats is in the signatures.)
    pub before: BTreeSet<(usize, usize)>,
    /// How often each name occurs in one element, as a sorted list of (name, count), and how many elements had exactly these counts.
    pub signatures: BTreeMap<Vec<(usize, usize)>, usize>,
    /// There were more signatures than are kept.
    pub signatures_overflow: bool,
    /// Some element had more distinct children than [`MAX_ORDERED_SYMBOLS`], so the order is not known.
    pub order_unknown: bool,
}

impl ChildStats {
    fn symbol(&mut self, name: &Name) -> usize {
        if let Some(&i) = self.index.get(name) {
            return i;
        }
        let i = self.symbols.len();
        self.symbols.push(name.clone());
        self.index.insert(name.clone(), i);
        i
    }

    /// Learn one element's children, in order.
    pub(crate) fn observe_names(&mut self, names: impl Iterator<Item = Name>) {
        let indices: Vec<usize> = names.map(|n| self.symbol(&n)).collect();
        self.observe(&indices);
    }

    fn observe(&mut self, children: &[usize]) {
        // First position, last position and count of each distinct child name.
        let mut seen: Vec<(usize, usize, usize, usize)> = Vec::new();
        for (position, &child) in children.iter().enumerate() {
            match seen.iter_mut().find(|s| s.0 == child) {
                Some(entry) => {
                    entry.2 = position;
                    entry.3 = entry.3.saturating_add(1);
                }
                None => seen.push((child, position, position, 1)),
            }
            if seen.len() > MAX_ORDERED_SYMBOLS * 4 {
                self.order_unknown = true;
                break;
            }
        }
        if seen.len() > MAX_ORDERED_SYMBOLS {
            self.order_unknown = true;
        }
        if !self.order_unknown {
            for a in &seen {
                for b in &seen {
                    // An `a` comes before a `b` when the first `a` is before the last `b`.
                    if a.0 != b.0 && a.1 < b.2 {
                        self.before.insert((a.0, b.0));
                    }
                }
            }
        }
        let mut signature: Vec<(usize, usize)> = seen.iter().map(|s| (s.0, s.3)).collect();
        signature.sort_unstable();
        if let Some(count) = self.signatures.get_mut(&signature) {
            *count = count.saturating_add(1);
        } else if self.signatures.len() < MAX_SIGNATURES {
            self.signatures.insert(signature, 1);
        } else {
            self.signatures_overflow = true;
        }
    }
}

/// Everything learned about one kind of element.
#[derive(Debug, Default)]
pub struct ElementStats {
    pub instances: usize,
    pub attributes: BTreeMap<Name, AttributeStats>,
    pub children: ChildStats,
    /// Elements with at least one child element.
    pub with_children: usize,
    /// Elements with child elements and text that is not white space.
    pub mixed: usize,
    /// The text of the elements that have no child elements (an empty element has the empty text).
    pub text: ValueSet,
}

impl ElementStats {
    /// Some element without child elements had text.
    pub fn has_text(&self) -> bool {
        self.text.has_non_empty()
    }
}

/// What has been read from the documents so far.
#[derive(Debug)]
pub struct Observed {
    pub output: Output,
    pub elements: BTreeMap<Name, ElementStats>,
    /// The names of the root elements, in the order they were first seen.
    pub roots: Vec<Name>,
    /// The first prefix a namespace was written with.
    pub prefixes: BTreeMap<String, String>,
    pub documents: usize,
    bytes: usize,
    types: Types,
}

impl Observed {
    pub fn new(output: Output) -> Self {
        Self { output, elements: BTreeMap::new(), roots: Vec::new(), prefixes: BTreeMap::new(), documents: 0, bytes: 0, types: Types::new() }
    }

    /// Read one document.
    pub fn observe_document(&mut self, text: &str) -> Result<(), InferError> {
        self.bytes = self.bytes.saturating_add(text.len());
        if self.bytes > MAX_BYTES {
            return Err(InferError::TooLarge);
        }
        let parsed = parse(text);
        if let Some(problem) = parsed.diagnostics().iter().find(|d| d.severity() == Severity::Error) {
            return Err(InferError::NotWellFormed { document: self.documents, message: problem.message(), offset: problem.span.start });
        }
        let dom = dom::build(&parsed.syntax());
        if let Some(why) = dom.truncated {
            return Err(InferError::Limit(why));
        }
        if let Some((prefix, at)) = dom.unbound.first() {
            return Err(InferError::NotWellFormed {
                document: self.documents,
                message: format!("The namespace prefix '{prefix}' is not declared."),
                offset: at.start,
            });
        }
        let Some(root) = dom.root() else { return Err(InferError::NoRoot) };
        self.documents = self.documents.saturating_add(1);
        for index in 0..dom.elements.len() {
            self.observe_element(&dom, index, index == root);
        }
        Ok(())
    }

    fn name_of(&self, namespace: Option<&str>, local: &str, qname: &str) -> Name {
        match self.output {
            Output::Xsd { .. } => Name::new(namespace, local),
            Output::Dtd => Name::new(None, qname),
        }
    }

    fn observe_element(&mut self, dom: &Dom, index: usize, is_root: bool) {
        let Some(el) = dom.elements.get(index) else { return };
        let name = self.name_of(el.namespace.as_deref(), &el.local, &el.qname);
        if is_root && !self.roots.contains(&name) {
            self.roots.push(name.clone());
        }
        if let Some(ns) = &el.namespace {
            self.prefixes.entry(ns.clone()).or_insert_with(|| el.qname.split_once(':').map(|(p, _)| p.to_owned()).unwrap_or_default());
        }
        for (prefix, ns) in &el.declarations {
            if let Some(p) = prefix {
                self.prefixes.entry(ns.clone()).or_insert_with(|| p.clone());
            }
        }
        // The names of the children, and the text, before the element is borrowed from the map.
        let child_names: Vec<Name> =
            el.children.iter().filter_map(|&c| dom.elements.get(c)).map(|c| self.name_of(c.namespace.as_deref(), &c.local, &c.qname)).collect();
        let xsd = matches!(self.output, Output::Xsd { .. });
        let mut attributes: Vec<(Name, &str)> = Vec::new();
        for attribute in &el.attrs {
            let in_schema_instance = attribute.namespace.as_deref() == Some(XSI_NS);
            if xsd && in_schema_instance {
                continue; // `xsi:type`, `xsi:nil`, `xsi:schemaLocation`: the validator reads them, a schema does not declare them
            }
            attributes.push((self.name_of(attribute.namespace.as_deref(), &attribute.local, &attribute.qname), &attribute.value));
        }
        if !xsd {
            // The namespace declarations are attributes in a DTD, which must declare every attribute a document has.
            for (prefix, value) in &el.declarations {
                let written = match prefix {
                    Some(p) => format!("xmlns:{p}"),
                    None => "xmlns".to_owned(),
                };
                attributes.push((Name::new(None, &written), value));
            }
        }
        let Observed { elements, types, .. } = self;
        let stats = elements.entry(name).or_default();
        stats.instances = stats.instances.saturating_add(1);
        for (attribute, value) in attributes {
            let entry = stats.attributes.entry(attribute).or_default();
            entry.count = entry.count.saturating_add(1);
            entry.values.observe(value, types);
        }
        stats.children.observe_names(child_names.into_iter());
        if el.children.is_empty() {
            stats.text.observe(&el.text, types);
        } else {
            stats.with_children = stats.with_children.saturating_add(1);
            if el.text.chars().any(|c| !xmlcraft_syntax::is_xml_space(c)) {
                stats.mixed = stats.mixed.saturating_add(1);
            }
        }
    }
}
