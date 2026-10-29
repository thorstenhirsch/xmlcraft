#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-sample: an XML document generated from an XML Schema or a DTD.
//!
//! - **Layer:** L4, no UI and no I/O, so it compiles for the web too.
//! - [`sample_from_schema`] makes a document from an assembled schema ([`xmlcraft_xsd::schema::Schema`]) and [`sample_from_dtd`] from a
//!   [`Dtd`]. The root is the element asked for, or the first that can be made.
//! - The document follows the content models: a sequence is each of its parts, a choice is one branch ([`ChoicePolicy`]), a particle occurs
//!   [`Options::repeat`] times kept between its least and greatest number, optional elements and attributes are left out or put in
//!   ([`Options::optional`]). A recursive schema is cut at [`Options::max_depth`]: below it only what is required is made, of a choice the
//!   branch that ends soonest.
//! - Values come from a fixed value, then a default, then the enumeration of the type, then a string that matches its patterns, then a
//!   value of the kind the type is; each is checked by the type itself, so a value that is written is a valid one ([`value`]). When none
//!   fits, a warning says so.
//! - Abstract elements are replaced by a member of their substitution group, abstract types by a type derived from them (`xsi:type`).

mod dtd;
mod tree;
mod value;
mod xsd;

pub use value::Fill;
pub use xmlcraft_dtd::Dtd;
pub use xmlcraft_xsd::model::QName;
pub use xmlcraft_xsd::schema::Schema;

use std::fmt;

/// Which branch of a choice is made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChoicePolicy {
    /// The first branch that can be made.
    #[default]
    First,
    /// The branches in turn, each time a choice is met.
    Rotate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// Make the optional elements and attributes (otherwise only what is required).
    pub optional: bool,
    /// How often a particle that may occur more than once occurs (kept between its least and greatest number).
    pub repeat: usize,
    pub choice: ChoicePolicy,
    /// How the text of elements and attributes is filled in.
    pub fill: Fill,
    /// How deep the elements nest before only what is required is made.
    pub max_depth: usize,
    /// Where the schema is, to say so in the document (`xsi:schemaLocation`, or `xsi:noNamespaceSchemaLocation`).
    pub schema_location: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self { optional: true, repeat: 1, choice: ChoicePolicy::First, fill: Fill::Typed, max_depth: 6, schema_location: None }
    }
}

/// A document, and what could not be made as asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    pub text: String,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SampleError {
    /// The schema declares no element a document could start with.
    NoRoot,
    /// The element asked for is not declared.
    UnknownRoot(String),
    /// No finite document can be made for the element (it requires itself, or something that cannot be made).
    Unsatisfiable(String),
    /// The document would nest deeper than is made.
    TooDeep,
    /// The document would have more elements than is made.
    TooLarge,
}

impl fmt::Display for SampleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SampleError::NoRoot => write!(f, "The schema declares no element a document can start with."),
            SampleError::UnknownRoot(name) => write!(f, "The element '{name}' is not declared."),
            SampleError::Unsatisfiable(name) => write!(f, "No finite document can be made for '{name}'."),
            SampleError::TooDeep => write!(f, "The document would nest too deeply."),
            SampleError::TooLarge => write!(f, "The document would be too large."),
        }
    }
}

impl std::error::Error for SampleError {}

/// The names of the global elements of `schema` a document may start with.
pub fn schema_roots(schema: &Schema) -> Vec<QName> {
    xsd::roots(schema)
}

/// A document from `schema`, starting with `root` (or the first element that can be made).
pub fn sample_from_schema(schema: &Schema, root: Option<&QName>, options: &Options) -> Result<Sample, SampleError> {
    xsd::sample(schema, root, options)
}

/// The names of the elements `dtd` declares.
pub fn dtd_roots(dtd: &Dtd) -> Vec<String> {
    dtd::roots(dtd)
}

/// A document from `dtd`, starting with `root` (or the first element that can be made).
pub fn sample_from_dtd(dtd: &Dtd, root: Option<&str>, options: &Options) -> Result<Sample, SampleError> {
    dtd::sample(dtd, root, options)
}
