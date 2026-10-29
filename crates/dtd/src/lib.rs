#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-dtd: reading a DTD.
//!
//! - **Layer:** L3, no UI and no I/O (the external parts of a DTD come from a [`SubsetLoader`] the caller gives, or from the resolver
//!   through [`ResolvingSubsetLoader`]), so it compiles for the web too.
//! - [`parse_doctype`] reads the internal subset of a document and the external subset it names: element, attribute-list, entity and
//!   notation declarations, comments and processing instructions, conditional sections, and the parameter entities that carry any of
//!   these, expanded as XML 1.0 §4.4 says. Every syntax error is reported with a [`Code`](xmlcraft_syntax::Code) and where it is, and
//!   reading goes on after it, so that one mistake is not the only one shown.
//! - What is read is a [`Dtd`]: the declarations as written. Building automata from the content models and validating are other crates'
//!   work.
//! - [`check_references`] applies the well-formedness rules that need the declarations to the references to general entities in a
//!   document and in the default values of its attributes: declared, parsed, not external inside an attribute value, no `<` in an
//!   attribute value, not recursive, and a replacement text that is well-formed where it is used. Nothing is expanded: each entity is
//!   analysed once, so an entity bomb costs no more than the declarations it is made of.
//! - Everything is bounded: the depth of parameter-entity nesting, the characters an expansion may produce, the number of external
//!   texts loaded and the number of declarations.

mod attribute;
mod compile;
mod doctype;
mod model;
mod namespaces;
mod parse;
mod references;
mod resolving;

pub use attribute::{MAX_VALUE_BYTES, MAX_VALUE_DEPTH, ValueProblem, ValueReader, check_value};
pub use compile::{Compiled, compile};
pub use doctype::{Checked, DoctypeParts, check_document, doctype_parts, problems_in_text, problems_of_document, read_doctype};
pub use model::{
    AttDef, AttType, AttlistDecl, ContentSpec, Default, Dtd, ElementDecl, EntityDecl, EntityValue, ExternalId, NotationDecl, Occurs, Particle,
    ParticleKind, Place,
};
pub use parse::{
    DoctypeInput, DtdDiagnostic, ExternalRef, LoadRequest, Loaded, MAX_DECLARATIONS, MAX_DIAGNOSTICS, MAX_EXPANDED_BYTES, MAX_GROUP_DEPTH,
    MAX_LOADED_BYTES, MAX_LOADS, MAX_PE_DEPTH, NoLoader, Parsed, SubsetLoader, parse_doctype, parse_external_subset, parse_internal_subset,
};
pub use references::{check_references, is_standalone};
pub use resolving::{MAX_EXTERNAL_BYTES, ResolvingSubsetLoader};
