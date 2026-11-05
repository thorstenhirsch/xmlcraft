#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! BSON for XmlCraft (layer L1, no UI, no I/O, compiles for the web): a codec and a text projection.
//!
//! - [`read_all`] / [`write_all`]: the documents of a `.bson` file and back, with every type of the specification kept so that what is
//!   read is written again byte for byte. Reading is strict about what makes bytes ambiguous and says where; it is bounded in nesting
//!   ([`MAX_DEPTH`]) and never allocates more than the input is long.
//! - [`to_text`] / [`from_text`]: the documents as MongoDB Extended JSON (canonical, so the text turns back into the same bytes; one
//!   indented document, or one document to a line) and back, with the place in the text of every value that is not valid Extended JSON.
//!   This is the text a BSON file is edited as; saving writes the bytes the edited text stands for.

mod base64;
mod codec;
mod decimal;
mod ejson;
mod model;

pub use codec::{MAX_DEPTH, read_all, write_all};
pub use ejson::{Layout, from_text, to_text};
pub use model::{Bson, BsonError, Document};
