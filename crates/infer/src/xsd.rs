//! The XML Schema writer: one schema document per namespace, in one of three layouts.
//!
//! - **Russian doll**: one global element for each root, the others declared inside their parents with anonymous types. An element that
//!   is a root, belongs to a cycle (`section` in `section`), is the child of more than one parent, or is in another namespace than its
//!   parent is global and referred to, so the output stays finite and does not grow with the number of parents.
//! - **Salami slice**: every element is a global declaration and every use of it is a reference.
//! - **Venetian blind**: a named global complex type for each element with attributes or children; elements are local, of those types;
//!   only the roots (and elements of another namespace) are global.

use std::collections::{BTreeMap, BTreeSet};

use crate::content::{Factor, factors};
use crate::observe::{ElementStats, Name, Observed, XML_NS};
use crate::values::{Kind, ValueSet};
use crate::{GeneratedFile, Layout, Options};

/// The attributes of the XML namespace a schema may refer to (the others are not declared in `xml.xsd`).
const XML_ATTRIBUTES: [&str; 4] = ["lang", "space", "base", "id"];
const XML_SCHEMA_LOCATION: &str = "http://www.w3.org/2001/xml.xsd";

#[derive(Clone, Debug)]
struct SimpleSpec {
    kind: Kind,
    enumeration: Option<Vec<String>>,
}

#[derive(Debug)]
struct AttrDecl {
    name: Name,
    required: bool,
    spec: SimpleSpec,
}

#[derive(Debug)]
enum Body {
    Empty,
    Simple(SimpleSpec),
    Complex { factors: Vec<Factor>, mixed: bool },
}

#[derive(Debug)]
struct Shape {
    attrs: Vec<AttrDecl>,
    body: Body,
    /// The child names the factors' indices are into.
    symbols: Vec<Name>,
}

fn spec_of(values: &ValueSet, options: &Options) -> SimpleSpec {
    let kind = if options.detect_types { values.kind() } else { Kind::String };
    let enumeration = options.enumerations.and_then(|rule| values.enumeration(rule));
    SimpleSpec { kind, enumeration }
}

fn shape_of(stats: &ElementStats, options: &Options) -> Shape {
    let attrs = stats
        .attributes
        .iter()
        .map(|(name, a)| AttrDecl {
            name: name.clone(),
            required: a.count >= stats.instances && stats.instances > 0,
            spec: spec_of(&a.values, options),
        })
        .collect();
    let body = if stats.with_children == 0 {
        if stats.has_text() { Body::Simple(spec_of(&stats.text, options)) } else { Body::Empty }
    } else {
        // Text next to the child elements, in some element or in elements that have no children, makes the content mixed.
        let mixed = stats.mixed > 0 || stats.has_text();
        Body::Complex { factors: factors(&stats.children), mixed }
    };
    Shape { attrs, body, symbols: stats.children.symbols.clone() }
}

/// An XML writer: one element to a line.
struct Xml {
    text: String,
    depth: usize,
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            c => out.push(c),
        }
    }
    out
}

impl Xml {
    fn new() -> Self {
        Self { text: String::new(), depth: 0 }
    }

    fn line(&mut self, tag: &str, attrs: &[(&str, String)], kind: Tag) {
        for _ in 0..self.depth {
            self.text.push_str("  ");
        }
        if kind == Tag::Close {
            self.text.push_str("</");
            self.text.push_str(tag);
        } else {
            self.text.push('<');
            self.text.push_str(tag);
            for (name, value) in attrs {
                self.text.push(' ');
                self.text.push_str(name);
                self.text.push_str("=\"");
                self.text.push_str(&escape(value));
                self.text.push('"');
            }
            if kind == Tag::Empty {
                self.text.push('/');
            }
        }
        self.text.push_str(">\n");
    }

    fn open(&mut self, tag: &str, attrs: &[(&str, String)]) {
        self.line(tag, attrs, Tag::Open);
        self.depth += 1;
    }

    fn close(&mut self, tag: &str) {
        self.depth = self.depth.saturating_sub(1);
        self.line(tag, &[], Tag::Close);
    }

