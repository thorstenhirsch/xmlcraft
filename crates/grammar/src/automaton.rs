//! Content models compiled to Glushkov (position) automata.
//!
//! Every occurrence of an element or wildcard in the (expanded) content model is a *position*; the automaton knows which positions may
//! come first, which may follow which, and which may come last. Reading a child element moves from a set of positions to the positions
//! that follow and match it. For a deterministic model (which both DTD and XML Schema require) the set has one position, so the cost of
//! a step is the size of one follow list, looked up by name when the list is long.
//!
//! Counted occurrences (`{m,n}`) are unrolled into copies of the particle, up to [`MAX_POSITIONS`] positions in all.

use std::collections::{HashMap, VecDeque};

use crate::name::{Name, Term};
use crate::particle::{Occurs, Particle, ParticleKind};

/// The most positions one content model may unroll to.
pub const MAX_POSITIONS: usize = 100_000;
/// The most work (edges made, positions copied) building one automaton may take.
pub const MAX_WORK: u64 = 10_000_000;
/// How deep particles may nest.
pub const MAX_DEPTH: usize = 256;
/// A follow list longer than this is looked up by name.
const INDEX_FROM: usize = 8;
/// `expected` says what is mandatory only where there are at most this many different things to say it of.
const MANDATORY_CANDIDATES: usize = 32;

/// Why a content model cannot be compiled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompileError {
    /// A particle that may occur at least `min` and at most `max` times, with `min` more than `max`.
    BadOccurs { min: u32, max: u32 },
    /// Unrolling the counts gives more than [`MAX_POSITIONS`] positions, or building the automaton takes more than [`MAX_WORK`].
    TooLarge,
    /// The particles nest deeper than [`MAX_DEPTH`].
    TooDeep,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompileError::BadOccurs { min, max } => write!(f, "a particle cannot occur at least {min} and at most {max} times"),
            CompileError::TooLarge => write!(f, "the content model is too large (more than {MAX_POSITIONS} positions once its counts are unrolled)"),
            CompileError::TooDeep => write!(f, "the content model nests deeper than {MAX_DEPTH} levels"),
        }
    }
}

impl std::error::Error for CompileError {}

/// Two particles of a model that can both match the same element: the model is not deterministic ("Unique Particle Attribution").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ambiguity {
    /// The tokens of the two particles.
    pub first: usize,
    pub second: usize,
    /// What they compete for (the term of the first).
    pub term: Term,
}

/// What may come next, as far as [`Automaton::expected`] tells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expected<'a> {
    pub term: &'a Term,
    /// The token of the particle (of the first one, if the term is in the model more than once).
    pub token: usize,
    /// The content cannot be finished without this one: every way on passes through it.
    pub mandatory: bool,
}

/// An element or wildcard of the model, as it comes in a completion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol<'a> {
    pub term: &'a Term,
    pub token: usize,
}

/// Where a read of the children has got to: before the first, or after one of a set of positions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    at_start: bool,
    positions: Vec<u32>,
}

impl State {
    /// How many positions the children read so far may be at: one for a deterministic model (and for the start).
    pub fn alternatives(&self) -> usize {
        if self.at_start { 1 } else { self.positions.len() }
    }
}

/// A step that was possible.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub state: State,
    /// The token of the particle that took the element (the first, in position order, if more than one could).
    pub token: usize,
}

#[derive(Clone, Debug)]
struct Leaf {
    term: Term,
    token: usize,
}

/// Positions that may come next, with a way to find those that match a name quickly when there are many.
#[derive(Clone, Debug, Default)]
struct Follow {
    items: Vec<u32>,
    index: Option<Index>,
}

#[derive(Clone, Debug)]
struct Index {
    /// The positions of elements, by local name.
    by_local: HashMap<String, Vec<u32>>,
    /// The positions of wildcards.
    wildcards: Vec<u32>,
}

/// A content model, compiled.
#[derive(Clone, Debug)]
pub struct Automaton {
    leaves: Vec<Leaf>,
    /// The leaf each position is an occurrence of.
    leaf_of: Vec<u32>,
    start: Follow,
    follow: Vec<Follow>,
    accepting: Vec<bool>,
    nullable: bool,
}

