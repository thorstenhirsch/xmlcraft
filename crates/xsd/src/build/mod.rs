//! Putting a schema together: the documents' top-level definitions are registered by name and given their numbers, and the components
//! are built on demand, resolving the names they refer to. A type is built when something first needs it complete (a type derived
//! from it), so the order of the definitions in the documents does not matter; one that is only named (the type of an element) needs
//! just its number, so a type may contain elements of its own type. A definition that comes back to itself through its derivation is a
//! circular definition, reported once.

mod complex;
mod declarations;
mod particles;
mod simple;
mod wildcards;

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use xmlcraft_datatypes::{ExpandedName, NamespaceContext, SimpleType};
use xmlcraft_syntax::{Code, Diagnostic};

use crate::loader::{Documents, SourcedDiagnostic};
use crate::model::*;
use crate::schema::{
    AttributeId, AttributeUse as SAttributeUse, ComplexType, ConstraintId, Content, ContentType, ElementId, Origin, Particle as SParticle, Schema,
    Term as STerm, TypeEntry, TypeId, TypeKind, Wildcard as SWildcard,
};

/// How deep definitions may depend on one another (a type derived from a type derived from …) before the schema is refused.
pub const MAX_DEPTH: usize = 400;

/// The schema, and what is wrong with its documents and components.
#[derive(Debug)]
pub struct Assembly {
    pub schema: Schema,
    pub diagnostics: Vec<SourcedDiagnostic>,
}

/// Assemble the components of `documents` (as [`crate::load`] gave them).
pub fn assemble(documents: &Documents) -> Assembly {
    let mut builder = Builder::new(documents);
    builder.register();
    builder.build_all();
    builder.finish()
}

/// Where a top-level definition is: the document and the index into its items.
pub(super) type Place = (usize, usize);

/// The attribute uses and the wildcard of an attribute group, expanded.
#[derive(Clone, Debug, Default)]
pub(super) struct AttributeSet {
    pub(super) uses: Vec<SAttributeUse>,
    pub(super) prohibited: Vec<QName>,
    pub(super) wildcard: Option<SWildcard>,
}

/// The default or fixed value of an element, waiting for its type to be complete.
struct PendingValue {
    document: usize,
    element: ElementId,
    constraint: ValueConstraint,
    fixed: bool,
}

/// An anonymous complex type of a local element, built after the type that holds the element, so that a type may contain elements of
/// types that contain it.
struct Job<'a> {
    document: usize,
    def: &'a ComplexTypeDef,
    id: TypeId,
}

/// A keyref's `refer`, resolved once all the constraints are known.
struct PendingRefer {
    document: usize,
    keyref: ConstraintId,
    refer: QRef,
}

/// How many particles all the content models of a schema may have, group references expanded, before the schema is refused.
const MAX_PARTICLES: usize = 500_000;

pub(super) struct Builder<'a> {
    docs: &'a Documents,
    pub(super) schema: Schema,
    diags: Vec<SourcedDiagnostic>,
    type_places: HashMap<QName, Place>,
    group_places: HashMap<QName, Place>,
    attribute_group_places: HashMap<QName, Place>,
    element_places: HashMap<QName, Place>,
    attribute_places: HashMap<QName, Place>,
    notation_places: HashMap<QName, Place>,
    built: HashSet<TypeId>,
    /// Types, groups and attribute groups whose building has begun and not ended.
    progress: HashSet<(Kind, QName)>,
    /// Definitions that could not be built; they are not tried again.
    failed: HashSet<(Kind, QName)>,
    /// Circular definitions that were reported.
    reported: HashSet<(Kind, QName)>,
    depth: usize,
    /// The model groups that were built, with how many particles each has.
    group_memo: HashMap<QName, Option<(Rc<SParticle>, usize)>>,
    attribute_group_memo: HashMap<QName, Option<Rc<AttributeSet>>>,
    pending: Vec<PendingValue>,
    queue: Vec<Job<'a>>,
    element_built: HashSet<ElementId>,
    particles_left: usize,
    particle_limit_reported: bool,
    constraint_names: HashMap<QName, ConstraintId>,
    pending_refers: Vec<PendingRefer>,
    /// Elements with a substitution group: the member, the head and where the member was written.
    pending_substitutions: Vec<(usize, ElementId, ElementId, Range<usize>)>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum Kind {
    Type,
    Group,
    AttributeGroup,
    Element,
    Attribute,
}

/// Why a name did not give a component.
pub(super) enum Lookup {
    Circular,
    TooDeep,
    /// The definition had mistakes of its own, which were reported.
    Failed,
}