    fn leaf(&mut self, tag: &str, attrs: &[(&str, String)]) {
        self.line(tag, attrs, Tag::Empty);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tag {
    Open,
    Close,
    Empty,
}

/// What a schema document refers to in other documents.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Import {
    /// A namespace written here (`None`: no namespace).
    Schema(Option<String>),
    /// The XML namespace, for `xml:lang` and the others.
    Xml,
}

struct Plan<'a> {
    options: &'a Options,
    layout: Layout,
    shapes: BTreeMap<Name, Shape>,
    global: BTreeSet<Name>,
    /// Namespaced attributes (other than the XML namespace's) that need a global declaration.
    global_attributes: BTreeSet<Name>,
    prefixes: BTreeMap<String, String>,
    files: BTreeMap<Option<String>, String>,
}

/// One schema document being written.
struct File {
    namespace: Option<String>,
    xml: Xml,
    imports: BTreeSet<Import>,
}

pub fn write(obs: &Observed, options: &Options, layout: Layout) -> Vec<GeneratedFile> {
    let shapes: BTreeMap<Name, Shape> = obs.elements.iter().map(|(n, s)| (n.clone(), shape_of(s, options))).collect();
    let mut namespaces: BTreeSet<Option<String>> = shapes.keys().map(|n| n.namespace.clone()).collect();
    let mut global_attributes = BTreeSet::new();
    for shape in shapes.values() {
        for a in &shape.attrs {
            if let Some(ns) = &a.name.namespace
                && ns != XML_NS
            {
                global_attributes.insert(a.name.clone());
                namespaces.insert(Some(ns.clone()));
            }
        }
    }
    let namespaces: Vec<Option<String>> = namespaces.into_iter().collect();
    let plan = Plan {
        options,
        layout,
        global: global_set(obs, &shapes, layout),
        shapes,
        global_attributes,
        prefixes: choose_prefixes(obs, &namespaces),
        files: choose_file_names(obs, &namespaces),
    };
    namespaces.iter().map(|ns| plan.write_file(ns)).collect()
}

/// The elements that are declared globally.
fn global_set(obs: &Observed, shapes: &BTreeMap<Name, Shape>, layout: Layout) -> BTreeSet<Name> {
    if layout == Layout::SalamiSlice {
        return shapes.keys().cloned().collect();
    }
    let mut global: BTreeSet<Name> = obs.roots.iter().cloned().collect();
    // Children in another namespace than their parent are referred to.
    let mut parents: BTreeMap<&Name, BTreeSet<&Name>> = BTreeMap::new();
    for (parent, shape) in shapes {
        for child in &shape.symbols {
            if child.namespace != parent.namespace {
                global.insert(child.clone());
            }
            parents.entry(child).or_default().insert(parent);
        }
    }
    if layout == Layout::RussianDoll {
        for (name, shape) in shapes {
            let has_content = !shape.symbols.is_empty();
            // Declared inside more than one parent, the content would be written again for each. This is also what keeps an element out
            // of itself: a cycle is entered from outside (the element has a second parent) or at a root, and both are global.
            if has_content && parents.get(name).is_some_and(|p| p.len() > 1) {
                global.insert(name.clone());
            }
        }
    }
    global
}

/// A prefix for each namespace: the one the documents used first, or `ns1`, `ns2`, … when that is taken or reserved.
fn choose_prefixes(obs: &Observed, namespaces: &[Option<String>]) -> BTreeMap<String, String> {
    let mut used: BTreeSet<String> = ["xs", "xsi", "xml", "xmlns"].iter().map(|s| (*s).to_owned()).collect();
    let mut out = BTreeMap::new();
    let mut fallback = 0usize;
    for ns in namespaces.iter().flatten() {
        let wanted = obs.prefixes.get(ns).filter(|p| !p.is_empty() && !used.contains(*p) && xmlcraft_datatypes::is_ncname(p)).cloned();
        let prefix = wanted.unwrap_or_else(|| {
            loop {
                fallback += 1;
                let candidate = format!("ns{fallback}");
                if !used.contains(&candidate) {
                    break candidate;
                }
            }
        });
        used.insert(prefix.clone());
        out.insert(ns.clone(), prefix);
    }
    out
}