// --- the expanded expression -----------------------------------------------------------------------------------------------

/// The content model with its counts unrolled.
enum Re {
    /// Nothing: the empty sequence.
    Eps,
    /// No sequence at all.
    Fail,
    /// An element or wildcard (an index into the leaves).
    Leaf(usize),
    Seq(Vec<Re>),
    Alt(Vec<Re>),
    /// `x*`
    Star(Box<Re>),
    /// `x+`
    Plus(Box<Re>),
    /// `x?`
    Opt(Box<Re>),
    /// `(c1 (c2 (c3)?)?)?`: the optional copies of a counted particle, kept flat so that a count of a thousand is not a nesting of a
    /// thousand.
    Tail(Vec<Re>),
}

struct Expander {
    leaves: Vec<Leaf>,
}

impl Expander {
    /// The particle unrolled, and the number of positions that gives.
    fn expand(&mut self, particle: &Particle, depth: usize) -> Result<(Re, usize), CompileError> {
        if depth > MAX_DEPTH {
            return Err(CompileError::TooDeep);
        }
        let (inner, size) = match &particle.kind {
            ParticleKind::Term { term, token } => {
                self.leaves.push(Leaf { term: term.clone(), token: *token });
                (Re::Leaf(self.leaves.len() - 1), 1)
            }
            ParticleKind::Sequence(items) => {
                let mut parts = Vec::with_capacity(items.len());
                let mut size = 0usize;
                for item in items {
                    let (re, n) = self.expand(item, depth + 1)?;
                    size = size.checked_add(n).filter(|&n| n <= MAX_POSITIONS).ok_or(CompileError::TooLarge)?;
                    parts.push(re);
                }
                (Re::Seq(parts), size)
            }
            ParticleKind::Choice(items) => {
                if items.is_empty() {
                    (Re::Fail, 0)
                } else {
                    let mut parts = Vec::with_capacity(items.len());
                    let mut size = 0usize;
                    for item in items {
                        let (re, n) = self.expand(item, depth + 1)?;
                        size = size.checked_add(n).filter(|&n| n <= MAX_POSITIONS).ok_or(CompileError::TooLarge)?;
                        parts.push(re);
                    }
                    (Re::Alt(parts), size)
                }
            }
        };
        occurring(inner, size, particle.occurs)
    }
}

/// `re` (which has `size` positions) occurring as `occurs` says.
fn occurring(re: Re, size: usize, occurs: Occurs) -> Result<(Re, usize), CompileError> {
    let Occurs { min, max } = occurs;
    if let Some(max) = max
        && min > max
    {
        return Err(CompileError::BadOccurs { min, max });
    }
    if max == Some(0) {
        return Ok((Re::Eps, 0));
    }
    // Something without positions is the empty sequence or nothing at all, however often it occurs; something that cannot occur occurs
    // zero times or not at all.
    if size == 0 {
        return Ok((if min > 0 && matches_nothing(&re) { Re::Fail } else { Re::Eps }, 0));
    }
    // The number of copies of the particle that the unrolled expression holds.
    let copies = u64::from(match max {
        Some(n) => n,
        None => min.max(1),
    });
    let total = (size as u64).checked_mul(copies).filter(|&t| t <= MAX_POSITIONS as u64).ok_or(CompileError::TooLarge)?;
    let total = total as usize;
    let repeat = |n: u32, re: &Re| -> Vec<Re> { (0..n).map(|_| copy(re)).collect() };
    let out = match (min, max) {
        (1, Some(1)) => re,
        (0, Some(1)) => Re::Opt(Box::new(re)),
        (0, None) => Re::Star(Box::new(re)),
        (1, None) => Re::Plus(Box::new(re)),
        (m, None) => {
            let mut parts = repeat(m - 1, &re);
            parts.push(Re::Plus(Box::new(re)));
            Re::Seq(parts)
        }
        (m, Some(n)) => {
            let mut parts = repeat(m, &re);
            if n > m {
                parts.push(Re::Tail(repeat(n - m, &re)));
            }
            if parts.len() == 1 { parts.pop().unwrap_or(Re::Eps) } else { Re::Seq(parts) }
        }
    };
    Ok((out, total))
}

