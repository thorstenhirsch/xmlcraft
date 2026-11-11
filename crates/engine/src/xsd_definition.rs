//! Go to definition for the names an XML Schema document writes: the type of a `type` or `base`, the global element of a `ref` or a
//! `substitutionGroup`, and the group or attribute group of a `ref`. The documents of the schema are read with the resolver the app uses
//! for validation, so a definition in an included or imported document is found there too.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use xmlcraft_resolve::Resolver;
use xmlcraft_text::Buffer;
use xmlcraft_xsd::model::{
    AttributeDecl, AttributeUses, ComplexContentDef, ElementDecl, Item, Particle, QName, QRef, SimpleTypeDef, SimpleTypeKind, Term,
};
use xmlcraft_xsd::{LoadedDocument, ResolvingLoader, load};

use crate::validation::{OpenDocuments, ResolverFactory};
use crate::worker::Worker;

/// What kind of component a name refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DefinitionKind {
    Type,
    Element,
    Attribute,
    Group,
    AttributeGroup,
}

impl DefinitionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Type => "type",
            Self::Element => "element",
            Self::Attribute => "attribute",
            Self::Group => "group",
            Self::AttributeGroup => "attribute_group",
        }
    }
}

/// Where a component is defined.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    pub kind: DefinitionKind,
    /// The expanded name (`{namespace}local`).
    pub name: String,
    /// The document it is written in (empty for a text with no location).
    pub location: String,
    /// The byte range of its name.
    pub start: usize,
    pub end: usize,
    /// Where the name starts and ends as lines and columns (from 0, columns in characters), in the text the document was read as.
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

#[derive(Clone, Copy)]
enum RefKind {
    Type,
    Element,
    Attribute,
    Group,
    AttributeGroup,
}

impl RefKind {
    fn definition_kind(self) -> DefinitionKind {
        match self {
            Self::Type => DefinitionKind::Type,
            Self::Element => DefinitionKind::Element,
            Self::Attribute => DefinitionKind::Attribute,
            Self::Group => DefinitionKind::Group,
            Self::AttributeGroup => DefinitionKind::AttributeGroup,
        }
    }
}

struct Reference {
    name: QRef,
    kind: RefKind,
}

/// Where the name under `offset` in the schema `text` is defined, if it is a name that refers to another component.
pub fn definition(text: &str, location: &str, offset: usize, resolver: &Resolver) -> Result<Option<Definition>, String> {
    if text.len() > crate::analysis::MAX_ANALYSED_BYTES {
        return Err("the text is too large to analyse".to_owned());
    }
    let mut loader = ResolvingLoader::new(resolver);
    let documents = load(text, location, &mut loader);
    let Some(root) = documents.list.first() else { return Ok(None) };
    let mut references = Vec::new();
    for item in &root.document.items {
        references_in_item(item, &mut references);
    }
    let Some(reference) = references.into_iter().find(|r| r.name.span.start <= offset && offset <= r.name.span.end) else {
        return Ok(None);
    };
    let index = Index::new(&documents.list);
    let Some((found_in, span)) = index.lookup(reference.kind, &reference.name.name) else { return Ok(None) };
    // The text the definition is in: the schema itself (also when it is named by its own location), or a document the loader read.
    let defined_in = if found_in.is_empty() || found_in == location {
        Some(text)
    } else {
        loader.read.iter().find(|(read, _)| read == found_in).map(|(_, text)| text.as_str())
    };
    let Some(defined_in) = defined_in.filter(|t| span.end <= t.len()) else {
        return Err(format!("the definition is in {found_in}, whose text could not be placed"));
    };
    let buffer = Buffer::from_text(defined_in);
    let (from, to) = (buffer.position(span.start), buffer.position(span.end));
    Ok(Some(Definition {
        kind: reference.kind.definition_kind(),
        name: reference.name.name.to_string(),
        location: found_in.to_owned(),
        start: span.start,
        end: span.end,
        line: from.line,
        column: from.col,
        end_line: to.line,
        end_column: to.col,
    }))
}

/// The definitions of the global components of every document of the schema.
struct Index {
    types: HashMap<QName, (String, std::ops::Range<usize>)>,
    elements: HashMap<QName, (String, std::ops::Range<usize>)>,
    attributes: HashMap<QName, (String, std::ops::Range<usize>)>,
    groups: HashMap<QName, (String, std::ops::Range<usize>)>,
    attribute_groups: HashMap<QName, (String, std::ops::Range<usize>)>,
}

