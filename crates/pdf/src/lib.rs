#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! A minimal PDF reader for XmlCraft: the structure of a file (cross-references, indirect objects, object streams), the stream filters
//! it needs, and the files embedded in it, so that the XML an invoice PDF carries can be read.
//!
//! - **Layer:** L1, no UI and no I/O, so it compiles for the web.
//! - A PDF is never rewritten here: reading takes the bytes and gives values. (Changing the embedded XML is an *incremental update*
//!   appended after the original bytes, which leaves every byte of the file as it was.)
//! - Files that are encrypted are refused, as are filters and predictors beyond what the structure needs. A file whose cross-reference
//!   is damaged is read by scanning it for objects, and says so (`Pdf::recovered`).
//! - Every number, length and offset in the file is untrusted: reading is total, bounded in depth, objects and decoded size, and
//!   never panics.

mod document;
mod embedded;
mod filter;
mod object;

pub use document::Pdf;
pub use embedded::{EmbeddedFile, text_string};
pub use object::{Dict, ObjRef, Object, Parser, Stream};

/// Why a PDF was not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PdfError {
    /// Not a PDF at all (no `%PDF-` header).
    NotAPdf,
    /// Its objects are encrypted.
    Encrypted,
    /// Something this reader does not do (a filter, a predictor).
    Unsupported(String),
    /// The file is damaged in a way that could not be worked around.
    Corrupt(String),
    /// The file may be valid, but is larger or more complex than what is read (too many objects, a stream that decodes to too much).
    LimitExceeded(String),
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PdfError::NotAPdf => f.write_str("this is not a PDF file"),
            PdfError::Encrypted => f.write_str("this PDF is encrypted, and XmlCraft does not read or change encrypted PDFs"),
            PdfError::Unsupported(what) => write!(f, "not supported yet: {what}"),
            PdfError::Corrupt(what) => write!(f, "the PDF is damaged: {what}"),
            PdfError::LimitExceeded(what) => write!(f, "the PDF is larger or more complex than XmlCraft reads: {what}"),
        }
    }
}

impl std::error::Error for PdfError {}