/// Whether an expression with no positions has no sequence at all (rather than only the empty one).
fn matches_nothing(re: &Re) -> bool {
    match re {
        Re::Fail => true,
        Re::Seq(items) => items.iter().any(matches_nothing),
        Re::Alt(items) => items.iter().all(matches_nothing),
        Re::Plus(x) => matches_nothing(x),
        Re::Eps | Re::Leaf(_) | Re::Star(_) | Re::Opt(_) | Re::Tail(_) => false,
    }
}

/// A copy of an expression (a leaf stays the same leaf: the copies are different positions of it).
fn copy(re: &Re) -> Re {
    match re {
        Re::Eps => Re::Eps,
        Re::Fail => Re::Fail,
        Re::Leaf(i) => Re::Leaf(*i),
        Re::Seq(items) => Re::Seq(items.iter().map(copy).collect()),
        Re::Alt(items) => Re::Alt(items.iter().map(copy).collect()),
        Re::Star(x) => Re::Star(Box::new(copy(x))),
        Re::Plus(x) => Re::Plus(Box::new(copy(x))),
        Re::Opt(x) => Re::Opt(Box::new(copy(x))),
        Re::Tail(items) => Re::Tail(items.iter().map(copy).collect()),
    }
}

// --- the position sets -------------------------------------------------------------------------------------------------------

/// What the Glushkov construction needs of an expression: whether it matches the empty sequence, and which positions may come first
/// and last.
#[derive(Default)]
struct Summary {
    nullable: bool,
    first: Vec<u32>,
    last: Vec<u32>,
}

struct Builder {
    follow: Vec<Vec<u32>>,
    leaf_of: Vec<u32>,
    work: u64,
}

impl Builder {
    fn spend(&mut self, amount: usize) -> Result<(), CompileError> {
        self.work = self.work.saturating_add(amount as u64);
        if self.work > MAX_WORK { Err(CompileError::TooLarge) } else { Ok(()) }
    }

    /// Every position of `last` may be followed by every position of `first`.
    fn connect(&mut self, last: &[u32], first: &[u32]) -> Result<(), CompileError> {
        self.spend(last.len().saturating_mul(first.len()))?;
        for &p in last {
            if let Some(list) = self.follow.get_mut(p as usize) {
                list.extend_from_slice(first);
            }
        }
        Ok(())
    }

    fn summarize(&mut self, re: &Re) -> Result<Summary, CompileError> {
        match re {
            Re::Eps => Ok(Summary { nullable: true, ..Summary::default() }),
            Re::Fail => Ok(Summary::default()),
            Re::Leaf(leaf) => {
                let position = u32::try_from(self.leaf_of.len()).map_err(|_| CompileError::TooLarge)?;
                self.leaf_of.push(u32::try_from(*leaf).map_err(|_| CompileError::TooLarge)?);
                self.follow.push(Vec::new());
                Ok(Summary { nullable: false, first: vec![position], last: vec![position] })
            }
            Re::Seq(items) => {
                let mut acc = Summary { nullable: true, ..Summary::default() };
                for item in items {
                    let s = self.summarize(item)?;
                    self.connect(&acc.last, &s.first)?;
                    self.spend(s.first.len())?;
                    if acc.nullable {
                        acc.first.extend_from_slice(&s.first);
                    }
                    if s.nullable {
                        self.spend(s.last.len())?;
                        acc.last.extend_from_slice(&s.last);
                    } else {
                        acc.last = s.last;
                    }
                    acc.nullable &= s.nullable;
                }
                Ok(acc)
            }
            Re::Alt(items) => {
                let mut acc = Summary::default();
                for item in items {
                    let s = self.summarize(item)?;
                    acc.nullable |= s.nullable;
                    acc.first.extend_from_slice(&s.first);
                    acc.last.extend_from_slice(&s.last);
                }
                Ok(acc)
            }
            Re::Star(x) | Re::Plus(x) => {
                let mut s = self.summarize(x)?;
                self.connect(&s.last, &s.first)?;
                if matches!(re, Re::Star(_)) {
                    s.nullable = true;
                }
                Ok(s)
            }
            Re::Opt(x) => {
                let mut s = self.summarize(x)?;
                s.nullable = true;
                Ok(s)
            }
            Re::Tail(copies) => {
                // From the last copy to the first: tail = (copy tail')?.
                let mut tail = Summary { nullable: true, ..Summary::default() };
                for copy in copies.iter().rev() {
                    let s = self.summarize(copy)?;
                    self.connect(&s.last, &tail.first)?;
                    self.spend(s.first.len().saturating_add(s.last.len()))?;
                    let mut first = s.first;
                    if s.nullable {
                        self.spend(tail.first.len())?;
                        first.extend_from_slice(&tail.first);
                    }
                    // `tail` can be empty, so the end may come after the copy or after the rest.
                    let mut last = tail.last;
                    last.extend_from_slice(&s.last);
                    tail = Summary { nullable: true, first, last };
                }
                Ok(tail)
            }
        }
    }
}