/// The namespace declarations at a value, as a context for reading qualified names in it.
pub(super) struct Scope<'n>(pub &'n Namespaces);

impl NamespaceContext for Scope<'_> {
    fn resolve(&self, prefix: Option<&str>) -> Option<String> {
        self.0.iter().find(|(p, _)| p.as_deref() == prefix).map(|(_, uri)| uri.clone())
    }
}

pub(super) fn expanded(name: &QName) -> ExpandedName {
    ExpandedName { namespace: name.namespace.clone(), local: name.local.clone() }
}

/// Note where the definition `name` is; false when another definition of the name was there first.
pub(super) fn claim(places: &mut HashMap<QName, Place>, name: &QName, place: Place) -> bool {
    match places.entry(name.clone()) {
        Entry::Occupied(_) => false,
        Entry::Vacant(vacant) => {
            vacant.insert(place);
            true
        }
    }
}

pub(super) fn xs(local: &str) -> QName {
    QName::new(Some(crate::XSD_NAMESPACE), local)
}

impl<'a> Builder<'a> {
    fn new(docs: &'a Documents) -> Self {
        Self {
            docs,
            schema: crate::schema::with_builtins(docs.list.iter().map(|d| d.location.clone()).collect()),
            diags: Vec::new(),
            type_places: HashMap::new(),
            group_places: HashMap::new(),
            attribute_group_places: HashMap::new(),
            element_places: HashMap::new(),
            attribute_places: HashMap::new(),
            notation_places: HashMap::new(),
            built: HashSet::new(),
            progress: HashSet::new(),
            failed: HashSet::new(),
            reported: HashSet::new(),
            depth: 0,
            group_memo: HashMap::new(),
            attribute_group_memo: HashMap::new(),
            pending: Vec::new(),
            queue: Vec::new(),
            element_built: HashSet::new(),
            particles_left: MAX_PARTICLES,
            particle_limit_reported: false,
            constraint_names: HashMap::new(),
            pending_refers: Vec::new(),
            pending_substitutions: Vec::new(),
        }
    }

    pub(super) fn report(&mut self, doc: usize, code: Code, span: Range<usize>, args: &[&str]) {
        self.diags.push(SourcedDiagnostic { source: doc, diagnostic: Diagnostic::new(code, span, args.iter().map(|a| (*a).to_owned()).collect()) });
    }

    pub(super) fn origin(doc: usize, span: &Range<usize>) -> Origin {
        Origin { document: doc, range: span.clone() }
    }

