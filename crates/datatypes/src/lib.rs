#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-datatypes: XML Schema Part 2, the datatypes.
//!
//! - **Layer:** L1, no UI and no I/O, so it compiles for the web too.
//! - The built-in types ([`SimpleType::builtin`]): the primitive types, the types derived from them, and the three list types.
//! - [`SimpleType::validate`] checks a lexical representation against a type: white space is normalised, the patterns are matched
//!   against the lexical form, the lexical form is mapped to a [`Value`], and the facets of every derivation step are checked against
//!   the value. A list type checks its items, a union type tries its members. The answer is the value, or a [`Violation`] with the
//!   name of the validity constraint it breaks.
//! - Values ([`Value`]) can be compared for equality and, for the types that have one, order; the order of dates, times and durations
//!   is partial, as in the specification.

mod binary;
mod decimal;
mod derive;
mod duration;
mod error;
mod facets;
mod name;
mod number;
mod temporal;
mod types;
mod uri;
mod value;
mod whitespace;

pub use binary::{parse_base64, parse_hex, to_base64, to_hex};
pub use decimal::Decimal;
pub use derive::{FacetSpec, Setting, list_of, restrict, union_of};
pub use duration::Duration;
pub use error::{Violation, ViolationKind};
pub use facets::{Facets, Fixed};
pub use name::{is_language, is_name, is_name_char, is_name_start_char, is_ncname, is_nmtoken, split_qname};
pub use number::{double_canonical, float_canonical, parse_double, parse_float};
pub use temporal::{Kind as TemporalKind, Temporal};
pub use types::{Derivation, ExpandedName, NamespaceContext, NoNamespaces, Primitive, SimpleType, Variety, XSD_NAMESPACE};
pub use uri::is_any_uri;
pub use value::{QNameValue, Value};
pub use whitespace::{Whitespace, collapse};
