#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-pattern: XML Schema regular expressions (XML Schema Part 2, appendix F), the language of the `pattern` facet.
//!
//! - **Layer:** L0, no UI and no I/O, so it compiles for the web too.
//! - [`Pattern::new`] reads a pattern the way the XSD syntax defines it and translates it to the syntax tree of the `regex` crates. What differs from the
//!   expressions most programmers know is in the module `parse`; the main points: the whole value has to match (`^` and `$` are
//!   ordinary characters), `.` is anything but a line end, `[a-z-[aeiou]]` takes a class out of a class, `\p{IsBasicLatin}` names a
//!   Unicode block, and `\i`, `\c`, `\I`, `\C` are the characters XML names start with or are made of.
//! - A pattern has limits ([`MAX_PATTERN_CHARS`], [`MAX_DEPTH`], [`MAX_COUNT`], and the memory the finished automaton may use): a
//!   pattern from a schema is untrusted input and is refused, with the place and the reason, instead of being compiled at any cost.
//! - [`Pattern::hir`] is the pattern as a syntax tree of the regex crate, for generating values that match it.

mod blocks;
mod classes;
mod error;
mod parse;

use regex_automata::meta::Regex;
use regex_syntax::hir::{Class, Hir, Look, Repetition};

pub use error::{ErrorKind, PatternError};

use parse::Node;

/// The longest pattern, in characters.
pub const MAX_PATTERN_CHARS: usize = 100_000;
/// How deep groups and character classes may nest.
pub const MAX_DEPTH: usize = 128;
/// The largest number in a quantifier (`{n,m}`).
pub const MAX_COUNT: u32 = 1_000_000;
/// The most memory the finished automaton may use, in bytes.
const MAX_AUTOMATON_BYTES: usize = 32 * 1024 * 1024;

/// An XML Schema regular expression, ready to match values.
#[derive(Clone, Debug)]
pub struct Pattern {
    source: String,
    hir: Hir,
    regex: Regex,
}

impl Pattern {
    /// Read `source`, an XML Schema regular expression.
    pub fn new(source: &str) -> Result<Self, PatternError> {
        let node = parse::parse(source)?;
        let hir = to_hir(&node);
        // The whole value has to match.
        let anchored = Hir::concat(vec![Hir::look(Look::Start), hir.clone(), Hir::look(Look::End)]);
        // The syntax tree goes to the engine as it is: printing it as text and reading that back is not safe for a tree that was not
        // read from text (a repetition of a repetition would print as a lazy or an invalid quantifier).
        let regex = Regex::builder()
            .configure(Regex::config().nfa_size_limit(Some(MAX_AUTOMATON_BYTES)).hybrid_cache_capacity(MAX_AUTOMATON_BYTES))
            .build_from_hir(&anchored)
            .map_err(|_| PatternError { position: 0, kind: ErrorKind::TooComplex })?;
        Ok(Self { source: source.to_owned(), hir, regex })
    }

    /// Whether the whole of `value` matches.
    pub fn is_match(&self, value: &str) -> bool {
        self.regex.is_match(value)
    }

    /// The pattern as it was written.
    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// The pattern as a syntax tree, without the anchors (it describes the values that match).
    pub fn hir(&self) -> &Hir {
        &self.hir
    }
}

fn to_hir(node: &Node) -> Hir {
    match node {
        Node::Empty => Hir::empty(),
        Node::Class(set) => match set.ranges() {
            [only] if only.start() == only.end() => Hir::literal(only.start().to_string().into_bytes()),
            _ => Hir::class(Class::Unicode(set.clone())),
        },
        Node::Concat(nodes) => Hir::concat(nodes.iter().map(to_hir).collect()),
        Node::Alt(nodes) => Hir::alternation(nodes.iter().map(to_hir).collect()),
        Node::Repeat { node, min, max } => Hir::repetition(Repetition { min: *min, max: *max, greedy: true, sub: Box::new(to_hir(node)) }),
    }
}
