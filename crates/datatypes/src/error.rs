//! Why a value is not valid for a type.

use std::fmt;

/// Which rule a value broke. The codes are those of the schema language's validity constraints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ViolationKind {
    /// The lexical representation is not in the lexical space of the type (or of its item or member type).
    Lexical,
    /// A union: no member type accepts the value.
    Union,
    Pattern,
    Enumeration,
    Length,
    MinLength,
    MaxLength,
    MinInclusive,
    MaxInclusive,
    MinExclusive,
    MaxExclusive,
    TotalDigits,
    FractionDigits,
}

impl ViolationKind {
    /// The name of the constraint in the specification (`cvc-pattern-valid`, …).
    pub fn code(self) -> &'static str {
        match self {
            Self::Lexical => "cvc-datatype-valid.1.2.1",
            Self::Union => "cvc-datatype-valid.1.2.3",
            Self::Pattern => "cvc-pattern-valid",
            Self::Enumeration => "cvc-enumeration-valid",
            Self::Length => "cvc-length-valid",
            Self::MinLength => "cvc-minLength-valid",
            Self::MaxLength => "cvc-maxLength-valid",
            Self::MinInclusive => "cvc-minInclusive-valid",
            Self::MaxInclusive => "cvc-maxInclusive-valid",
            Self::MinExclusive => "cvc-minExclusive-valid",
            Self::MaxExclusive => "cvc-maxExclusive-valid",
            Self::TotalDigits => "cvc-totalDigits-valid",
            Self::FractionDigits => "cvc-fractionDigits-valid",
        }
    }
}

/// A value that is not valid for a type, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub kind: ViolationKind,
    pub message: String,
}

impl Violation {
    pub(crate) fn new(kind: ViolationKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Violation {}
