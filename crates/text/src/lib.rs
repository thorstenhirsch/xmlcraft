#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-text: the text of a document.
//!
//! - **Layer:** L0, standalone, no UI and no I/O, so it compiles for the web too.
//! - [`Buffer`] is the text itself, kept in a rope (cheap to clone, so a snapshot can go to a worker thread). It is always UTF-8.
//!   Offsets are **byte offsets** into that UTF-8; columns are counted in characters.
//! - [`Edit`]s change it; every edit is checked (a range outside the text, or inside a character, is an [`TextError`], never a
//!   panic) and returns its own inverse, which is how [`History`] undoes.
//!
//! A line ends at `\n`; a `\r` before it is part of the line ending, not of the line (so `\r\n` files work). A file with only `\r`
//! line endings (classic Mac OS) is therefore one long line; the app will be able to offer to convert it.

mod buffer;
mod history;

pub use buffer::{Buffer, Edit, Position, TextError};
pub use history::{EditKind, History, Selection};
