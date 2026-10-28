#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-validate: a document checked against the grammar its DTD declares, or against an XML Schema.
//!
//! - **Layer:** L4, no UI and no I/O, so it compiles for the web too.
//! - [`Validator`] is a push validator: told the elements, text and other content of a document in order, it checks the validity
//!   constraints of XML 1.0 (root element type, element valid: declared, EMPTY, ANY, mixed and element content through the automata of
//!   `xmlcraft-grammar`; attributes declared, required, fixed, of the right type; ID, IDREF and ENTITY values; standalone document
//!   declaration). It knows nothing of trees or files.
//! - [`validate_document`] drives it over a syntax tree, replacing the references to internal entities in content by their text (read
//!   once, walked each time, with limits on depth and on the number of events, so an entity bomb costs a bounded amount).
//!   External entities cannot be read here; their content is taken as unknown.
//! - [`XsdValidator`] is the same for an assembled XML Schema (`xmlcraft-xsd`): the declaration of the root and of each child through
//!   the compiled content models, the type of an element or the `xsi:type` it names, `xsi:nil`, attributes, simple values through
//!   `xmlcraft-datatypes`, fixed values, and IDs and ID references. [`validate_against_schema`] drives it over a syntax tree.

mod check;
mod document;
mod validator;
mod xsd_document;
mod xsd_validator;

pub use check::{Problems, check};
pub use document::{Limits, MAX_ENTITY_DEPTH, MAX_EVENTS, MAX_OPEN, validate_document, validate_document_with};
pub use validator::{Attribute, Validator};
pub use xsd_document::validate_against_schema;
pub use xsd_validator::{MAX_OPEN as XSD_MAX_OPEN, XSI_NAMESPACE, XsdAttribute, XsdValidator};
