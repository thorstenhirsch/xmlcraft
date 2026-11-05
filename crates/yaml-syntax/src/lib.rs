#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! YAML as XmlCraft reads it.
//!
//! The text is the model: nothing here rewrites a document. [`parse`] reads a text, even a broken one, into the value tree of
//! `xmlcraft-json-syntax` (so schemas, conversion and the outline treat YAML and JSON alike), with the byte span of every value and
//! every key, and the problems as [`Diagnostic`]s; [`highlight_line`] classifies one line for colouring, carrying the little state a line
//! needs (an open block scalar or a quoted scalar that goes on) to the next.
//!
//! Scalars are resolved with the YAML 1.2 core schema (`true`, `null`, `0x1F`, `.inf` …); aliases are expanded into copies that are
//! all placed at the alias, and the expansion is bounded ([`Options::max_nodes`]); `<<` merge keys are applied. The reader never panics
//! and bounds its work: nesting and the number of problems are cut off, with a problem that says so.

mod highlight;
mod parser;

pub use highlight::{Mode, Span, highlight_line};
pub use parser::{Options, Parse, parse};
pub use xmlcraft_json_syntax::{Data, JsonClass, Member, Value};
pub use xmlcraft_syntax::{Code, Diagnostic, Severity};
