//! A schema document as it is written: the declarations and definitions of one `xs:schema` element, with where each was written.
//!
//! This is the document, not the schema: references are names (`QName`s with their namespaces resolved) that nothing has looked up yet,
//! and anything that could not be read is left out (and was reported). Putting the documents of a schema together, looking the names
//! up and checking the constraints between components is the next step.

use std::ops::Range;

pub type Span = Range<usize>;

/// An expanded name: a namespace name (none for no namespace) and a local part.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct QName {
    pub namespace: Option<String>,
    pub local: String,
}

impl QName {
    pub fn new(namespace: Option<&str>, local: &str) -> Self {
        Self { namespace: namespace.map(str::to_owned), local: local.to_owned() }
    }
}

impl std::fmt::Display for QName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.namespace {
            Some(ns) => write!(f, "{{{ns}}}{}", self.local),
            None => f.write_str(&self.local),
        }
    }
}

/// A reference to a component by name, with where the name was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QRef {
    pub name: QName,
    pub span: Span,
}

/// The namespace declarations in scope where a value was written, for the values that hold qualified names (the enumeration of a
/// `QName` type, a default).
pub type Namespaces = Vec<(Option<String>, String)>;

/// `qualified` or `unqualified`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    Qualified,
    Unqualified,
}

/// The `block`, `final`, `blockDefault` and `finalDefault` attributes: which derivations (and substitutions) they stop.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DerivationSet {
    pub extension: bool,
    pub restriction: bool,
    pub substitution: bool,
    pub list: bool,
    pub union: bool,
}

