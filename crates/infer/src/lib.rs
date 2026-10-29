#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-infer: an XML Schema or a DTD generated from one or more documents.
//!
//! - **Layer:** L4, no UI and no I/O, so it compiles for the web too.
//! - [`infer`] reads the documents (each must be well-formed) and writes the schema: for an XML Schema one document per namespace
//!   in one of three [`Layout`]s, for a DTD one file.
//! - The content model of an element is learned from the order and number of its children ([`content`]): a sequence of choices, each
//!   repeated between a least and a most number of times. It is deterministic, and accepts every element the documents had, so a
//!   schema inferred from a document always validates that document.
//! - The types of attributes and of the text of elements are the narrowest of boolean, integer, decimal, double, date, dateTime and
//!   time that all the values are lexical forms of, else string; a few values, each seen several times, are an enumeration.

mod content;
mod dtd;
mod observe;
mod values;
mod xsd;

use std::fmt;

pub use values::EnumerationRule;

/// How the declarations of an XML Schema are arranged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    /// Each root is a global element and the others are declared inside their parents (those that cannot be, such as an element inside
    /// itself, are global and referred to).
    #[default]
    RussianDoll,
    /// Every element is a global declaration, and every use of it a reference.
    SalamiSlice,
    /// A named global complex type for each element that has attributes or children, and local elements of those types.
    VenetianBlind,
}

/// What is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Xsd { layout: Layout },
    Dtd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub output: Output,
    /// Infer boolean, integer and the other narrow types; otherwise every value is a string.
    pub detect_types: bool,
    /// What makes a set of values an enumeration, or `None` for none.
    pub enumerations: Option<EnumerationRule>,
    /// An element that occurs more than once somewhere may occur any number of times (otherwise at most as many times as the most).
    pub unbounded: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            output: Output::Xsd { layout: Layout::default() },
            detect_types: true,
            enumerations: Some(EnumerationRule::default()),
            unbounded: true,
        }
    }
}

/// One file of the result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedFile {
    /// A file name (`schema.xsd`, `schema.dtd`; with several namespaces one per namespace, which the others import by this name).
    pub name: String,
    /// The target namespace of the schema (always `None` for a DTD).
    pub namespace: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generated {
    pub files: Vec<GeneratedFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InferError {
    /// There was nothing to learn from.
    NoDocuments,
    /// A document is not well-formed (`document` counts from 0, `offset` is a byte offset in it).
    NotWellFormed { document: usize, message: String, offset: usize },
    /// A document has no root element.
    NoRoot,
    /// The documents are larger than one run reads.
    TooLarge,
    /// A document is nested or large beyond what is read.
    Limit(&'static str),
}

impl fmt::Display for InferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InferError::NoDocuments => write!(f, "There is no document to generate a schema from."),
            InferError::NotWellFormed { document, message, .. } => write!(f, "Document {} is not well-formed: {message}", document + 1),
            InferError::NoRoot => write!(f, "A document has no root element."),
            InferError::TooLarge => write!(f, "The documents are too large to generate a schema from."),
            InferError::Limit(why) => write!(f, "A document is too large to generate a schema from: {why}."),
        }
    }
}

impl std::error::Error for InferError {}

/// Generate a schema from `documents`.
pub fn infer(documents: &[&str], options: &Options) -> Result<Generated, InferError> {
    if documents.is_empty() {
        return Err(InferError::NoDocuments);
    }
    let mut observed = observe::Observed::new(options.output);
    for text in documents {
        observed.observe_document(text)?;
    }
    let files = match options.output {
        Output::Xsd { layout } => xsd::write(&observed, options, layout),
        Output::Dtd => vec![dtd::write(&observed, options)],
    };
    Ok(Generated { files })
}
