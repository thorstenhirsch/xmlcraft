//! What a DTD declares: the declarations as they were read, with where each one is.

use std::ops::Range;

/// Where something is: which text (0 is the document's own internal subset; the others are the external texts that were read, in the
/// order they were loaded) and the byte range in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub source: usize,
    pub range: Range<usize>,
}

/// How often a content particle may occur.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Occurs {
    Once,
    /// `?`
    Optional,
    /// `*`
    ZeroOrMore,
    /// `+`
    OneOrMore,
}

/// A content particle of a children content model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Particle {
    pub kind: ParticleKind,
    pub occurs: Occurs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParticleKind {
    Name(String),
    /// `(a | b | c)`
    Choice(Vec<Particle>),
    /// `(a , b , c)` (a group of one particle is a sequence)
    Sequence(Vec<Particle>),
}

/// What an element may contain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentSpec {
    Empty,
    Any,
    /// `(#PCDATA | a | b)*`, or `(#PCDATA)`: the names, which may repeat in what was written.
    Mixed(Vec<String>),
    Children(Particle),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ElementDecl {
    pub name: String,
    pub spec: ContentSpec,
    pub place: Place,
}

/// The type of an attribute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttType {
    Cdata,
    Id,
    Idref,
    Idrefs,
    Entity,
    Entities,
    Nmtoken,
    Nmtokens,
    /// `NOTATION (a | b)`
    Notation(Vec<String>),
    /// `(a | b)`
    Enumeration(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Default {
    Required,
    Implied,
    /// `#FIXED "value"`: the value as written, its references not yet resolved.
    Fixed(String),
    /// `"value"`
    Value(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttDef {
    pub name: String,
    pub ty: AttType,
    pub default: Default,
    pub place: Place,
}

/// The attributes one `<!ATTLIST>` declares for an element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttlistDecl {
    pub element: String,
    pub defs: Vec<AttDef>,
    pub place: Place,
}

/// What an entity stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntityValue {
    /// An internal entity: its replacement text, with character references and parameter-entity references resolved and general-entity
    /// references left as they were written.
    Internal(String),
    External(ExternalId),
}

/// `SYSTEM "..."` or `PUBLIC "..." "..."`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalId {
    pub public: Option<String>,
    pub system: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityDecl {
    pub name: String,
    pub value: EntityValue,
    /// `NDATA name`: the entity is unparsed.
    pub notation: Option<String>,
    /// Where the declaration is, and the location (a URI or a path) of the text it was read from, when that is not the document.
    pub place: Place,
    pub location: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotationDecl {
    pub name: String,
    pub id: ExternalId,
    pub place: Place,
}

/// Everything a DTD declares. Where a name is declared more than once, the first declaration is the one that counts (for entities and
/// for the attributes of an element, as XML 1.0 says); the later ones are kept in the lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dtd {
    pub elements: Vec<ElementDecl>,
    pub attlists: Vec<AttlistDecl>,
    pub general_entities: Vec<EntityDecl>,
    pub parameter_entities: Vec<EntityDecl>,
    pub notations: Vec<NotationDecl>,
    /// The DTD refers to something outside the document that was not read (an external subset, or an external parameter entity): what is
    /// declared there is not known, so a name that is not declared is not necessarily an error.
    pub incomplete: bool,
    /// There is an external subset, or the internal subset has a parameter-entity reference (XML 1.0 §4.1, "Entity Declared").
    pub has_external_parts: bool,
}

impl Dtd {
    /// The general entity called `name` (the first declaration).
    pub fn general_entity(&self, name: &str) -> Option<&EntityDecl> {
        self.general_entities.iter().find(|e| e.name == name)
    }

    /// The parameter entity called `name` (the first declaration).
    pub fn parameter_entity(&self, name: &str) -> Option<&EntityDecl> {
        self.parameter_entities.iter().find(|e| e.name == name)
    }

    /// The first declaration of the element `name`.
    pub fn element(&self, name: &str) -> Option<&ElementDecl> {
        self.elements.iter().find(|e| e.name == name)
    }

    /// The attribute `attribute` of `element`, from the first declaration of it.
    pub fn attribute(&self, element: &str, attribute: &str) -> Option<&AttDef> {
        self.attlists.iter().filter(|a| a.element == element).flat_map(|a| a.defs.iter()).find(|d| d.name == attribute)
    }
}
