//! The profiler: an expression run once with the engine telling about each instruction, and for each node of the expression (a step,
//! a predicate, a comparison, a call; see [`crate::probe`]) how often it was evaluated, how many items it produced and what it cost.
//!
//! - **Hits**: for a node that loops (an axis step applied to each context item, a predicate tested on each item, a `for`, `some` or
//!   `every`) the number of items it was evaluated for; for any other node the number of times the run went into it.
//! - **Cost**: the instructions the engine ran for the node itself (*self*) and with the nodes inside it (*total*), and the time
//!   (natively; on the web there is no clock, so only the instructions). A built-in function's work is the cost of its call.
//! - **Items**: the items the node's evaluations produced in all; for an axis step also the nodes it selected before its predicates.
//!
//! A profiled run is slower than a normal one (the engine stops before each instruction); a normal evaluation is not slowed (one check
//! per instruction). The run is bounded by [`Limits::max_instructions`]: a run over it stops and says so.

use std::ops::Range;

use xee_interpreter::error::{ApplicationError, Error as EngineError, Result as EngineResult};
use xee_interpreter::interpreter::hook::{At, Hook};

use crate::probe::{Event, Nodes, Tracker, excerpt};
use crate::{Evaluated, Options, Phase, Prepared, QueryError};

/// The most characters of a node's text kept.
const TEXT_CHARS: usize = 80;

/// What bounds a profiled (or debugged) run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The most instructions the engine runs before the run is stopped.
    pub max_instructions: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_instructions: 50_000_000 }
    }
}

/// One node of the expression and what its evaluations cost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileNode {
    /// Where it is in the expression (bytes).
    pub span: Range<usize>,
    /// Its text, on one line and cut.
    pub text: String,
    /// The node it is in (an index into [`Profile::nodes`]).
    pub parent: Option<usize>,
    /// How deep it is (0 for an outermost node).
    pub depth: usize,
    /// It loops over a sequence (an axis step, a predicate, a `for`): its hits are the items it was evaluated for.
    pub is_loop: bool,
    /// The headline count: the iterations of a node that loops, else the times the run went into it.
    pub hits: u64,
    /// The times the run went into it.
    pub entries: u64,
    /// The items it was evaluated for (a node that loops).
    pub iterations: u64,
    /// The items its evaluations produced.
    pub items: u64,
    /// For an axis step: the nodes it selected, before any predicate.
    pub selected: u64,
    pub self_instructions: u64,
    pub total_instructions: u64,
    /// Nanoseconds (0 where there is no clock, see [`Profile::timed`]).
    pub self_nanos: u64,
    pub total_nanos: u64,
}

/// A profiled run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    /// What the run returned (or why it failed or was stopped).
    pub result: Result<Evaluated, QueryError>,
    /// The nodes in the order of the text, each after the node it is in: the tree, by [`ProfileNode::parent`].
    pub nodes: Vec<ProfileNode>,
    /// The nodes that cost something, the costliest first (by self time, else self instructions): the hot spots.
    pub hot: Vec<usize>,
    /// The instructions the engine ran.
    pub instructions: u64,
    /// The time of the run in nanoseconds (0 without a clock).
    pub nanos: u64,
    /// Whether there was a clock (natively yes; on the web no).
    pub timed: bool,
    /// The run was stopped over [`Limits::max_instructions`].
    pub stopped: bool,
}

