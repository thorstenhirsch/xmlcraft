//! The declarations a document is checked against: elements with their content and attributes, entities and notations, each with where
//! it was written. A DTD and an XML Schema both compile into this.

use std::collections::HashMap;
use std::ops::Range;

use crate::automaton::Automaton;
use crate::name::Name;
use crate::particle::Particle;

/// The token of a particle that stands for an element no declaration was found for.
pub const NO_DECLARATION: usize = usize::MAX;

/// Where a declaration was written: which text (0 is the document, the others are the external texts that were read, numbered as the
/// reader numbered them) and the byte range in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    pub source: usize,
    pub range: Range<usize>,
}

/// What the grammar was made from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaKind {
    Dtd,
    Xsd,
}

/// The type of an attribute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttributeType {
    Cdata,
    Id,
    Idref,
    Idrefs,
    Entity,
    Entities,
    Nmtoken,
    Nmtokens,
    /// `NOTATION (a | b)`: the names of the notations.
    Notation(Vec<String>),
    /// `(a | b)`: the values allowed.
    Enumeration(Vec<String>),
}

/// What an attribute is when the element does not give it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttributeDefault {
    /// It must be given.
    Required,
    /// It may be left out, with no value then.
    Implied,
    /// `#FIXED "v"`: when given, it must be this; when left out, it has this value.
    Fixed(String),
    /// `"v"`: when left out, it has this value.
    Default(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttributeDecl {
    pub name: Name,
    pub ty: AttributeType,
    pub default: AttributeDefault,
    pub origin: Origin,
}

/// What an element may contain.
#[derive(Clone, Debug)]
pub enum Content {
    /// `EMPTY`: nothing at all.
    Empty,
    /// `ANY`: text and any declared elements.
    Any,
    /// `(#PCDATA | a | b)*` and `(#PCDATA)`: text, and the elements listed, in any order and number.
    Mixed(Vec<Name>),
    /// Elements only, in the order the model says (white space between them is not content).
    Elements(Box<ElementContent>),
}

/// A content model of elements only: as it was written, and compiled.
#[derive(Clone, Debug)]
pub struct ElementContent {
    pub particle: Particle,
    pub automaton: Automaton,
}

#[derive(Clone, Debug)]
pub struct ElementDecl {
    pub name: Name,
    pub content: Content,
    pub attributes: Vec<AttributeDecl>,
    pub origin: Origin,
}

impl ElementDecl {
    pub fn attribute(&self, name: &Name) -> Option<&AttributeDecl> {
        self.attributes.iter().find(|a| &a.name == name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntityKind {
    /// Parsed, with its replacement text (character references and parameter-entity references resolved, general-entity references
    /// as written).
    Internal(String),
    /// Parsed, in a file.
    External { public: Option<String>, system: Option<String> },
    /// In a file that is not XML (`NDATA`), of a kind a notation names.
    Unparsed { public: Option<String>, system: Option<String>, notation: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityDecl {
    pub name: String,
    pub kind: EntityKind,
    pub origin: Origin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotationDecl {
    pub name: String,
    pub public: Option<String>,
    pub system: Option<String>,
    pub origin: Origin,
}

/// Everything declared: the elements (the first declaration of a name is the one that counts), the general entities and the notations.
#[derive(Clone, Debug)]
pub struct Grammar {
    kind: SchemaKind,
    /// The name the DOCTYPE gives the root element, when there is one.
    pub root: Option<Name>,
    /// Some part of the DTD could not be read: a name that is not declared may be declared there.
    pub incomplete: bool,
    elements: Vec<ElementDecl>,
    element_index: HashMap<Name, usize>,
    entities: Vec<EntityDecl>,
    entity_index: HashMap<String, usize>,
    notations: Vec<NotationDecl>,
    notation_index: HashMap<String, usize>,
}

impl Grammar {
    pub fn new(kind: SchemaKind) -> Self {
        Self {
            kind,
            root: None,
            incomplete: false,
            elements: Vec::new(),
            element_index: HashMap::new(),
            entities: Vec::new(),
            entity_index: HashMap::new(),
            notations: Vec::new(),
            notation_index: HashMap::new(),
        }
    }

    pub fn kind(&self) -> SchemaKind {
        self.kind
    }

    /// Add an element; `false` (and nothing added) when the name is declared already.
    pub fn add_element(&mut self, decl: ElementDecl) -> bool {
        if self.element_index.contains_key(&decl.name) {
            return false;
        }
        self.element_index.insert(decl.name.clone(), self.elements.len());
        self.elements.push(decl);
        true
    }

    /// Add a general entity; `false` when the name is declared already.
    pub fn add_entity(&mut self, decl: EntityDecl) -> bool {
        if self.entity_index.contains_key(&decl.name) {
            return false;
        }
        self.entity_index.insert(decl.name.clone(), self.entities.len());
        self.entities.push(decl);
        true
    }

    /// Add a notation; `false` when the name is declared already.
    pub fn add_notation(&mut self, decl: NotationDecl) -> bool {
        if self.notation_index.contains_key(&decl.name) {
            return false;
        }
        self.notation_index.insert(decl.name.clone(), self.notations.len());
        self.notations.push(decl);
        true
    }

    pub fn elements(&self) -> &[ElementDecl] {
        &self.elements
    }

    pub fn element(&self, name: &Name) -> Option<&ElementDecl> {
        self.element_index.get(name).and_then(|&i| self.elements.get(i))
    }

    pub fn element_mut(&mut self, name: &Name) -> Option<&mut ElementDecl> {
        let i = *self.element_index.get(name)?;
        self.elements.get_mut(i)
    }

    /// The position of an element declaration (what the tokens of the particles that stand for it are).
    pub fn element_index(&self, name: &Name) -> Option<usize> {
        self.element_index.get(name).copied()
    }

    pub fn entities(&self) -> &[EntityDecl] {
        &self.entities
    }

    pub fn entity(&self, name: &str) -> Option<&EntityDecl> {
        self.entity_index.get(name).and_then(|&i| self.entities.get(i))
    }

    pub fn notations(&self) -> &[NotationDecl] {
        &self.notations
    }

    pub fn notation(&self, name: &str) -> Option<&NotationDecl> {
        self.notation_index.get(name).and_then(|&i| self.notations.get(i))
    }
}