impl Index {
    fn new(documents: &[LoadedDocument]) -> Self {
        let mut index = Self {
            types: HashMap::new(),
            elements: HashMap::new(),
            attributes: HashMap::new(),
            groups: HashMap::new(),
            attribute_groups: HashMap::new(),
        };
        for document in documents {
            for item in &document.document.items {
                index.add_item(document, item);
            }
        }
        index
    }

    fn add_item(&mut self, document: &LoadedDocument, item: &Item) {
        let location = &document.document.location;
        match item {
            Item::SimpleType(definition) => {
                if let Some(name) = &definition.name {
                    self.types.entry(QName::new(document.namespace.as_deref(), name)).or_insert((location.clone(), definition.name_span.clone()));
                }
            }
            Item::ComplexType(definition) => {
                if let Some(name) = &definition.name {
                    self.types.entry(QName::new(document.namespace.as_deref(), name)).or_insert((location.clone(), definition.name_span.clone()));
                }
            }
            Item::Group(definition) => {
                if let Some(name) = &definition.name {
                    self.groups.entry(QName::new(document.namespace.as_deref(), name)).or_insert((location.clone(), definition.name_span.clone()));
                }
            }
            Item::AttributeGroup(definition) => {
                if let Some(name) = &definition.name {
                    self.attribute_groups
                        .entry(QName::new(document.namespace.as_deref(), name))
                        .or_insert((location.clone(), definition.name_span.clone()));
                }
            }
            Item::Element(definition) => {
                if let Some(name) = &definition.name {
                    self.elements.entry(QName::new(document.namespace.as_deref(), name)).or_insert((location.clone(), definition.name_span.clone()));
                }
            }
            Item::Attribute(definition) => {
                if let Some(name) = &definition.name {
                    self.attributes
                        .entry(QName::new(document.namespace.as_deref(), name))
                        .or_insert((location.clone(), definition.name_span.clone()));
                }
            }
            Item::Redefine(redefinition) => {
                for item in &redefinition.items {
                    self.add_item(document, item);
                }
            }
            Item::Include(_) | Item::Import(_) | Item::Notation(_) => {}
        }
    }

    fn lookup(&self, kind: RefKind, name: &QName) -> Option<&(String, std::ops::Range<usize>)> {
        match kind {
            RefKind::Type => self.types.get(name),
            RefKind::Element => self.elements.get(name),
            RefKind::Attribute => self.attributes.get(name),
            RefKind::Group => self.groups.get(name),
            RefKind::AttributeGroup => self.attribute_groups.get(name),
        }
    }
}

fn reference(name: &QRef, kind: RefKind, out: &mut Vec<Reference>) {
    out.push(Reference { name: name.clone(), kind });
}

fn references_in_item(item: &Item, out: &mut Vec<Reference>) {
    match item {
        Item::SimpleType(definition) => references_in_simple(definition, out),
        Item::ComplexType(definition) => references_in_complex(definition, out),
        Item::Group(definition) => {
            if let Some(content) = &definition.content {
                references_in_particle(content, out);
            }
        }
        Item::AttributeGroup(definition) => references_in_uses(&definition.uses, out),
        Item::Element(definition) => references_in_element(definition, out),
        Item::Attribute(definition) => references_in_attribute(definition, out),
        Item::Redefine(redefinition) => {
            for item in &redefinition.items {
                references_in_item(item, out);
            }
        }
        Item::Include(_) | Item::Import(_) | Item::Notation(_) => {}
    }
}

fn references_in_simple(definition: &SimpleTypeDef, out: &mut Vec<Reference>) {
    match &definition.kind {
        SimpleTypeKind::Restriction { base, inline_base, .. } => {
            if let Some(base) = base {
                reference(base, RefKind::Type, out);
            }
            if let Some(base) = inline_base {
                references_in_simple(base, out);
            }
        }
        SimpleTypeKind::List { item_type, inline_item } => {
            if let Some(item_type) = item_type {
                reference(item_type, RefKind::Type, out);
            }
            if let Some(item) = inline_item {
                references_in_simple(item, out);
            }
        }
        SimpleTypeKind::Union { member_types, inline_members } => {
            for member in member_types {
                reference(member, RefKind::Type, out);
            }
            for member in inline_members {
                references_in_simple(member, out);
            }
        }
        SimpleTypeKind::Missing => {}
    }
}