#[cfg(not(target_arch = "wasm32"))]
mod clock {
    pub(crate) const TIMED: bool = true;
    pub(crate) struct Clock(std::time::Instant);
    impl Clock {
        pub(crate) fn start() -> Self {
            Self(std::time::Instant::now())
        }
        pub(crate) fn nanos(&self) -> u64 {
            u64::try_from(self.0.elapsed().as_nanos()).unwrap_or(u64::MAX)
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod clock {
    pub(crate) const TIMED: bool = false;
    pub(crate) struct Clock;
    impl Clock {
        pub(crate) fn start() -> Self {
            Self
        }
        pub(crate) fn nanos(&self) -> u64 {
            0
        }
    }
}

/// The error a hook stops a run with: the caller knows why.
pub(crate) fn stopped_error(why: &str) -> EngineError {
    let name = xot::xmlname::OwnedName::new("stopped".to_owned(), "urn:xmlcraft:query".to_owned(), String::new());
    EngineError::Application(Box::new(ApplicationError::new(name, why.to_owned())))
}

#[derive(Clone, Copy, Debug, Default)]
struct Counts {
    entries: u64,
    iterations: u64,
    items: u64,
    selected: u64,
    instructions: u64,
    nanos: u64,
}

struct Profiler {
    nodes: Nodes,
    tracker: Tracker,
    counts: Vec<Counts>,
    events: Vec<Event>,
    clock: clock::Clock,
    last: u64,
    /// The node the last instruction belonged to.
    last_node: Option<usize>,
    limit: u64,
    stopped: bool,
}

impl Profiler {
    fn count(&mut self) {
        for event in &self.events {
            match *event {
                Event::Enter { node, .. } => {
                    if let Some(c) = self.counts.get_mut(node) {
                        c.entries = c.entries.saturating_add(1);
                    }
                }
                Event::Iterate { node, .. } => {
                    if let Some(c) = self.counts.get_mut(node) {
                        c.iterations = c.iterations.saturating_add(1);
                    }
                }
                Event::Exit { node, items, .. } => {
                    if let Some(c) = self.counts.get_mut(node) {
                        c.items = c.items.saturating_add(items as u64);
                    }
                }
                Event::Stepped { node, nodes } => {
                    if let Some(c) = self.counts.get_mut(node) {
                        c.selected = c.selected.saturating_add(nodes as u64);
                    }
                }
            }
        }
    }

    /// The time and the instruction since the last call go to the node the last instruction belonged to.
    fn charge(&mut self) {
        let now = self.clock.nanos();
        let spent = now.saturating_sub(self.last);
        self.last = now;
        if let Some(c) = self.last_node.and_then(|n| self.counts.get_mut(n)) {
            c.nanos = c.nanos.saturating_add(spent);
            c.instructions = c.instructions.saturating_add(1);
        }
    }
}

impl Hook for Profiler {
    fn before(&mut self, at: &At<'_, '_>) -> EngineResult<()> {
        if self.tracker.instructions > 0 {
            self.charge();
        }
        if self.tracker.instructions >= self.limit {
            self.stopped = true;
            return Err(stopped_error("the profiled run was stopped: it ran more instructions than its limit"));
        }
        let mut events = std::mem::take(&mut self.events);
        self.tracker.observe(&self.nodes, at, &mut events);
        self.events = events;
        self.count();
        self.last_node = self.tracker.current();
        Ok(())
    }
}

/// Profile `expression` on the document `xml`.
pub fn profile(xml: &str, expression: &str, options: &Options, limits: &Limits) -> Profile {
    match Prepared::new(xml) {
        Ok(mut prepared) => prepared.profile(expression, options, limits),
        Err(e) => Profile { result: Err(e), nodes: Vec::new(), hot: Vec::new(), instructions: 0, nanos: 0, timed: clock::TIMED, stopped: false },
    }
}

impl Prepared {
    /// Profile `expression` with the document as the context item.
    pub fn profile(&mut self, expression: &str, options: &Options, limits: &Limits) -> Profile {
        let started = clock::Clock::start();
        // (The nodes are found from the compiled program, before the run.)
        let outcome = self.run_hooked(expression, options, |program, _, _| {
            let nodes = Nodes::of(program, expression);
            Profiler {
                counts: vec![Counts::default(); nodes.nodes.len()],
                nodes,
                tracker: Tracker::default(),
                events: Vec::new(),
                clock: clock::Clock::start(),
                last: 0,
                last_node: None,
                limit: limits.max_instructions,
                stopped: false,
            }
        });
        let nanos = started.nanos();
        let (result, mut profiler) = match outcome {
            Ok(done) => done,
            Err(e) => {
                return Profile { result: Err(e), nodes: Vec::new(), hot: Vec::new(), instructions: 0, nanos, timed: clock::TIMED, stopped: false };
            }
        };
        profiler.charge();
        let items = result.as_ref().map_or(0, |r| r.count);
        let mut events = Vec::new();
        profiler.tracker.finish(&profiler.nodes, items, &mut events);
        profiler.events = events;
        profiler.count();
        let stopped = profiler.stopped;
        let result = if stopped {
            Err(QueryError {
                phase: Phase::Run,
                code: String::new(),
                message: format!("the profiled run was stopped after {} instructions (the limit)", limits.max_instructions),
                span: None,
            })
        } else {
            result
        };
        build(result, &profiler.nodes, &profiler.counts, profiler.tracker.instructions, nanos, stopped, expression)
    }
}

fn build(
    result: Result<Evaluated, QueryError>,
    nodes: &Nodes,
    counts: &[Counts],
    instructions: u64,
    nanos: u64,
    stopped: bool,
    expression: &str,
) -> Profile {
    let mut out: Vec<ProfileNode> = nodes
        .nodes
        .iter()
        .zip(counts)
        .map(|(n, c)| ProfileNode {
            span: n.span.clone(),
            text: excerpt(expression.get(n.span.clone()).unwrap_or_default(), TEXT_CHARS),
            parent: n.parent,
            depth: n.depth,
            is_loop: n.is_loop,
            hits: if n.is_loop { c.iterations } else { c.entries },
            entries: c.entries,
            iterations: c.iterations,
            items: c.items,
            selected: c.selected,
            self_instructions: c.instructions,
            total_instructions: c.instructions,
            self_nanos: c.nanos,
            total_nanos: c.nanos,
        })
        .collect();
    // The totals: each node's own cost and that of the nodes inside it. A node comes after its parent, so adding from the end up adds
    // every descendant before its ancestor is added to.
    for i in (0..out.len()).rev() {
        let (Some(parent), Some(node)) = (out.get(i).and_then(|n| n.parent), out.get(i)) else { continue };
        let (instructions, nanos) = (node.total_instructions, node.total_nanos);
        if let Some(p) = out.get_mut(parent) {
            p.total_instructions = p.total_instructions.saturating_add(instructions);
            p.total_nanos = p.total_nanos.saturating_add(nanos);
        }
    }
    let mut hot: Vec<(u64, u64, usize, usize)> =
        out.iter().enumerate().filter(|(_, n)| n.self_instructions > 0).map(|(i, n)| (n.self_nanos, n.self_instructions, n.span.start, i)).collect();
    hot.sort_by(|x, y| y.0.cmp(&x.0).then(y.1.cmp(&x.1)).then(x.2.cmp(&y.2)));
    let hot = hot.into_iter().map(|(_, _, _, i)| i).collect();
    Profile { result, nodes: out, hot, instructions, nanos, timed: clock::TIMED, stopped }
}
