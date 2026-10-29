#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-complete: what a grammar allows at a place in a document, and the text to put there.
//!
//! - **Layer:** L4, no UI and no I/O, so it compiles for the web too.
//! - [`choices_at`] says, for a position in a parsed document, which elements may come there (and which of them must), which
//!   attributes the element has (and which are already given or required), and which entities can be referred to. The elements are
//!   those the content model of the enclosing element allows after the children that come before the position, so a model is followed
//!   through the siblings; an element that is not declared, or a document with no grammar, offers nothing.
//! - [`snippet`] is the text for one choice: an element with the attributes and the children that its declaration requires (or without),
//!   and where to put the caret.
//! - [`xsd_choices_at`] and [`xsd_element_snippet`] are the same for an XML Schema: the element at the position is typed by walking down
//!   from the root (each child matched to a declaration by the content model of its parent, `xsi:type` honoured), the attributes are
//!   those of its type (with the values an enumeration, a default or a fixed value gives), and so are the values its text may have.
//!   Names are written as the document writes them (the prefix a namespace has at the place, or the declaration it needs).

mod choices;
mod scope;
mod snippet;
mod xsd;
mod xsd_snippet;

pub use choices::{AttributeChoice, Choices, ElementChoice, EntityChoice, choices_at};
pub use scope::{Declaration, Written};
pub use snippet::{MAX_SNIPPET_BYTES, MAX_SNIPPET_DEPTH, Snippet, attribute_snippet, element_snippet, entity_snippet};
pub use xsd::{MAX_ELEMENTS, MAX_VALUES, xsd_choices_at};
pub use xsd_snippet::{xsd_attribute_snippet, xsd_element_snippet};
