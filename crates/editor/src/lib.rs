#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-editor: the editor's model, without any user interface.
//!
//! - **Layer:** L5. It builds on [`xmlcraft_text`] (the buffer, edits and undo) and nothing graphical, so it compiles for the web and
//!   every command can be tested by calling it.
//! - [`Editor`] owns a buffer, its undo history and one or more [`Caret`]s. Every command (typing, deleting, moving, indenting, cut and
//!   paste, undo) acts on all carets at once; those that change text are one undoable step.
//! - [`motion`] has the pure movement functions: by character cluster, by word, by line, by display column.
//!
//! The view (`xmlcraft-code-editor`) turns key presses and mouse gestures into calls on the editor and draws what it holds.

mod caret;
mod editor;
pub mod find;
pub mod motion;

pub use caret::Caret;
pub use editor::{Editor, Motion, Settings};
