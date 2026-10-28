#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-validate: a document checked against the grammar its DTD declares.
//!
//! - **Layer:** L4, no UI and no I/O, so it compiles for the web too.
//! - [`Validator`] is a push validator: told the elements, text and other content of a document in order, it checks the validity
//!   constraints of XML 1.0 (root element type, element valid: declared, EMPTY, ANY, mixed and element content through the automata of
//!   `xmlcraft-grammar`; attributes declared, required, fixed, of the right type; ID, IDREF and ENTITY values; standalone document
//!   declaration). It knows nothing of trees or files.
//! - [`validate_document`] drives it over a syntax tree, replacing the references to internal entities in content by their text (read
//!   once, walked each time, with limits on depth and on the number of events, so an entity bomb costs a bounded amount).
//!   External entities cannot be read here; their content is taken as unknown.

mod check;
mod document;
mod validator;

pub use check::{Problems, check};
pub use document::{Limits, MAX_ENTITY_DEPTH, MAX_EVENTS, MAX_OPEN, validate_document, validate_document_with};
pub use validator::{Attribute, Validator};
