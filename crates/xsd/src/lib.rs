#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-xsd: XML Schema 1.0.
//!
//! - **Layer:** L3, no UI and no I/O, so it compiles for the web too.
//! - [`read`] reads one schema document into the model of [`model`]: the declarations and definitions as they are written, with where
//!   each was written, and the mistakes of the document reported (which elements and attributes may be where, the values of the
//!   attributes, which attributes may not be given together). It is total: whatever the text says it gives diagnostics and whatever
//!   could be read.

mod build;
mod compile;
mod dom;
mod loader;
pub mod model;
mod reader;
pub mod schema;
mod values;
pub mod xpath;

pub use build::{Assembly, assemble};
pub use compile::{AllState, ContentModel, ModelState, Next, Symbol};
pub use loader::{
    Documents, LoadKind, LoadRequest, Loaded, LoadedDocument, MAX_DOCUMENTS, MAX_TOTAL_BYTES, NoLoader, SchemaLoader, SourcedDiagnostic, load,
};
pub use reader::{Read, XSD_NAMESPACE, read};