impl DerivationSet {
    pub const ALL: Self = Self { extension: true, restriction: true, substitution: true, list: true, union: true };

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Documentation {
    pub source: Option<String>,
    pub lang: Option<String>,
    pub text: String,
}

/// An `xs:annotation`: what is meant for people. (`xs:appinfo` is read for its place only.)
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Annotation {
    pub documentation: Vec<Documentation>,
    pub appinfo: usize,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct SchemaDocument {
    /// Where the document was read from (what `schemaLocation`s are relative to).
    pub location: String,
    pub span: Span,
    pub target_namespace: Option<String>,
    pub element_form_default: Form,
    pub attribute_form_default: Form,
    pub block_default: DerivationSet,
    pub final_default: DerivationSet,
    pub version: Option<String>,
    pub id: Option<String>,
    pub annotations: Vec<Annotation>,
    /// The includes, imports, redefinitions and the top-level declarations, in the order they were written.
    pub items: Vec<Item>,
}

#[derive(Clone, Debug)]
pub enum Item {
    Include(Include),
    Import(Import),
    Redefine(Redefine),
    SimpleType(SimpleTypeDef),
    ComplexType(ComplexTypeDef),
    Group(GroupDef),
    AttributeGroup(AttributeGroupDef),
    Element(ElementDecl),
    Attribute(AttributeDecl),
    Notation(NotationDef),
}

#[derive(Clone, Debug)]
pub struct Include {
    pub schema_location: String,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Import {
    pub namespace: Option<String>,
    pub schema_location: Option<String>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Redefine {
    pub schema_location: String,
    pub items: Vec<Item>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct SimpleTypeDef {
    pub name: Option<String>,
    pub final_: Option<DerivationSet>,
    pub kind: SimpleTypeKind,
    pub annotation: Option<Annotation>,
    pub span: Span,
    pub name_span: Span,
}

#[derive(Clone, Debug)]
pub enum SimpleTypeKind {
    Restriction {
        base: Option<QRef>,
        inline_base: Option<Box<SimpleTypeDef>>,
        facets: Vec<FacetDef>,
    },
    List {
        item_type: Option<QRef>,
        inline_item: Option<Box<SimpleTypeDef>>,
    },
    Union {
        member_types: Vec<QRef>,
        inline_members: Vec<SimpleTypeDef>,
    },
    /// The definition has none of the three (it was reported).
    Missing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FacetKind {
    MinExclusive,
    MinInclusive,
    MaxExclusive,
    MaxInclusive,
    TotalDigits,
    FractionDigits,
    Length,
    MinLength,
    MaxLength,
    Enumeration,
    WhiteSpace,
    Pattern,
}

impl FacetKind {
    pub fn from_local(local: &str) -> Option<Self> {
        Some(match local {
            "minExclusive" => Self::MinExclusive,
            "minInclusive" => Self::MinInclusive,
            "maxExclusive" => Self::MaxExclusive,
            "maxInclusive" => Self::MaxInclusive,
            "totalDigits" => Self::TotalDigits,
            "fractionDigits" => Self::FractionDigits,
            "length" => Self::Length,
            "minLength" => Self::MinLength,
            "maxLength" => Self::MaxLength,
            "enumeration" => Self::Enumeration,
            "whiteSpace" => Self::WhiteSpace,
            "pattern" => Self::Pattern,
            _ => return None,
        })
    }

    pub fn local(self) -> &'static str {
        match self {
            Self::MinExclusive => "minExclusive",
            Self::MinInclusive => "minInclusive",
            Self::MaxExclusive => "maxExclusive",
            Self::MaxInclusive => "maxInclusive",
            Self::TotalDigits => "totalDigits",
            Self::FractionDigits => "fractionDigits",
            Self::Length => "length",
            Self::MinLength => "minLength",
            Self::MaxLength => "maxLength",
            Self::Enumeration => "enumeration",
            Self::WhiteSpace => "whiteSpace",
            Self::Pattern => "pattern",
        }
    }
}

#[derive(Clone, Debug)]
pub struct FacetDef {
    pub kind: FacetKind,
    pub value: String,
    pub fixed: bool,
    /// The namespace declarations at the facet (the value of an enumeration of qualified names is read with them).
    pub namespaces: Namespaces,
    pub span: Span,
    pub value_span: Span,
}

#[derive(Clone, Debug)]
pub struct ComplexTypeDef {
    pub name: Option<String>,
    pub abstract_: Option<bool>,
    pub mixed: Option<bool>,
    pub block: Option<DerivationSet>,
    pub final_: Option<DerivationSet>,
    pub content: ComplexContentDef,
    pub annotation: Option<Annotation>,
    pub span: Span,
    pub name_span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DerivationMethod {
    Restriction,
    Extension,
}

#[derive(Clone, Debug)]
pub enum ComplexContentDef {
    /// A model group and attributes written straight in the type: a restriction of `xs:anyType`.
    Implicit { particle: Option<Particle>, attributes: AttributeUses },
    /// `xs:simpleContent`: a simple type with attributes.
    Simple {
        method: DerivationMethod,
        base: Option<QRef>,
        /// The simple type that a restriction applies the facets to.
        inline_base: Option<Box<SimpleTypeDef>>,
        facets: Vec<FacetDef>,
        attributes: AttributeUses,
    },
    /// `xs:complexContent`: a complex type derived from another.
    Complex { method: DerivationMethod, base: Option<QRef>, mixed: Option<bool>, particle: Option<Particle>, attributes: AttributeUses },
}

/// The attribute declarations, references to attribute groups and the attribute wildcard of a type or an attribute group.
#[derive(Clone, Debug, Default)]
pub struct AttributeUses {
    pub attributes: Vec<AttributeDecl>,
    pub groups: Vec<AttributeGroupRef>,
    pub wildcard: Option<Wildcard>,
}

#[derive(Clone, Debug)]
pub struct AttributeGroupRef {
    pub reference: QRef,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttributeUse {
    Optional,
    Required,
    Prohibited,
}

#[derive(Clone, Debug)]
pub struct ValueConstraint {
    pub value: String,
    pub namespaces: Namespaces,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct AttributeDecl {
    pub name: Option<String>,
    pub reference: Option<QRef>,
    pub type_name: Option<QRef>,
    pub inline_type: Option<Box<SimpleTypeDef>>,
    pub use_: Option<AttributeUse>,
    pub default: Option<ValueConstraint>,
    pub fixed: Option<ValueConstraint>,
    pub form: Option<Form>,
    pub annotation: Option<Annotation>,
    pub span: Span,
    pub name_span: Span,
}

#[derive(Clone, Debug)]
pub struct AttributeGroupDef {
    pub name: Option<String>,
    pub uses: AttributeUses,
    pub annotation: Option<Annotation>,
    pub span: Span,
    pub name_span: Span,
}

/// `maxOccurs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaxOccurs {
    Bounded(u32),
    Unbounded,
}

#[derive(Clone, Debug)]
pub struct Particle {
    pub min_occurs: u32,
    pub max_occurs: MaxOccurs,
    pub term: Term,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Term {
    Element(Box<ElementDecl>),
    GroupRef(QRef),
    Sequence(Vec<Particle>),
    Choice(Vec<Particle>),
    All(Vec<Particle>),
    Any(Wildcard),
}

#[derive(Clone, Debug)]
pub enum InlineType {
    Simple(SimpleTypeDef),
    Complex(ComplexTypeDef),
}

#[derive(Clone, Debug)]
pub struct ElementDecl {
    pub name: Option<String>,
    pub reference: Option<QRef>,
    pub type_name: Option<QRef>,
    pub inline_type: Option<Box<InlineType>>,
    pub substitution_group: Option<QRef>,
    pub default: Option<ValueConstraint>,
    pub fixed: Option<ValueConstraint>,
    pub nillable: Option<bool>,
    pub abstract_: Option<bool>,
    pub block: Option<DerivationSet>,
    pub final_: Option<DerivationSet>,
    pub form: Option<Form>,
    pub identity_constraints: Vec<IdentityConstraintDef>,
    pub annotation: Option<Annotation>,
    pub span: Span,
    pub name_span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityKind {
    Unique,
    Key,
    Keyref,
}

#[derive(Clone, Debug)]
pub struct IdentityConstraintDef {
    pub kind: IdentityKind,
    pub name: Option<String>,
    pub refer: Option<QRef>,
    pub selector: Option<(String, Span)>,
    pub fields: Vec<(String, Span)>,
    /// The namespace declarations at the constraint (the XPath expressions are read with them).
    pub namespaces: Namespaces,
    pub span: Span,
    pub name_span: Span,
}

#[derive(Clone, Debug)]
pub struct GroupDef {
    pub name: Option<String>,
    /// The model group; its occurrence is always one.
    pub content: Option<Particle>,
    pub annotation: Option<Annotation>,
    pub span: Span,
    pub name_span: Span,
}

/// One of the tokens of a wildcard's `namespace` list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamespaceToken {
    Uri(String),
    /// `##local`: no namespace.
    Local,
    /// `##targetNamespace`.
    TargetNamespace,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NamespaceConstraint {
    /// `##any`
    Any,
    /// `##other`: any namespace but the target namespace (and not no namespace).
    Other,
    List(Vec<NamespaceToken>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessContents {
    Strict,
    Lax,
    Skip,
}

#[derive(Clone, Debug)]
pub struct Wildcard {
    pub namespace: NamespaceConstraint,
    pub process_contents: ProcessContents,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct NotationDef {
    pub name: Option<String>,
    pub public: Option<String>,
    pub system: Option<String>,
    pub annotation: Option<Annotation>,
    pub span: Span,
    pub name_span: Span,
}
