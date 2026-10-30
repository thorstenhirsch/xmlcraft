//! The schema: the components of all the documents of a schema, put together and resolved.
//!
//! Components refer to each other by number: a type by its [`TypeId`], an element by its [`ElementId`], …, into the tables of the
//! [`Schema`]. The built-in types are the first entries of the type table and are the same in every schema. Everything that has a
//! place in a document keeps it (`origin`), for diagnostics and for go-to-definition.

use std::collections::HashMap;
use std::sync::Arc;

use xmlcraft_datatypes::{SimpleType, Value};

use crate::model::{DerivationMethod, DerivationSet, IdentityKind, NamespaceConstraint, ProcessContents, QName};

/// Where a component was written: the document (an index into [`Schema::documents`]) and the byte range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub document: usize,
    pub range: std::ops::Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TypeId(pub usize);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ElementId(pub usize);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AttributeId(pub usize);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ConstraintId(pub usize);

/// A type definition: simple or complex.
#[derive(Clone, Debug)]
pub enum TypeKind {
    Simple(Arc<SimpleType>),
    Complex(Box<ComplexType>),
}

#[derive(Clone, Debug)]
pub struct TypeEntry {
    pub name: Option<QName>,
    pub kind: TypeKind,
    /// The type this one is derived from (`None` for `xs:anyType`).
    pub base: Option<TypeId>,
    pub derivation: Option<DerivationMethod>,
    /// `final`: which derivations are not allowed.
    pub final_: DerivationSet,
    pub origin: Option<Origin>,
}

/// A wildcard with its namespace constraint as a set of namespaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wildcard {
    pub constraint: NamespaceSet,
    pub process_contents: ProcessContents,
}

/// The namespaces a wildcard allows: any, anything but the given one (and no namespace), or a list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamespaceSet {
    Any,
    /// Any namespace except this one (`None`: the absent namespace is excluded too) and no namespace.
    Not(Option<String>),
    /// These namespaces (`None` is no namespace).
    List(Vec<Option<String>>),
}

impl NamespaceSet {
    pub fn allows(&self, namespace: Option<&str>) -> bool {
        match self {
            Self::Any => true,
            Self::Not(excluded) => namespace.is_some() && namespace != excluded.as_deref(),
            Self::List(list) => list.iter().any(|n| n.as_deref() == namespace),
        }
    }

    /// The set from the `namespace` attribute of a wildcard written in a document whose target namespace is `target`.
    pub fn from_constraint(constraint: &NamespaceConstraint, target: Option<&str>) -> Self {
        use crate::model::NamespaceToken;
        match constraint {
            NamespaceConstraint::Any => Self::Any,
            NamespaceConstraint::Other => Self::Not(target.map(str::to_owned)),
            NamespaceConstraint::List(tokens) => Self::List(
                tokens
                    .iter()
                    .map(|t| match t {
                        NamespaceToken::Uri(u) => Some(u.clone()),
                        NamespaceToken::Local => None,
                        NamespaceToken::TargetNamespace => target.map(str::to_owned),
                    })
                    .collect(),
            ),
        }
    }
}

/// What an element may contain.
#[derive(Clone, Debug)]
pub enum ContentType {
    Empty,
    /// Text of a simple type, no elements.
    Simple(Arc<SimpleType>),
    /// Elements only.
    ElementOnly(Content),
    /// Elements and text.
    Mixed(Content),
}

/// A content model: the particle as written (group references expanded), and compiled to an automaton.
#[derive(Clone, Debug)]
pub struct Content {
    pub particle: Particle,
}

#[derive(Clone, Debug)]
pub struct Particle {
    pub min: u32,
    /// `None`: unbounded.
    pub max: Option<u32>,
    pub term: Term,
}

#[derive(Clone, Debug)]
pub enum Term {
    Element(ElementId),
    Wildcard(Wildcard),
    Sequence(Vec<Particle>),
    Choice(Vec<Particle>),
    All(Vec<Particle>),
}

/// An attribute declaration used by a type.
#[derive(Clone, Debug)]
pub struct AttributeUse {
    pub attribute: AttributeId,
    pub required: bool,
    /// The default or fixed value that overrides the declaration's, if the use gives one.
    pub value_constraint: Option<ValueConstraint>,
}

