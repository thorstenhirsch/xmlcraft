#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! JSON Schema for XmlCraft: drafts 04, 06, 07, 2019-09 and 2020-12.
//!
//! - **Layer:** L3, no UI and no I/O, so it compiles for the web too. Schemas that refer to other documents get them through a
//!   [`Loader`]; nothing is fetched here.
//! - A [`SchemaSet`] is the schema to validate with and the documents it refers to, indexed by `$id`, `$anchor` and `$dynamicAnchor`.
//!   [`SchemaSet::validate`] checks an instance (a `xmlcraft_json_syntax::Value`) and returns [`Violation`]s whose `instance` pointer can be
//!   turned into a place in the text with `Value::span_of_pointer`.
//! - Never panics and bounds its work: nesting, the number of evaluations, the size of a pattern and the number of violations are capped,
//!   and a result that was cut off says so.

mod draft;
mod number;
mod schema;
mod uri;
mod validate;

pub use draft::Draft;
pub use schema::{Loader, MAX_DOCUMENTS, NoLoader, SchemaProblem, SchemaSet};
pub use validate::{MAX_VIOLATIONS, Outcome, Violation};