/// A file name for the schema of each namespace.
fn choose_file_names(obs: &Observed, namespaces: &[Option<String>]) -> BTreeMap<Option<String>, String> {
    if namespaces.len() <= 1 {
        return namespaces.iter().map(|ns| (ns.clone(), "schema.xsd".to_owned())).collect();
    }
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut out = BTreeMap::new();
    for ns in namespaces {
        let stem = match ns {
            None => "no-namespace".to_owned(),
            Some(uri) => {
                let slug: String = obs
                    .prefixes
                    .get(uri)
                    .filter(|p| !p.is_empty())
                    .cloned()
                    .unwrap_or_else(|| uri.rsplit(['/', '#', ':']).find(|s| !s.is_empty()).unwrap_or("namespace").to_owned());
                let clean: String =
                    slug.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '-' }).collect();
                if clean.is_empty() { "namespace".to_owned() } else { clean }
            }
        };
        let mut name = format!("{stem}.xsd");
        let mut n = 1usize;
        while !used.insert(name.clone()) {
            n += 1;
            name = format!("{stem}-{n}.xsd");
        }
        out.insert(ns.clone(), name);
    }
    out
}

impl Plan<'_> {
    fn prefix(&self, ns: &Option<String>) -> Option<&str> {
        ns.as_ref().and_then(|n| self.prefixes.get(n)).map(String::as_str)
    }

    /// `prefix:local`, or just `local` in no namespace.
    fn qualified(&self, ns: &Option<String>, local: &str) -> String {
        match self.prefix(ns) {
            Some(p) => format!("{p}:{local}"),
            None => local.to_owned(),
        }
    }

    fn occurs(&self, min: usize, max: usize) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        // Never a least number above one: that a document happens to have two is not a rule. (Zero is: it was seen.)
        if min == 0 {
            out.push(("minOccurs", "0".to_owned()));
        }
        if max == usize::MAX || (self.options.unbounded && max > 1) {
            out.push(("maxOccurs", "unbounded".to_owned()));
        } else if max != 1 {
            out.push(("maxOccurs", max.to_string()));
        }
        out
    }

    fn type_name(local: &str) -> String {
        format!("{local}Type")
    }

    fn write_file(&self, ns: &Option<String>) -> GeneratedFile {
        let mut file = File { namespace: ns.clone(), xml: Xml::new(), imports: BTreeSet::new() };
        // The body first: what it refers to decides the imports.
        file.xml.depth = 1;
        // Global attributes.
        for name in self.global_attributes.iter().filter(|n| &n.namespace == ns) {
            file.xml.leaf("xs:attribute", &[("name", name.local.clone()), ("type", "xs:string".to_owned())]);
        }
        if self.layout == Layout::VenetianBlind {
            for (name, shape) in self.shapes.iter().filter(|(n, _)| &n.namespace == ns) {
                if needs_complex_type(shape) {
                    self.write_named_type(&mut file, name, shape);
                }
            }
        }
        for (name, shape) in self.shapes.iter().filter(|(n, _)| &n.namespace == ns) {
            if self.global.contains(name) {
                self.write_element(&mut file, name, shape, &[]);
            }
        }
        let body = std::mem::take(&mut file.xml.text);
        let mut head = Xml::new();
        let mut root_attrs: Vec<(&str, String)> = Vec::new();
        let mut declarations: Vec<(String, String)> = vec![("xmlns:xs".to_owned(), "http://www.w3.org/2001/XMLSchema".to_owned())];
        if let Some(uri) = ns {
            root_attrs.push(("targetNamespace", uri.clone()));
            if let Some(p) = self.prefix(ns) {
                declarations.push((format!("xmlns:{p}"), uri.clone()));
            }
            root_attrs.push(("elementFormDefault", "qualified".to_owned()));
        }
        // The namespaces the document refers to need a prefix declared for their names.
        for import in &file.imports {
            match import {
                Import::Schema(Some(other)) => {
                    if let Some(p) = self.prefixes.get(other) {
                        declarations.push((format!("xmlns:{p}"), other.clone()));
                    }
                }
                Import::Xml => declarations.push(("xmlns:xml".to_owned(), XML_NS.to_owned())),
                Import::Schema(None) => {}
            }
        }
        declarations.sort();
        declarations.dedup();
        head.text.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        let mut all: Vec<(&str, String)> = declarations.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        all.extend(root_attrs);
        head.open("xs:schema", &all);
        for import in &file.imports {
            match import {
                Import::Schema(Some(other)) => {
                    head.leaf(
                        "xs:import",
                        &[("namespace", other.clone()), ("schemaLocation", self.files.get(&Some(other.clone())).cloned().unwrap_or_default())],
                    );
                }
                Import::Schema(None) => {
                    head.leaf("xs:import", &[("schemaLocation", self.files.get(&None).cloned().unwrap_or_default())]);
                }
                Import::Xml => {
                    head.leaf("xs:import", &[("namespace", XML_NS.to_owned()), ("schemaLocation", XML_SCHEMA_LOCATION.to_owned())]);
                }
            }
        }
        let mut text = head.text;
        text.push_str(&body);
        text.push_str("</xs:schema>\n");
        GeneratedFile { name: self.files.get(ns).cloned().unwrap_or_else(|| "schema.xsd".to_owned()), namespace: ns.clone(), text }
    }

    /// An element declaration. `occurs` are the `minOccurs` and `maxOccurs` attributes of a local one (none for a top-level one).
    fn write_element(&self, file: &mut File, name: &Name, shape: &Shape, occurs: &[(&'static str, String)]) {
        let mut attrs: Vec<(&str, String)> = vec![("name", name.local.clone())];
        let named = self.layout == Layout::VenetianBlind && needs_complex_type(shape);
        if named {
            attrs.push(("type", self.qualified(&name.namespace, &Self::type_name(&name.local))));
            attrs.extend(occurs.iter().map(|(k, v)| (*k, v.clone())));
            file.xml.leaf("xs:element", &attrs);
            return;
        }
        match &shape.body {
            Body::Simple(spec) if shape.attrs.is_empty() && spec.enumeration.is_none() => {
                attrs.push(("type", format!("xs:{}", spec.kind.xsd_name())));
                attrs.extend(occurs.iter().map(|(k, v)| (*k, v.clone())));
                file.xml.leaf("xs:element", &attrs);
            }
            Body::Simple(spec) if shape.attrs.is_empty() => {
                attrs.extend(occurs.iter().map(|(k, v)| (*k, v.clone())));
                file.xml.open("xs:element", &attrs);
                self.write_simple_type(file, spec);
                file.xml.close("xs:element");
            }
            _ => {
                attrs.extend(occurs.iter().map(|(k, v)| (*k, v.clone())));
                file.xml.open("xs:element", &attrs);
                self.write_complex_type(file, name, shape, None);
                file.xml.close("xs:element");
            }
        }
    }

    fn write_named_type(&self, file: &mut File, name: &Name, shape: &Shape) {
        self.write_complex_type(file, name, shape, Some(Self::type_name(&name.local)));
    }

    fn write_simple_type(&self, file: &mut File, spec: &SimpleSpec) {
        file.xml.open("xs:simpleType", &[]);
        file.xml.open("xs:restriction", &[("base", format!("xs:{}", spec.kind.xsd_name()))]);
        for value in spec.enumeration.iter().flatten() {
            file.xml.leaf("xs:enumeration", &[("value", value.clone())]);
        }
        file.xml.close("xs:restriction");
        file.xml.close("xs:simpleType");
    }

    /// The complex type of an element, anonymous or with `type_name`.
    fn write_complex_type(&self, file: &mut File, name: &Name, shape: &Shape, type_name: Option<String>) {
        let mut head: Vec<(&str, String)> = Vec::new();
        if let Some(t) = type_name {
            head.push(("name", t));
        }
        if matches!(shape.body, Body::Complex { mixed: true, .. }) {
            head.push(("mixed", "true".to_owned()));
        }
        let empty = matches!(shape.body, Body::Empty) && shape.attrs.is_empty();
        if empty {
            file.xml.leaf("xs:complexType", &head);
            return;
        }
        file.xml.open("xs:complexType", &head);
        match &shape.body {
            Body::Empty => self.write_attributes(file, shape),
            Body::Simple(spec) => {
                file.xml.open("xs:simpleContent", &[]);
                file.xml.open("xs:extension", &[("base", format!("xs:{}", spec.kind.xsd_name()))]);
                self.write_attributes(file, shape);
                file.xml.close("xs:extension");
                file.xml.close("xs:simpleContent");
            }
            Body::Complex { factors, mixed } => {
                if *mixed {
                    file.xml.open("xs:choice", &[("minOccurs", "0".to_owned()), ("maxOccurs", "unbounded".to_owned())]);
                    for symbol in &shape.symbols {
                        self.write_child(file, name, symbol, &[]);
                    }
                    file.xml.close("xs:choice");
                } else {
                    file.xml.open("xs:sequence", &[]);
                    for factor in factors {
                        let occurs = self.occurs(factor.min, factor.max);
                        let symbols: Vec<&Name> = factor.symbols.iter().filter_map(|&i| shape.symbols.get(i)).collect();
                        match symbols.as_slice() {
                            [only] => self.write_child(file, name, only, &occurs),
                            many => {
                                file.xml.open("xs:choice", &occurs);
                                for symbol in many {
                                    self.write_child(file, name, symbol, &[]);
                                }
                                file.xml.close("xs:choice");
                            }
                        }
                    }
                    file.xml.close("xs:sequence");
                }
                self.write_attributes(file, shape);
            }
        }
        file.xml.close("xs:complexType");
    }

    /// A child element of `parent`: declared here, or referred to.
    fn write_child(&self, file: &mut File, parent: &Name, child: &Name, occurs: &[(&'static str, String)]) {
        let reference = child.namespace != parent.namespace
            || self.layout == Layout::SalamiSlice
            || (self.layout == Layout::RussianDoll && self.global.contains(child));
        if reference {
            if child.namespace != file.namespace {
                file.imports.insert(Import::Schema(child.namespace.clone()));
            }
            let mut attrs: Vec<(&str, String)> = vec![("ref", self.qualified(&child.namespace, &child.local))];
            attrs.extend(occurs.iter().map(|(k, v)| (*k, v.clone())));
            file.xml.leaf("xs:element", &attrs);
            return;
        }
        if let Some(shape) = self.shapes.get(child) {
            self.write_element(file, child, shape, occurs);
        }
    }

    fn write_attributes(&self, file: &mut File, shape: &Shape) {
        for a in &shape.attrs {
            let use_attr: Vec<(&str, String)> = if a.required { vec![("use", "required".to_owned())] } else { Vec::new() };
            match &a.name.namespace {
                None => {
                    let mut attrs: Vec<(&str, String)> = vec![("name", a.name.local.clone())];
                    attrs.extend(use_attr);
                    if a.spec.enumeration.is_none() {
                        attrs.push(("type", format!("xs:{}", a.spec.kind.xsd_name())));
                        file.xml.leaf("xs:attribute", &attrs);
                    } else {
                        file.xml.open("xs:attribute", &attrs);
                        self.write_simple_type(file, &a.spec);
                        file.xml.close("xs:attribute");
                    }
                }
                Some(ns) if ns == XML_NS => {
                    if XML_ATTRIBUTES.contains(&a.name.local.as_str()) {
                        file.imports.insert(Import::Xml);
                        let mut attrs: Vec<(&str, String)> = vec![("ref", format!("xml:{}", a.name.local))];
                        attrs.extend(use_attr);
                        file.xml.leaf("xs:attribute", &attrs);
                    }
                }
                Some(ns) => {
                    if file.namespace.as_ref() != Some(ns) {
                        file.imports.insert(Import::Schema(Some(ns.clone())));
                    }
                    let mut attrs: Vec<(&str, String)> = vec![("ref", self.qualified(&a.name.namespace, &a.name.local))];
                    attrs.extend(use_attr);
                    file.xml.leaf("xs:attribute", &attrs);
                }
            }
        }
    }
}

/// Whether the type of an element is a complex type (a simple element without attributes is just a built-in type or a restriction).
fn needs_complex_type(shape: &Shape) -> bool {
    !(matches!(shape.body, Body::Simple(_)) && shape.attrs.is_empty())
}