    pub(super) fn document(&self, doc: usize) -> Option<&'a crate::loader::LoadedDocument> {
        self.docs.list.get(doc)
    }

    /// The namespace the components of document `doc` are in.
    pub(super) fn namespace_of(&self, doc: usize) -> Option<String> {
        self.docs.list.get(doc).and_then(|d| d.namespace.clone())
    }

    /// The name a definition in document `doc` has.
    pub(super) fn named(&self, doc: usize, local: &str) -> QName {
        QName { namespace: self.namespace_of(doc), local: local.to_owned() }
    }

    /// A reference written in `doc`: a document with no target namespace that is included into one takes its namespace for the names
    /// that have none.
    pub(super) fn qualify(&self, doc: usize, name: &QName) -> QName {
        match self.docs.list.get(doc) {
            Some(d) if d.chameleon && name.namespace.is_none() => QName { namespace: d.namespace.clone(), local: name.local.clone() },
            _ => name.clone(),
        }
    }

    /// May `doc` refer to the components of this namespace? Its own, those of the language and those it imports.
    fn visible(&self, doc: usize, namespace: &Option<String>) -> bool {
        let Some(d) = self.docs.list.get(doc) else { return false };
        namespace.as_deref() == Some(crate::XSD_NAMESPACE) || *namespace == d.namespace || d.imports.contains(namespace)
    }

    /// The reference `reference` of document `doc` as a name that `doc` may use, or the reason it may not (reported).
    pub(super) fn visible_name(&mut self, doc: usize, reference: &QRef, what: &str) -> Option<QName> {
        let name = self.qualify(doc, &reference.name);
        if self.visible(doc, &name.namespace) {
            Some(name)
        } else {
            self.report(doc, Code::XsdNotImported, reference.span.clone(), &[what, &name.to_string()]);
            None
        }
    }

    pub(super) fn item(&self, place: Place) -> Option<&'a Item> {
        self.docs.list.get(place.0).and_then(|d| d.document.items.get(place.1))
    }

    // ---- registering ----

    fn register(&mut self) {
        let docs = self.docs;
        for (doc, loaded) in docs.list.iter().enumerate() {
            for (index, item) in loaded.document.items.iter().enumerate() {
                let place = (doc, index);
                match item {
                    Item::SimpleType(SimpleTypeDef { name: Some(name), name_span, span, .. })
                    | Item::ComplexType(ComplexTypeDef { name: Some(name), name_span, span, .. }) => {
                        let qname = self.named(doc, name);
                        if self.type_places.contains_key(&qname) || self.schema.global_types.contains_key(&qname) {
                            self.report(doc, Code::XsdDuplicateComponent, name_span.clone(), &["type", &qname.to_string()]);
                        } else {
                            // The number is reserved now; the definition is filled in when it is built.
                            let id = TypeId(self.schema.types.len());
                            self.schema.types.push(TypeEntry {
                                name: Some(qname.clone()),
                                kind: TypeKind::Complex(Box::new(any_type_complex())),
                                base: None,
                                derivation: None,
                                final_: DerivationSet::default(),
                                origin: Some(Self::origin(doc, span)),
                            });
                            self.schema.global_types.insert(qname.clone(), id);
                            self.type_places.insert(qname, place);
                        }
                    }
                    Item::Group(GroupDef { name: Some(name), name_span, .. }) => {
                        self.register_place(doc, place, name, name_span, "group", Kind::Group)
                    }
                    Item::AttributeGroup(AttributeGroupDef { name: Some(name), name_span, .. }) => {
                        self.register_place(doc, place, name, name_span, "attribute group", Kind::AttributeGroup)
                    }
                    Item::Element(e) => self.register_element(doc, place, e),
                    Item::Attribute(a) => self.register_attribute(doc, place, a),
                    Item::Redefine(r) => {
                        self.report(doc, Code::XsdUnsupported, r.span.clone(), &["xs:redefine", "the redefining definitions are not applied"]);
                    }
                    Item::Notation(NotationDef { name: Some(name), name_span, .. }) => {
                        let qname = self.named(doc, name);
                        if !claim(&mut self.notation_places, &qname, place) {
                            self.report(doc, Code::XsdDuplicateComponent, name_span.clone(), &["notation", &qname.to_string()]);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn register_place(&mut self, doc: usize, place: Place, name: &str, name_span: &Range<usize>, what: &str, kind: Kind) {
        let qname = self.named(doc, name);
        let places = match kind {
            Kind::Group => &mut self.group_places,
            _ => &mut self.attribute_group_places,
        };
        if !claim(places, &qname, place) {
            self.report(doc, Code::XsdDuplicateComponent, name_span.clone(), &[what, &qname.to_string()]);
        }
    }

    // ---- types ----

    fn build_all(&mut self) {
        let mut types: Vec<(Place, QName)> = self.type_places.iter().map(|(n, p)| (*p, n.clone())).collect();
        types.sort();
        for (_, name) in types {
            if let Some(&id) = self.schema.global_types.get(&name) {
                let _ = self.ensure_built(id);
            }
        }
        self.build_global_attributes();
        self.build_notations();
        self.build_global_elements();
        self.drain_queue();
        self.finish_value_constraints();
        self.finish_constraint_refs();
        self.finish_substitutions();
    }

    /// Build the anonymous types that were put off.
    fn drain_queue(&mut self) {
        while let Some(job) = self.queue.pop() {
            self.depth = 0;
            let _ = self.complex_type(job.document, job.def, None, Some(job.id));
        }
    }

    /// Make sure the type `id` is complete. A type that is being built is a circular definition.
    pub(super) fn ensure_built(&mut self, id: TypeId) -> Result<(), Lookup> {
        if id.0 < self.first_user_type() || self.built.contains(&id) {
            return Ok(());
        }
        let Some(name) = self.schema.types.get(id.0).and_then(|t| t.name.clone()) else { return Ok(()) };
        let key = (Kind::Type, name.clone());
        if self.failed.contains(&key) {
            return Err(Lookup::Failed);
        }
        if self.progress.contains(&key) {
            return Err(Lookup::Circular);
        }
        if self.depth >= MAX_DEPTH {
            return Err(Lookup::TooDeep);
        }
        let Some(&place) = self.type_places.get(&name) else { return Ok(()) };
        self.progress.insert(key.clone());
        self.depth += 1;
        let built = match self.item(place) {
            Some(Item::SimpleType(def)) => self.simple_type(place.0, def, Some(name.clone()), Some(id)).is_some(),
            Some(Item::ComplexType(def)) => self.complex_type(place.0, def, Some(name.clone()), Some(id)).is_some(),
            _ => false,
        };
        self.depth -= 1;
        self.progress.remove(&key);
        if built {
            self.built.insert(id);
            Ok(())
        } else {
            self.failed.insert(key);
            Err(Lookup::Failed)
        }
    }

    /// The number of the first type that is not built in.
    fn first_user_type(&self) -> usize {
        self.schema.builtin_count
    }

    /// Resolve a reference to a type written in `doc`, and report what is wrong with it. The type may be incomplete.
    pub(super) fn resolve_type(&mut self, doc: usize, reference: &QRef) -> Option<TypeId> {
        let name = self.visible_name(doc, reference, "type")?;
        match self.schema.global_types.get(&name) {
            Some(&id) => Some(id),
            None => {
                self.report(doc, Code::XsdUnresolved, reference.span.clone(), &["type", &name.to_string()]);
                None
            }
        }
    }

    /// Resolve a reference to a type and make it complete (for a type that is derived from it).
    pub(super) fn resolve_type_built(&mut self, doc: usize, reference: &QRef) -> Option<TypeId> {
        let id = self.resolve_type(doc, reference)?;
        let name = self.qualify(doc, &reference.name);
        match self.ensure_built(id) {
            Ok(()) => Some(id),
            Err(Lookup::Failed) => None,
            Err(Lookup::Circular) => {
                if self.reported.insert((Kind::Type, name.clone())) {
                    self.report(doc, Code::XsdCircular, reference.span.clone(), &[&name.to_string(), "the type is derived from itself"]);
                }
                None
            }
            Err(Lookup::TooDeep) => {
                self.report(doc, Code::XsdTooDeep, reference.span.clone(), &[&name.to_string()]);
                None
            }
        }
    }

    /// The simple type a reference names, complete; a complex type is reported.
    pub(super) fn resolve_simple(&mut self, doc: usize, reference: &QRef) -> Option<(TypeId, Arc<SimpleType>)> {
        let id = self.resolve_type_built(doc, reference)?;
        match self.schema.types.get(id.0).map(|t| &t.kind) {
            Some(TypeKind::Simple(arc)) => Some((id, arc.clone())),
            _ => {
                let shown = self.qualify(doc, &reference.name).to_string();
                self.report(doc, Code::XsdWrongKind, reference.span.clone(), &[&shown, "a complex type, where a simple type is needed"]);
                None
            }
        }
    }

    /// Put `entry` in the place reserved for it, or at the end of the table for a type that has no name.
    pub(super) fn put_type(&mut self, slot: Option<TypeId>, entry: TypeEntry) -> TypeId {
        match slot {
            Some(id) if id.0 < self.schema.types.len() => {
                if let Some(place) = self.schema.types.get_mut(id.0) {
                    *place = entry;
                }
                id
            }
            _ => {
                let id = TypeId(self.schema.types.len());
                self.schema.types.push(entry);
                id
            }
        }
    }

    /// A slot for a type that has no name yet, so that the elements declared inside it can say where they are.
    pub(super) fn reserve_type(&mut self, origin: Origin) -> TypeId {
        let id = TypeId(self.schema.types.len());
        self.schema.types.push(TypeEntry {
            name: None,
            kind: TypeKind::Complex(Box::new(any_type_complex())),
            base: None,
            derivation: None,
            final_: DerivationSet::default(),
            origin: Some(origin),
        });
        id
    }

    pub(super) fn any_type(&self) -> TypeId {
        TypeId(0)
    }

    pub(super) fn any_simple_type(&self) -> Option<TypeId> {
        self.schema.global_types.get(&xs("anySimpleType")).copied()
    }

    fn finish(self) -> Assembly {
        let mut diagnostics = self.diags;
        diagnostics.sort_by_key(|d| (d.source, d.diagnostic.span.start, d.diagnostic.span.end));
        Assembly { schema: self.schema, diagnostics }
    }
}

/// `anyType`'s content: any elements and text, as far as the schema says.
pub(crate) fn any_type_complex() -> ComplexType {
    let wildcard = SWildcard { constraint: crate::schema::NamespaceSet::Any, process_contents: ProcessContents::Lax };
    ComplexType {
        abstract_: false,
        block: DerivationSet::default(),
        content: ContentType::Mixed(Content { particle: SParticle { min: 0, max: None, term: STerm::Wildcard(wildcard.clone()) } }),
        attributes: Vec::new(),
        attribute_wildcard: Some(wildcard),
    }
}
