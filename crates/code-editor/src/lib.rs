#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-code-editor: the text editor widget for egui.
//!
//! - **Layer:** L7. It draws an [`xmlcraft_editor::Editor`] and turns keys, the mouse, the clipboard and input methods into calls on it.
//!   It knows no theme: the host passes [`EditorColors`].
//! - **Virtualised:** only the lines on screen are laid out, and a line longer than a few thousand characters is laid out in a window
//!   around what is visible, so a document of any length scrolls at the same speed.
//! - [`ViewState`] holds what the view keeps between frames (layouts, the composition being typed, scroll requests); the host keeps one
//!   next to each [`xmlcraft_editor::Editor`].

mod colors;
mod keys;
mod layout;
mod view;

pub use colors::EditorColors;
pub use keys::{Command, command_for};
pub use layout::{LineLayout, LineStyler, MAX_WINDOW, StyleSpan};
pub use view::{CodeEditor, CodeEditorOutput, Geometry, ViewState};
