#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-dtd: reading a DTD.
//!
//! - **Layer:** L3, no UI and no I/O (the external parts of a DTD come from a [`SubsetLoader`] the caller gives), so it compiles for the
//!   web too.
//! - [`parse_doctype`] reads the internal subset of a document and the external subset it names: element, attribute-list, entity and
//!   notation declarations, comments and processing instructions, conditional sections, and the parameter entities that carry any of
//!   these, expanded as XML 1.0 §4.4 says. Every syntax error is reported with a [`Code`](xmlcraft_syntax::Code) and where it is, and
//!   reading goes on after it, so that one mistake is not the only one shown.
//! - What is read is a [`Dtd`]: the declarations as written. Building automata from the content models, validating, and the rules about
//!   entity references in content are other crates' work.
//! - Everything is bounded: the depth of parameter-entity nesting, the characters an expansion may produce, the number of external
//!   texts loaded and the number of declarations.

mod doctype;
mod model;
mod parse;

pub use doctype::{DoctypeParts, doctype_parts, problems_in_text, problems_of_document};
pub use model::{
    AttDef, AttType, AttlistDecl, ContentSpec, Default, Dtd, ElementDecl, EntityDecl, EntityValue, ExternalId, NotationDecl, Occurs, Particle,
    ParticleKind, Place,
};
pub use parse::{
    DoctypeInput, DtdDiagnostic, ExternalRef, LoadRequest, Loaded, MAX_DECLARATIONS, MAX_DIAGNOSTICS, MAX_EXPANDED_BYTES, MAX_GROUP_DEPTH,
    MAX_LOADED_BYTES, MAX_LOADS, MAX_PE_DEPTH, NoLoader, Parsed, SubsetLoader, parse_doctype, parse_external_subset, parse_internal_subset,
};
