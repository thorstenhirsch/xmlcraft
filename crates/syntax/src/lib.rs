#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-syntax: the XML syntax layer.
//!
//! - **Layer:** L1, no UI and no I/O, so it compiles for the web too.
//! - [`Lexer`] cuts text into [`Token`]s. It is **lossless** (the tokens tile the text exactly), **total** (any text, however broken,
//!   gives tokens; it never fails and never panics), and **restartable** (its whole state is a [`Mode`], so lexing can resume at any
//!   token boundary, which is how the editor colours only the lines on screen).
//! - Character classes from XML 1.0 (5th edition) are in [`is_name_start_char`], [`is_name_char`], [`is_xml_char`] and
//!   [`is_xml_space`].
//!
//! The lexer reports no errors itself: a construct that is not well-formed becomes a token of an error kind ([`TokenKind::BareLt`],
//! [`TokenKind::BareAmp`], [`TokenKind::Error`]) or lacks its closing token (an unterminated comment has no
//! [`TokenKind::CommentEnd`]), and the parser turns that into diagnostics.

mod chars;
mod lexer;

pub use chars::{is_name_char, is_name_start_char, is_xml_char, is_xml_space};
pub use lexer::{Checkpoint, Lexer, Mode, Quote, Return, Token, TokenKind};