fn references_in_complex(definition: &xmlcraft_xsd::model::ComplexTypeDef, out: &mut Vec<Reference>) {
    match &definition.content {
        ComplexContentDef::Implicit { particle, attributes } => {
            if let Some(particle) = particle {
                references_in_particle(particle, out);
            }
            references_in_uses(attributes, out);
        }
        ComplexContentDef::Simple { base, inline_base, attributes, .. } => {
            if let Some(base) = base {
                reference(base, RefKind::Type, out);
            }
            if let Some(base) = inline_base {
                references_in_simple(base, out);
            }
            references_in_uses(attributes, out);
        }
        ComplexContentDef::Complex { base, particle, attributes, .. } => {
            if let Some(base) = base {
                reference(base, RefKind::Type, out);
            }
            if let Some(particle) = particle {
                references_in_particle(particle, out);
            }
            references_in_uses(attributes, out);
        }
    }
}

fn references_in_particle(particle: &Particle, out: &mut Vec<Reference>) {
    match &particle.term {
        Term::Element(element) => references_in_element(element, out),
        Term::GroupRef(group) => reference(group, RefKind::Group, out),
        Term::Sequence(particles) | Term::Choice(particles) | Term::All(particles) => {
            for particle in particles {
                references_in_particle(particle, out);
            }
        }
        Term::Any(_) => {}
    }
}

fn references_in_uses(uses: &AttributeUses, out: &mut Vec<Reference>) {
    for attribute in &uses.attributes {
        references_in_attribute(attribute, out);
    }
    for group in &uses.groups {
        reference(&group.reference, RefKind::AttributeGroup, out);
    }
}

fn references_in_element(element: &ElementDecl, out: &mut Vec<Reference>) {
    if let Some(reference_) = &element.reference {
        reference(reference_, RefKind::Element, out);
    }
    if let Some(type_name) = &element.type_name {
        reference(type_name, RefKind::Type, out);
    }
    if let Some(substitution) = &element.substitution_group {
        reference(substitution, RefKind::Element, out);
    }
    if let Some(inline) = &element.inline_type {
        match inline.as_ref() {
            xmlcraft_xsd::model::InlineType::Simple(definition) => references_in_simple(definition, out),
            xmlcraft_xsd::model::InlineType::Complex(definition) => references_in_complex(definition, out),
        }
    }
}

fn references_in_attribute(attribute: &AttributeDecl, out: &mut Vec<Reference>) {
    if let Some(reference_) = &attribute.reference {
        reference(reference_, RefKind::Attribute, out);
    }
    if let Some(type_name) = &attribute.type_name {
        reference(type_name, RefKind::Type, out);
    }
    if let Some(inline) = &attribute.inline_type {
        references_in_simple(inline, out);
    }
}

struct Request {
    version: u64,
    offset: usize,
    text: Buffer,
    location: String,
    open: Vec<(String, Buffer)>,
}

/// What a request found: the version and place it was about, and the definition there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub version: u64,
    pub offset: usize,
    pub definition: Option<Definition>,
    pub error: Option<String>,
}

/// Finds definitions on a worker thread, so that a schema document with a slow include or import never makes the window wait.
pub struct DefinitionFinder {
    worker: Worker<Request, Answer>,
}

impl DefinitionFinder {
    pub fn new(resolvers: ResolverFactory) -> Self {
        Self { worker: Worker::new("definition", move |request| run(&resolvers, request)) }
    }

    /// A finder with no thread, for tests and the web.
    pub fn inline(resolvers: ResolverFactory) -> Self {
        Self { worker: Worker::inline(move |request| run(&resolvers, request)) }
    }

    pub fn on_done(&self, notify: impl Fn() + Send + Sync + 'static) {
        self.worker.on_done(notify);
    }

    /// Find the definition of the name at `offset` in `text` (version `version` of the document at `path`). `open` are the open documents,
    /// by their paths, so that an included document being edited is read as it is.
    pub fn find(&self, document: u64, version: u64, text: &Buffer, path: Option<&Path>, offset: usize, open: Vec<(String, Buffer)>) {
        let location = path.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        self.worker.submit(document, Request { version, offset, text: text.clone(), location, open });
    }

    pub fn poll(&self) -> Vec<(u64, Answer)> {
        self.worker.poll()
    }
}