#[derive(Clone, Debug)]
pub struct ValueConstraint {
    pub value: Value,
    pub lexical: String,
    pub fixed: bool,
}

#[derive(Clone, Debug)]
pub struct ComplexType {
    pub abstract_: bool,
    pub block: DerivationSet,
    pub content: ContentType,
    pub attributes: Vec<AttributeUse>,
    pub attribute_wildcard: Option<Wildcard>,
}

#[derive(Clone, Debug)]
pub struct ElementDecl {
    pub name: QName,
    pub type_id: TypeId,
    /// The complex type a local element is declared in; `None` for a global element.
    pub scope: Option<TypeId>,
    pub nillable: bool,
    pub value_constraint: Option<ValueConstraint>,
    pub abstract_: bool,
    pub substitution_group: Option<ElementId>,
    pub block: DerivationSet,
    pub final_: DerivationSet,
    pub identity_constraints: Vec<ConstraintId>,
    pub origin: Origin,
}

#[derive(Clone, Debug)]
pub struct AttributeDecl {
    pub name: QName,
    pub type_: Arc<SimpleType>,
    pub value_constraint: Option<ValueConstraint>,
    pub global: bool,
    pub origin: Origin,
}

#[derive(Clone, Debug)]
pub struct NotationDecl {
    pub name: QName,
    pub public: Option<String>,
    pub system: Option<String>,
    pub origin: Origin,
}

#[derive(Clone, Debug)]
pub struct IdentityConstraint {
    pub kind: IdentityKind,
    pub name: QName,
    pub refer: Option<ConstraintId>,
    pub selector: String,
    pub fields: Vec<String>,
    /// The namespace declarations at the constraint, for the prefixes in its XPath expressions.
    pub namespaces: Vec<(Option<String>, String)>,
    /// The expressions, parsed; none when one of them is not valid (that was reported).
    pub paths: Option<crate::xpath::ConstraintPaths>,
    pub origin: Origin,
}

/// The assembled schema.
#[derive(Debug)]
pub struct Schema {
    /// The locations of the documents; [`Origin::document`] indexes this.
    pub documents: Vec<String>,
    pub types: Vec<TypeEntry>,
    pub elements: Vec<ElementDecl>,
    pub attributes: Vec<AttributeDecl>,
    pub notations: Vec<NotationDecl>,
    pub constraints: Vec<IdentityConstraint>,
    pub(crate) global_types: HashMap<QName, TypeId>,
    pub(crate) global_elements: HashMap<QName, ElementId>,
    pub(crate) global_attributes: HashMap<QName, AttributeId>,
    pub(crate) global_notations: HashMap<QName, usize>,
    /// How many types are built in (the first entries of [`Schema::types`]).
    pub(crate) builtin_count: usize,
    /// The compiled content model of each complex type with elements in it, by type number.
    pub(crate) models: Vec<Option<crate::compile::ContentModel>>,
}

impl Schema {
    pub fn type_by_name(&self, name: &QName) -> Option<TypeId> {
        self.global_types.get(name).copied()
    }

    pub fn element_by_name(&self, name: &QName) -> Option<ElementId> {
        self.global_elements.get(name).copied()
    }

    /// Whether `id` is a global element: declared at the top of a schema document, and so a root a document may have. (`scope` is `None`
    /// for it, and also for an element declared in a named model group, which belongs to whichever types use the group.)
    pub fn is_global(&self, id: ElementId) -> bool {
        self.element(id).is_some_and(|e| self.global_elements.get(&e.name) == Some(&id))
    }

    pub fn attribute_by_name(&self, name: &QName) -> Option<AttributeId> {
        self.global_attributes.get(name).copied()
    }

    /// The compiled content model of a complex type with element-only or mixed content; none for other types.
    pub fn content_model(&self, id: TypeId) -> Option<&crate::compile::ContentModel> {
        self.models.get(id.0).and_then(Option::as_ref)
    }