// --- the automaton -----------------------------------------------------------------------------------------------------------

impl Automaton {
    /// Compile a content model.
    pub fn compile(particle: &Particle) -> Result<Self, CompileError> {
        let mut expander = Expander { leaves: Vec::new() };
        let (re, _) = expander.expand(particle, 0)?;
        let mut builder = Builder { follow: Vec::new(), leaf_of: Vec::new(), work: 0 };
        let whole = builder.summarize(&re)?;
        drop(re);
        let Builder { follow, leaf_of, .. } = builder;
        let n = leaf_of.len();

        // Only the positions that can be reached and can lead to the end are kept: a step that works is then always on the way to a
        // content that is valid.
        let mut reachable = vec![false; n];
        let mut queue: VecDeque<u32> = VecDeque::new();
        for &p in &whole.first {
            if let Some(seen) = reachable.get_mut(p as usize)
                && !*seen
            {
                *seen = true;
                queue.push_back(p);
            }
        }
        while let Some(p) = queue.pop_front() {
            for &q in follow.get(p as usize).map(Vec::as_slice).unwrap_or_default() {
                if let Some(seen) = reachable.get_mut(q as usize)
                    && !*seen
                {
                    *seen = true;
                    queue.push_back(q);
                }
            }
        }
        let mut before: Vec<Vec<u32>> = vec![Vec::new(); n];
        for (p, list) in follow.iter().enumerate() {
            if reachable.get(p).copied().unwrap_or(false) {
                for &q in list {
                    if let Some(back) = before.get_mut(q as usize) {
                        back.push(p as u32);
                    }
                }
            }
        }
        let mut leads_to_end = vec![false; n];
        for &p in &whole.last {
            if reachable.get(p as usize).copied().unwrap_or(false)
                && let Some(seen) = leads_to_end.get_mut(p as usize)
                && !*seen
            {
                *seen = true;
                queue.push_back(p);
            }
        }
        while let Some(q) = queue.pop_front() {
            for &p in before.get(q as usize).map(Vec::as_slice).unwrap_or_default() {
                if let Some(seen) = leads_to_end.get_mut(p as usize)
                    && !*seen
                {
                    *seen = true;
                    queue.push_back(p);
                }
            }
        }
        drop(before);
        let useful: Vec<bool> = (0..n).map(|p| reachable.get(p).copied().unwrap_or(false) && leads_to_end.get(p).copied().unwrap_or(false)).collect();
        let keep = |list: &[u32]| -> Vec<u32> {
            let mut out: Vec<u32> = list.iter().copied().filter(|&p| useful.get(p as usize).copied().unwrap_or(false)).collect();
            out.sort_unstable();
            out.dedup();
            out
        };
        let mut accepting = vec![false; n];
        for &p in &whole.last {
            if useful.get(p as usize).copied().unwrap_or(false)
                && let Some(flag) = accepting.get_mut(p as usize)
            {
                *flag = true;
            }
        }
        let leaves = expander.leaves;
        let start = Follow::new(keep(&whole.first), &leaf_of, &leaves);
        let follow: Vec<Follow> = follow
            .iter()
            .enumerate()
            .map(|(p, list)| {
                let items = if useful.get(p).copied().unwrap_or(false) { keep(list) } else { Vec::new() };
                Follow::new(items, &leaf_of, &leaves)
            })
            .collect();
        Ok(Self { leaves, leaf_of, start, follow, accepting, nullable: whole.nullable })
    }