fn run(resolvers: &ResolverFactory, request: Request) -> Answer {
    let resolver = resolvers(Arc::new(OpenDocuments::new(request.open)));
    let text = request.text.text();
    let outcome = crate::guard(|| definition(&text, &request.location, request.offset, &resolver)).unwrap_or_else(|message| {
        log::error!("finding a definition failed: {message}");
        Err("finding the definition failed (a bug)".to_owned())
    });
    match outcome {
        Ok(definition) => Answer { version: request.version, offset: request.offset, definition, error: None },
        Err(error) => Answer { version: request.version, offset: request.offset, definition: None, error: Some(error) },
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use xmlcraft_resolve::{Buffers, Environment, FileAccess, FixedClock, Policy};

    use super::*;

    struct Files(HashMap<String, Vec<u8>>);

    impl FileAccess for Files {
        fn read(&self, path: &str) -> Result<Vec<u8>, String> {
            self.0.get(path).cloned().ok_or_else(|| format!("{path}: no such file"))
        }
    }

    fn resolver(files: HashMap<String, Vec<u8>>) -> Resolver {
        let env =
            Environment { buffers: Arc::new(NoBuffers), files: Arc::new(Files(files)), fetcher: None, cache: None, clock: Arc::new(FixedClock(0)) };
        Resolver::new(env, Policy { offline: true, ..Policy::default() }, &[])
    }

    struct NoBuffers;

    impl Buffers for NoBuffers {
        fn text(&self, _: &str) -> Option<Arc<str>> {
            None
        }
    }

    const XS: &str = "http://www.w3.org/2001/XMLSchema";

    #[test]
    fn a_type_a_group_and_a_substitution_head_are_found() {
        let text = format!(
            r#"<xs:schema xmlns:xs="{XS}" targetNamespace="urn:t" xmlns:t="urn:t">
  <xs:group name="body"><xs:sequence><xs:element name="x" type="xs:string"/></xs:sequence></xs:group>
  <xs:element name="head" type="xs:string"/>
  <xs:element name="order" type="t:OrderType" substitutionGroup="t:head"/>
  <xs:complexType name="OrderType"><xs:group ref="t:body"/></xs:complexType>
</xs:schema>"#
        );
        let resolver = resolver(HashMap::new());
        // The type of the element.
        let at = text.find("t:OrderType").expect("the name") + 2;
        let found = definition(&text, "/w/a.xsd", at, &resolver).expect("runs").expect("a definition");
        assert_eq!((found.kind, found.name.as_str(), found.location.as_str()), (DefinitionKind::Type, "{urn:t}OrderType", "/w/a.xsd"));
        assert_eq!(text.get(found.start..found.end), Some("OrderType"));
        assert_eq!((found.line, found.end_column - found.column), (4, "OrderType".len()));
        // The substitution head.
        let at = text.find("t:head").expect("the name") + 2;
        let found = definition(&text, "/w/a.xsd", at, &resolver).expect("runs").expect("a definition");
        assert_eq!((found.kind, found.name.as_str()), (DefinitionKind::Element, "{urn:t}head"));
        // The group reference.
        let at = text.find("t:body").expect("the name") + 2;
        let found = definition(&text, "/w/a.xsd", at, &resolver).expect("runs").expect("a definition");
        assert_eq!((found.kind, found.name.as_str()), (DefinitionKind::Group, "{urn:t}body"));
    }

    #[test]
    fn a_definition_in_an_included_document_names_that_document_and_its_span() {
        let root = format!(
            r#"<xs:schema xmlns:xs="{XS}" targetNamespace="urn:t" xmlns:t="urn:t"><xs:include schemaLocation="types.xsd"/><xs:element name="order" type="t:OrderType"/></xs:schema>"#
        );
        let types = format!(
            r#"<xs:schema xmlns:xs="{XS}" targetNamespace="urn:t"><xs:complexType name="OrderType"><xs:sequence/></xs:complexType></xs:schema>"#
        );
        let resolver = resolver(HashMap::from([("/w/types.xsd".to_owned(), types.clone().into_bytes())]));
        let at = root.find("t:OrderType").expect("the name") + 2;
        let found = definition(&root, "/w/root.xsd", at, &resolver).expect("runs").expect("a definition");
        assert_eq!((found.kind, found.location.as_str()), (DefinitionKind::Type, "/w/types.xsd"));
        assert_eq!(types.get(found.start..found.end), Some("OrderType"));
        assert_eq!((found.line, found.column), (0, types.find("OrderType").expect("the name")), "placed in the included text");
    }

    #[test]
    fn a_place_that_names_nothing_or_a_name_that_is_not_defined_has_no_definition() {
        let text = format!(r#"<xs:schema xmlns:xs="{XS}"><xs:element name="a" type="missing"/></xs:schema>"#);
        let resolver = resolver(HashMap::new());
        assert_eq!(definition(&text, "", text.find("missing").expect("the name") + 2, &resolver).expect("runs"), None);
        assert_eq!(definition(&text, "", text.find("<xs:schema").expect("the root"), &resolver).expect("runs"), None);
    }
}
