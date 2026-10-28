#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-catalog: OASIS XML Catalogs 1.1.
//!
//! - **Layer:** L2, no UI and no I/O, so it compiles for the web too.

mod reader;
mod resolve;
pub mod uri;

pub use reader::{CATALOG_NS, Catalog, CatalogError, Entry, MAX_ENTRIES, MAX_WARNINGS};
pub use resolve::{Catalogs, Loader, MAX_LOADS, NoLoader};
