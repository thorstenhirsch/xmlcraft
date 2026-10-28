//! The content model as written: a tree of particles with occurrence counts.

use crate::name::{Name, NamespaceConstraint, Term};

/// How often a particle may occur: at least `min` times and at most `max` (`None`: without limit).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Occurs {
    pub min: u32,
    pub max: Option<u32>,
}

impl Occurs {
    pub const ONCE: Occurs = Occurs { min: 1, max: Some(1) };
    /// `?`
    pub const OPTIONAL: Occurs = Occurs { min: 0, max: Some(1) };
    /// `*`
    pub const ZERO_OR_MORE: Occurs = Occurs { min: 0, max: None };
    /// `+`
    pub const ONE_OR_MORE: Occurs = Occurs { min: 1, max: None };

    pub const fn new(min: u32, max: Option<u32>) -> Self {
        Self { min, max }
    }
}

/// A particle: what it is, and how often it may occur.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Particle {
    pub kind: ParticleKind,
    pub occurs: Occurs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParticleKind {
    /// An element or a wildcard. `token` is the caller's own number for it (an index of the declaration it stands for): it comes back
    /// with every step.
    Term { term: Term, token: usize },
    /// `(a, b, c)`: one after the other.
    Sequence(Vec<Particle>),
    /// `(a | b | c)`: one of them. A choice of nothing allows nothing.
    Choice(Vec<Particle>),
}

impl Particle {
    pub fn element(name: Name, token: usize) -> Self {
        Self { kind: ParticleKind::Term { term: Term::Element(name), token }, occurs: Occurs::ONCE }
    }

    pub fn wildcard(constraint: NamespaceConstraint, token: usize) -> Self {
        Self { kind: ParticleKind::Term { term: Term::Wildcard(constraint), token }, occurs: Occurs::ONCE }
    }

    pub fn sequence(items: Vec<Particle>) -> Self {
        Self { kind: ParticleKind::Sequence(items), occurs: Occurs::ONCE }
    }

    pub fn choice(items: Vec<Particle>) -> Self {
        Self { kind: ParticleKind::Choice(items), occurs: Occurs::ONCE }
    }

    /// The same particle, occurring as often as `occurs` says.
    pub fn occurring(mut self, occurs: Occurs) -> Self {
        self.occurs = occurs;
        self
    }
}