    /// Does the type `t` come from `base` by any chain of derivations (or is it `base`)?
    pub fn derives_from(&self, t: TypeId, base: TypeId) -> bool {
        let mut at = Some(t);
        for _ in 0..=self.types.len() {
            match at {
                Some(id) if id == base => return true,
                Some(id) => at = self.types.get(id.0).and_then(|e| e.base),
                None => return false,
            }
        }
        false
    }

    /// Is the type `from` derived from `to` in ways that `blocked` and the types on the way do not stop? (What may take the place of
    /// an element in a substitution group, and what `xsi:type` may name.)
    pub fn derivation_allowed(&self, from: TypeId, to: TypeId, blocked: DerivationSet) -> bool {
        let mut at = from;
        for _ in 0..=self.types.len() {
            if at == to {
                return true;
            }
            let Some(entry) = self.types.get(at.0) else { return false };
            let (Some(base), method) = (entry.base, entry.derivation) else { return false };
            let (stopped_here, stopped_by_base) = match method {
                Some(DerivationMethod::Extension) => (blocked.extension, self.type_blocks(base, DerivationMethod::Extension)),
                _ => (blocked.restriction, self.type_blocks(base, DerivationMethod::Restriction)),
            };
            if stopped_here || stopped_by_base {
                return false;
            }
            at = base;
        }
        false
    }

    /// Does the type `id` (a complex type) forbid being derived from in this way?
    fn type_blocks(&self, id: TypeId, method: DerivationMethod) -> bool {
        match self.types.get(id.0).map(|t| &t.kind) {
            Some(TypeKind::Complex(c)) => match method {
                DerivationMethod::Extension => c.block.extension,
                DerivationMethod::Restriction => c.block.restriction,
            },
            _ => false,
        }
    }

    pub fn type_entry(&self, id: TypeId) -> Option<&TypeEntry> {
        self.types.get(id.0)
    }

    pub fn element(&self, id: ElementId) -> Option<&ElementDecl> {
        self.elements.get(id.0)
    }

    pub fn attribute(&self, id: AttributeId) -> Option<&AttributeDecl> {
        self.attributes.get(id.0)
    }
}

/// A schema that has only the built-in types: `anyType` and the simple types of the language.
pub(crate) fn with_builtins(documents: Vec<String>) -> Schema {
    use crate::model::DerivationSet;
    let mut schema = Schema {
        documents,
        types: Vec::new(),
        elements: Vec::new(),
        attributes: Vec::new(),
        notations: Vec::new(),
        constraints: Vec::new(),
        global_types: HashMap::new(),
        global_elements: HashMap::new(),
        global_attributes: HashMap::new(),
        global_notations: HashMap::new(),
        builtin_count: 0,
        models: Vec::new(),
    };
    let xs = |local: &str| QName::new(Some(crate::XSD_NAMESPACE), local);
    schema.types.push(TypeEntry {
        name: Some(xs("anyType")),
        kind: TypeKind::Complex(Box::new(crate::build::any_type_complex())),
        base: None,
        derivation: None,
        final_: DerivationSet::default(),
        origin: None,
    });
    schema.global_types.insert(xs("anyType"), TypeId(0));
    for local in SimpleType::builtin_names() {
        let Some(arc) = SimpleType::builtin(local) else { continue };
        let id = TypeId(schema.types.len());
        schema.types.push(TypeEntry {
            name: Some(xs(local)),
            kind: TypeKind::Simple(arc),
            base: None,
            derivation: None,
            final_: DerivationSet::default(),
            origin: None,
        });
        schema.global_types.insert(xs(local), id);
    }
    // The bases: a built-in type's base is the built-in type its definition names; `anySimpleType` is derived from `anyType`.
    for index in 1..schema.types.len() {
        let base = match schema.types.get(index).map(|t| &t.kind) {
            Some(TypeKind::Simple(arc)) => match arc.base() {
                Some(base) => {
                    base.name().and_then(|n| schema.global_types.get(&QName { namespace: n.namespace.clone(), local: n.local.clone() })).copied()
                }
                None => Some(TypeId(0)),
            },
            _ => None,
        };
        if let Some(entry) = schema.types.get_mut(index) {
            entry.base = base;
            entry.derivation = Some(DerivationMethod::Restriction);
        }
    }
    schema.builtin_count = schema.types.len();
    schema
}