    /// The number of positions (occurrences of elements and wildcards once the counts are unrolled).
    pub fn position_count(&self) -> usize {
        self.leaf_of.len()
    }

    fn leaf(&self, position: u32) -> Option<&Leaf> {
        self.leaf_of.get(position as usize).and_then(|&leaf| self.leaves.get(leaf as usize))
    }

    /// The term and token of a position.
    pub fn symbol(&self, position: u32) -> Option<Symbol<'_>> {
        self.leaf(position).map(|leaf| Symbol { term: &leaf.term, token: leaf.token })
    }

    /// Whether no content at all is valid.
    pub fn matches_nothing(&self) -> bool {
        !self.nullable && self.start.items.is_empty()
    }

    /// Where reading the children starts.
    pub fn start(&self) -> State {
        State { at_start: true, positions: Vec::new() }
    }

    /// Whether the children read so far are a complete content.
    pub fn is_accepting(&self, state: &State) -> bool {
        if state.at_start { self.nullable } else { state.positions.iter().any(|&p| self.accepting.get(p as usize).copied().unwrap_or(false)) }
    }

    fn follow_of(&self, state: &State) -> Vec<&Follow> {
        if state.at_start { vec![&self.start] } else { state.positions.iter().filter_map(|&p| self.follow.get(p as usize)).collect() }
    }

    /// The positions that may come next, in position order.
    fn next_positions(&self, state: &State) -> Vec<u32> {
        let mut out: Vec<u32> = self.follow_of(state).iter().flat_map(|f| f.items.iter().copied()).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Read a child element called `name`: where that gets to and which particle took it, or `None` when it cannot come here.
    pub fn step(&self, state: &State, name: &Name) -> Option<Step> {
        let mut next: Vec<u32> = Vec::new();
        for follow in self.follow_of(state) {
            follow.matching(self, name, &mut next);
        }
        next.sort_unstable();
        next.dedup();
        let token = self.leaf(*next.first()?)?.token;
        Some(Step { state: State { at_start: false, positions: next }, token })
    }

    /// What may come next. `mandatory` is set for what the content cannot be finished without (every way on passes through it); it is
    /// worked out where there are few things to choose from (at most 32 different ones), and is false otherwise.
    pub fn expected(&self, state: &State) -> Vec<Expected<'_>> {
        let candidates = self.next_positions(state);
        let mut distinct: Vec<(&Term, usize)> = Vec::new();
        for &p in &candidates {
            if let Some(leaf) = self.leaf(p)
                && !distinct.iter().any(|(term, _)| **term == leaf.term)
            {
                distinct.push((&leaf.term, leaf.token));
            }
        }
        let decide = !self.is_accepting(state) && distinct.len() <= MANDATORY_CANDIDATES;
        distinct.into_iter().map(|(term, token)| Expected { term, token, mandatory: decide && !self.can_finish_without(&candidates, term) }).collect()
    }

    /// Whether the content can be finished from `from` without passing through a position of `avoid`.
    fn can_finish_without(&self, from: &[u32], avoid: &Term) -> bool {
        let mut seen = vec![false; self.position_count()];
        let mut queue: VecDeque<u32> = VecDeque::new();
        for &p in from {
            if self.leaf(p).is_some_and(|leaf| leaf.term != *avoid)
                && let Some(flag) = seen.get_mut(p as usize)
            {
                *flag = true;
                queue.push_back(p);
            }
        }
        while let Some(p) = queue.pop_front() {
            if self.accepting.get(p as usize).copied().unwrap_or(false) {
                return true;
            }
            for &q in self.follow.get(p as usize).map(|f| f.items.as_slice()).unwrap_or_default() {
                if self.leaf(q).is_some_and(|leaf| leaf.term != *avoid)
                    && let Some(flag) = seen.get_mut(q as usize)
                    && !*flag
                {
                    *flag = true;
                    queue.push_back(q);
                }
            }
        }
        false
    }

    /// The shortest content that finishes what was read: empty when it is complete already, `None` when nothing finishes it.
    pub fn shortest_completion(&self, state: &State) -> Option<Vec<Symbol<'_>>> {
        if self.is_accepting(state) {
            return Some(Vec::new());
        }
        let n = self.position_count();
        let mut came_from: Vec<Option<u32>> = vec![None; n];
        let mut seen = vec![false; n];
        let mut queue: VecDeque<u32> = VecDeque::new();
        for p in self.next_positions(state) {
            if let Some(flag) = seen.get_mut(p as usize) {
                *flag = true;
                queue.push_back(p);
            }
        }
        while let Some(p) = queue.pop_front() {
            if self.accepting.get(p as usize).copied().unwrap_or(false) {
                let mut path = vec![p];
                let mut at = p;
                while let Some(Some(before)) = came_from.get(at as usize) {
                    path.push(*before);
                    at = *before;
                }
                path.reverse();
                return path.into_iter().map(|p| self.symbol(p)).collect();
            }
            for &q in self.follow.get(p as usize).map(|f| f.items.as_slice()).unwrap_or_default() {
                if let Some(flag) = seen.get_mut(q as usize)
                    && !*flag
                {
                    *flag = true;
                    if let Some(slot) = came_from.get_mut(q as usize) {
                        *slot = Some(p);
                    }
                    queue.push_back(q);
                }
            }
        }
        None
    }

    /// Whether the model is deterministic: whatever has been read, an element can be taken by at most one particle ("Unique Particle
    /// Attribution" in XML Schema; the same rule of XML 1.0 for content models). The first two particles that compete are the error.
    pub fn check_deterministic(&self) -> Result<(), Ambiguity> {
        for follow in std::iter::once(&self.start).chain(self.follow.iter()) {
            if let Some(ambiguity) = self.ambiguity_in(&follow.items) {
                return Err(ambiguity);
            }
        }
        Ok(())
    }

    fn ambiguity_in(&self, items: &[u32]) -> Option<Ambiguity> {
        let ambiguity = |a: u32, b: u32| {
            let first = self.leaf(a)?;
            Some(Ambiguity { first: first.token, second: self.leaf(b)?.token, term: first.term.clone() })
        };
        let mut seen: HashMap<&Name, u32> = HashMap::new();
        let mut wildcards: Vec<u32> = Vec::new();
        for &p in items {
            match &self.leaf(p)?.term {
                Term::Element(name) => match seen.get(name) {
                    Some(&q) if q != p => return ambiguity(q, p),
                    Some(_) => {}
                    None => {
                        seen.insert(name, p);
                    }
                },
                Term::Wildcard(_) => wildcards.push(p),
            }
        }
        for &w in &wildcards {
            let term = &self.leaf(w)?.term;
            for &p in items {
                if p != w && self.leaf(p).is_some_and(|other| term.overlaps(&other.term)) {
                    return ambiguity(w.min(p), w.max(p));
                }
            }
        }
        None
    }
}

