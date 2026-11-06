#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-html: HTML5 as an editor needs to read it.
//!
//! - **Layer:** standalone (L1), no UI and no I/O, so it compiles for the web too. It depends on `xmlcraft-syntax` for the shape of a
//!   problem ([`Diagnostic`]).
//! - [`parse`] reads a text the way a browser would build its tree, as far as the structure goes: void elements have no end tag,
//!   the end tags that HTML lets a person leave out (`</p>`, `</li>`, `</td>`…) are implied, `script`, `style`, `textarea` and `title`
//!   hold text and not markup, `svg` and `math` allow `<path/>`. Every element and attribute has its place in the text. It never
//!   rejects a text: what is wrong is a [`Diagnostic`], and the tree is as good as it can be.
//! - [`data`] is what HTML5 is made of: the elements, the attributes each takes and the values some list (for completion and checks).
//! - The reader is bounded: elements, nesting, attributes and problems are capped, and a result that was cut says so.

pub mod data;
mod parse;

pub use parse::{Attribute, Close, Document, Element, MAX_DEPTH, MAX_ELEMENTS, parse};
pub use xmlcraft_syntax::{Code, Diagnostic, Severity};
