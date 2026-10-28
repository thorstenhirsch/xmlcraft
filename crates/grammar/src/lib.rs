#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-grammar: the grammar model that DTDs and XML Schemas are compiled into.
//!
//! - **Layer:** L2, no UI and no I/O, so it compiles for the web too.
//! - A content model is a tree of [`Particle`]s (elements and wildcards, sequences and choices, each with a count). [`Automaton::compile`]
//!   turns it into a Glushkov automaton, which
//!   - [`step`](Automaton::step)s over the child elements one by one and says which particle took each,
//!   - says what may come next ([`expected`](Automaton::expected), with what is mandatory) and what is the shortest way to finish
//!     ([`shortest_completion`](Automaton::shortest_completion)), which is what validation, completion and sample generation need,
//!   - says whether the model is deterministic ([`check_deterministic`](Automaton::check_deterministic)): XML 1.0 and XML Schema both
//!     forbid a model in which an element could be taken by two particles.
//! - Counted occurrences are unrolled into copies, up to [`MAX_POSITIONS`] in all; deeper nesting and bigger models are an error, not a
//!   hang. A step that works is always on the way to a valid content (positions that cannot be reached or cannot lead to the end are
//!   dropped when the model is compiled).

mod automaton;
mod name;
mod particle;

pub use automaton::{Ambiguity, Automaton, CompileError, Expected, MAX_DEPTH, MAX_POSITIONS, MAX_WORK, State, Step, Symbol};
pub use name::{Name, NamespaceConstraint, Term};
pub use particle::{Occurs, Particle, ParticleKind};
