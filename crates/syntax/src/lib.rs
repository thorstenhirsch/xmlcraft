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
//! - [`parse`] builds a lossless, error-tolerant syntax tree (`rowan`) whose text is exactly the input, and the [`Diagnostic`]s for
//!   everything that is not well-formed XML 1.0 or not valid under Namespaces in XML 1.0. Whatever the input, it gives a tree and never
//!   fails: broken markup is recovered from, and what was recovered is reported.
//!
//! The lexer reports no errors itself: a construct that is not well-formed becomes a token of an error kind ([`TokenKind::BareLt`],
//! [`TokenKind::BareAmp`], [`TokenKind::Error`]) or lacks its closing token (an unterminated comment has no
//! [`TokenKind::CommentEnd`]), and the parser turns that into diagnostics.

mod chars;
mod diagnostic;
mod kinds;
mod lexer;
mod ns;
mod parser;

pub use chars::{is_name_char, is_name_start_char, is_xml_char, is_xml_space};
pub use diagnostic::{Code, Diagnostic, Severity};
pub use kinds::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken, XmlLanguage};
pub use lexer::{Checkpoint, Lexer, Mode, Quote, Return, Token, TokenKind};
pub use ns::{NsScope, XML_NAMESPACE, XMLNS_NAMESPACE, split_qname};
pub use parser::{Limits, Parse, parse, parse_with};
