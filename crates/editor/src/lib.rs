#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-editor: the editor's model, without any user interface.
//!
//! - **Layer:** L5. It builds on [`xmlcraft_text`] (the buffer, edits and undo) and nothing graphical, so it compiles for the web and
//!   every command can be tested by calling it.
//! - [`Editor`] owns a buffer, its undo history and one or more [`Caret`]s. Every command (typing, deleting, moving, indenting, cut and
//!   paste, undo) acts on all carets at once; those that change text are one undoable step.
//! - [`Editor::type_input`] is what typing a key does when the editor knows XML: the quotes of an attribute value are paired and stepped
//!   over (and a Backspace between an empty pair removes both). Its tags and quotes are read with the lexer of `xmlcraft-syntax` (L1).
//! - Folds hide stretches of lines ([`Fold`]); the view works in rows ([`Editor::visible_rows`], [`Editor::row_of_line`]), and carets,
//!   movement and edits keep to the folds.
//! - [`Editor::toggle_comment`] comments lines out and in; [`Editor::expand_selection`] and [`Editor::shrink_selection`] grow and shrink a
//!   selection by what the syntax tree says (the word, the value, the attribute, the tag, the content, the element, the one around it).
//! - [`Editor::link_names`] keeps the names of a start tag and its end tag in step: typing in one is written into the other.
//! - [`motion`] has the pure movement functions: by character cluster, by word, by line, by display column.
//!
//! The view (`xmlcraft-code-editor`) turns key presses and mouse gestures into calls on the editor and draws what it holds.

mod caret;
mod editor;
pub mod find;
mod folds;
mod linked;
pub mod motion;
mod structure;
mod typing;

pub use caret::Caret;
pub use editor::{Editor, Motion, Settings};
pub use folds::{Fold, FoldMap};
pub use structure::CommentRefused;