impl Follow {
    fn new(items: Vec<u32>, leaf_of: &[u32], leaves: &[Leaf]) -> Self {
        let term_of = |p: u32| leaf_of.get(p as usize).and_then(|&l| leaves.get(l as usize)).map(|leaf| &leaf.term);
        let index = (items.len() > INDEX_FROM).then(|| {
            let mut by_local: HashMap<String, Vec<u32>> = HashMap::new();
            let mut wildcards = Vec::new();
            for &p in &items {
                match term_of(p) {
                    Some(Term::Element(name)) => by_local.entry(name.local.clone()).or_default().push(p),
                    Some(Term::Wildcard(_)) => wildcards.push(p),
                    None => {}
                }
            }
            Index { by_local, wildcards }
        });
        Self { items, index }
    }

    /// Add to `out` the positions of this list that take an element called `name`.
    fn matching(&self, automaton: &Automaton, name: &Name, out: &mut Vec<u32>) {
        let takes = |p: u32| automaton.leaf(p).is_some_and(|leaf| leaf.term.matches(name));
        match &self.index {
            Some(index) => {
                if let Some(list) = index.by_local.get(name.local.as_str()) {
                    out.extend(list.iter().copied().filter(|&p| takes(p)));
                }
                out.extend(index.wildcards.iter().copied().filter(|&p| takes(p)));
            }
            None => out.extend(self.items.iter().copied().filter(|&p| takes(p))),
        }
    }
}
