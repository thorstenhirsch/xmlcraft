//! Why a pattern is not an XML Schema regular expression.

use std::fmt;

/// What is wrong with a pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The pattern is longer than [`crate::MAX_PATTERN_CHARS`].
    TooLong,
    /// Groups or character classes nest deeper than [`crate::MAX_DEPTH`].
    TooDeep,
    /// A `(` without its `)`.
    UnclosedGroup,
    /// A `)` without its `(`.
    UnbalancedClose,
    /// A `[` without its `]`.
    UnclosedClass,
    /// `[]` or `[^]`: a class with nothing in it.
    EmptyClass,
    /// `?`, `*`, `+` or `{` where there is nothing before it to repeat.
    NothingToRepeat(char),
    /// A quantifier after a quantifier (`a**`, `a+?`).
    DoubleQuantifier,
    /// A `{` that is not `{n}`, `{n,}` or `{n,m}`.
    BadQuantifier,
    /// `{n,m}` with `m` less than `n`.
    QuantifierRange,
    /// A count above [`crate::MAX_COUNT`].
    QuantifierTooLarge,
    /// `\` followed by a character that is not an escape.
    UnknownEscape(char),
    /// A `\` at the end.
    TrailingBackslash,
    /// `\p` or `\P` not followed by `{name}`.
    BadProperty,
    /// `\p{X}` where `X` is no category.
    UnknownCategory(String),
    /// `\p{IsX}` where `X` is no block.
    UnknownBlock(String),
    /// A character that has to be escaped here (`]`, `}`, `{`, `[` inside a class).
    NeedsEscape(char),
    /// A range that is not one: `z-a`, `\d-z`, `a-\d`, or a `-` that is neither first, last nor between two characters.
    BadRange,
    /// A class subtraction `-[...]` that is not the end of its class.
    SubtractionNotLast,
    /// The pattern is valid but needs more memory than a pattern may have.
    TooComplex,
}

/// A pattern that is not an XML Schema regular expression, and where: `position` is the number of the character (from 0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatternError {
    pub position: usize,
    pub kind: ErrorKind,
}

impl PatternError {
    pub(crate) fn new(position: usize, kind: ErrorKind) -> Self {
        Self { position, kind }
    }

    /// The problem in words (without the position).
    pub fn message(&self) -> String {
        match &self.kind {
            ErrorKind::TooLong => format!("the pattern is longer than {} characters", crate::MAX_PATTERN_CHARS),
            ErrorKind::TooDeep => format!("groups and classes nest deeper than {}", crate::MAX_DEPTH),
            ErrorKind::UnclosedGroup => "a ( has no )".to_owned(),
            ErrorKind::UnbalancedClose => "a ) has no (".to_owned(),
            ErrorKind::UnclosedClass => "a [ has no ]".to_owned(),
            ErrorKind::EmptyClass => "a character class has no characters".to_owned(),
            ErrorKind::NothingToRepeat(c) => format!("'{c}' has nothing before it to repeat"),
            ErrorKind::DoubleQuantifier => "a quantifier cannot follow a quantifier".to_owned(),
            ErrorKind::BadQuantifier => "a { must be followed by n}, n,} or n,m}".to_owned(),
            ErrorKind::QuantifierRange => "the upper bound of a quantifier is less than its lower bound".to_owned(),
            ErrorKind::QuantifierTooLarge => format!("a quantifier counts higher than {}", crate::MAX_COUNT),
            ErrorKind::UnknownEscape(c) => format!("'\\{c}' is not an escape"),
            ErrorKind::TrailingBackslash => "the pattern ends with a \\".to_owned(),
            ErrorKind::BadProperty => "\\p and \\P are followed by a name in braces, as in \\p{L}".to_owned(),
            ErrorKind::UnknownCategory(name) => format!("'{name}' is not a Unicode category"),
            ErrorKind::UnknownBlock(name) => format!("'{name}' is not a Unicode block"),
            ErrorKind::NeedsEscape(c) => format!("'{c}' must be written \\{c} here"),
            ErrorKind::BadRange => "this is not a character range".to_owned(),
            ErrorKind::SubtractionNotLast => "a subtraction must be the last thing in its class".to_owned(),
            ErrorKind::TooComplex => "the pattern needs more memory than a pattern may use".to_owned(),
        }
    }
}

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "at character {}: {}", self.position + 1, self.message())
    }
}

impl std::error::Error for PatternError {}
