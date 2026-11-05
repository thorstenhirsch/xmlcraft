#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! JSON as XmlCraft reads it: JSON, JSON with comments (JSONC), JSON5 and JSON Lines.
//!
//! The text is the model: nothing here rewrites a document. [`parse`] reads a text, even a broken one, into a tree of [`Value`]s that
//! knows where each one is (byte ranges of the text) and the problems it found as [`Diagnostic`]s; [`highlight_line`] classifies one
//! line of text for colouring, carrying the little state a line needs (a block comment that is still open) to the next.
//!
//! The reader never panics and bounds its work: nesting deeper than [`Options::max_depth`] and more than [`Options::max_diagnostics`]
//! problems are cut off, with a problem that says so.

mod highlight;
mod lexer;
mod parser;
mod tokens;
mod value;

pub use highlight::{JsonClass, Mode, Span, highlight_line};
pub use parser::{Dialect, Options, Parse, parse, parse_lines};
pub use tokens::{Token, TokenKind, tokens};
pub use value::{Data, Member, Segment, Value};
pub use xmlcraft_syntax::{Code, Diagnostic, Severity};
