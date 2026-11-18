//! What the profiler and the debugger share: the expression's nodes, found from the compiled program, and a tracker that turns the
//! engine's instructions into evaluation events.
//!
//! The engine compiles an expression into bytecode and gives every instruction the source span of the expression it belongs to (a
//! step `item`, a predicate `price > 10`, a comparison, a call). The distinct spans, nested by where they are in the text, are the
//! **nodes** of the expression. The interpreter tells a hook about each instruction before it runs (the vendored engine's patch), and
//! the [`Tracker`] follows which node the run is in:
//!
//! - **Enter**: the run goes into a node from outside it (from its parent or a sibling).
//! - **Iterate**: a node that loops over a sequence (an axis step applied to each context item, a predicate tested on each item, a
//!   `for`, `some` or `every`) takes its next item.
//! - **Exit**: the run leaves a node; the value it left is on the stack (its item count is told).
//!
//! Variable reads are left out of the nodes the run is in, except a read of a variable the expression names (`$x`): the engine gives
//! an internal variable the span of where it was made, not of where it is read.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use xee_interpreter::interpreter::Program;
use xee_interpreter::interpreter::hook::At;
use xee_interpreter::interpreter::instruction::{Instruction, decode_instructions, instruction_size};
use xee_interpreter::stack;

/// The most nodes an expression is followed in (more distinct spans than this are not followed).
const MAX_NODES: usize = 20_000;
/// The most bytes of compiled code followed (a larger program runs, its nodes are not followed).
const MAX_CODE: usize = 4_000_000;

/// One node of the expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Node {
    pub span: Range<usize>,
    pub parent: Option<usize>,
    pub depth: usize,
    /// The node loops over a sequence: its iterations are the items it was evaluated for.
    pub is_loop: bool,
}

/// What an instruction is, as far as the tracker cares.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Op {
    #[default]
    Other,
    /// A variable read the run does not move for (an internal variable, or a place with no span).
    Ignored,
    /// The next item of a loop is taken.
    SequenceGet,
    /// An axis step.
    Step,
    /// A function call with this many arguments.
    Call(u8),
    /// A read of a local variable the expression names: its slot in the frame and the name's index in [`Nodes::names`].
    Var(u16, u32),
    /// A read of a variable an inline function closed over: its index and the name's index.
    ClosureVar(u16, u32),
}

/// An instruction: the node it belongs to and what it is.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Code {
    pub node: Option<u32>,
    pub op: Op,
}

/// The nodes of an expression and, for each instruction of each function, its node.
#[derive(Debug, Default)]
pub(crate) struct Nodes {
    pub nodes: Vec<Node>,
    /// For each function, by instruction offset.
    pub code: Vec<Vec<Code>>,
    /// The names of the variables the expression reads (`$x`).
    pub names: Vec<String>,
    /// For each function, the span of its definition (the span of its last instruction), when it has one.
    pub definitions: Vec<Option<Range<usize>>>,
}

impl Nodes {
    /// The nodes of a compiled expression whose text is `source`.
    pub(crate) fn of(program: &Program, source: &str) -> Nodes {
        let mut spans: Vec<Range<usize>> = Vec::new();
        let mut loops: Vec<Range<usize>> = Vec::new();
        let mut code_total = 0usize;
        let mut decoded: Vec<Vec<(usize, Instruction, Range<usize>)>> = Vec::new();
        for function in &program.functions {
            code_total = code_total.saturating_add(function.chunk.len());
            if code_total > MAX_CODE {
                decoded.push(Vec::new());
                continue;
            }
            let mut ip = 0usize;
            let mut list = Vec::new();
            for instruction in decode_instructions(&function.chunk) {
                let span = function.spans.get(ip).map(|s| s.range()).unwrap_or(0..0);
                let size = instruction_size(&instruction);
                list.push((ip, instruction, span));
                ip = ip.saturating_add(size);
            }
            decoded.push(list);
        }
        let user_variable = |span: &Range<usize>| source.get(span.clone()).is_some_and(|t| t.starts_with('$'));
        for list in &decoded {
            for (_, instruction, span) in list {
                if span.is_empty() || span.end > source.len() {
                    continue;
                }
                let ignored = matches!(instruction, Instruction::Var(_) | Instruction::ClosureVar(_)) && !user_variable(span);
                if ignored {
                    continue;
                }
                spans.push(span.clone());
                if matches!(instruction, Instruction::SequenceGet) {
                    loops.push(span.clone());
                }
            }
        }
        spans.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
        spans.dedup();
        spans.truncate(MAX_NODES);
        // Nest them: a span's parent is the nearest one before it that contains it.
        let loops: HashSet<(usize, usize)> = loops.iter().map(|s| (s.start, s.end)).collect();
        let mut nodes: Vec<Node> = Vec::with_capacity(spans.len());
        let mut open: Vec<usize> = Vec::new();
        for span in spans {
            while let Some(t) = open.last().and_then(|&top| nodes.get(top)) {
                if t.span.start <= span.start && span.end <= t.span.end {
                    break;
                }
                open.pop();
            }
            let parent = open.last().copied();
            let depth = parent.and_then(|p| nodes.get(p)).map_or(0, |p| p.depth.saturating_add(1));
            nodes.push(Node { is_loop: loops.contains(&(span.start, span.end)), span, parent, depth });
            open.push(nodes.len() - 1);
        }
        let index: HashMap<(usize, usize), u32> =
            nodes.iter().enumerate().filter_map(|(i, n)| u32::try_from(i).ok().map(|i| ((n.span.start, n.span.end), i))).collect();
        let mut names: Vec<String> = Vec::new();
        let mut name_of = |text: &str| -> u32 {
            if let Some(i) = names.iter().position(|n| n == text) {
                return u32::try_from(i).unwrap_or(u32::MAX);
            }
            names.push(text.to_owned());
            u32::try_from(names.len() - 1).unwrap_or(u32::MAX)
        };
        let mut code = Vec::with_capacity(decoded.len());
        let mut definitions = Vec::with_capacity(decoded.len());
        for (list, function) in decoded.iter().zip(&program.functions) {
            let mut table = vec![Code::default(); function.chunk.len().min(MAX_CODE)];
            for (ip, instruction, span) in list {
                let node = index.get(&(span.start, span.end)).copied();
                let named = source.get(span.clone()).filter(|t| t.starts_with('$')).map(&mut name_of);
                let op = match (instruction, named) {
                    (Instruction::Var(slot), Some(name)) => Op::Var(*slot, name),
                    (Instruction::ClosureVar(slot), Some(name)) => Op::ClosureVar(*slot, name),
                    (Instruction::Var(_) | Instruction::ClosureVar(_), None) => Op::Ignored,
                    (Instruction::SequenceGet, _) => Op::SequenceGet,
                    (Instruction::Step(_), _) => Op::Step,
                    (Instruction::Call(arity), _) => Op::Call(*arity),
                    _ if node.is_none() => Op::Ignored,
                    _ => Op::Other,
                };
                if let Some(slot) = table.get_mut(*ip) {
                    *slot = Code { node: if op == Op::Ignored { None } else { node }, op };
                }
            }
            definitions.push(list.last().map(|(_, _, span)| span.clone()).filter(|s| !s.is_empty()));
            code.push(table);
        }
        Nodes { nodes, code, names, definitions }
    }

    /// What the instruction at `ip` of `function` is.
    pub(crate) fn code(&self, function: usize, ip: usize) -> Code {
        self.code.get(function).and_then(|f| f.get(ip)).copied().unwrap_or_default()
    }

    pub(crate) fn node(&self, index: usize) -> Option<&Node> {
        self.nodes.get(index)
    }

    /// The innermost node at a place in the expression: the smallest one that covers `offset`, else the first that starts there.
    pub(crate) fn at_offset(&self, offset: usize) -> Option<usize> {
        let covering = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.span.start <= offset && offset < n.span.end)
            .min_by_key(|(_, n)| n.span.end - n.span.start)
            .map(|(i, _)| i);
        covering.or_else(|| self.nodes.iter().position(|n| n.span.start == offset))
    }

    /// The nodes from the common ancestor of `from` and `to` (not included) to each of them: what is left (innermost first) and what is
    /// entered (outermost first).
    fn path(&self, from: Option<usize>, to: usize, exits: &mut Vec<usize>, enters: &mut Vec<usize>) {
        exits.clear();
        enters.clear();
        let depth = |n: usize| self.nodes.get(n).map_or(0, |n| n.depth);
        let parent = |n: usize| self.nodes.get(n).and_then(|n| n.parent);
        let mut a = from;
        let mut b = Some(to);
        // (Each step moves one of them up, so the loop ends after at most the two depths.)
        loop {
            match (a, b) {
                (Some(x), Some(y)) if x == y => break,
                (Some(x), Some(y)) => {
                    if depth(x) >= depth(y) {
                        exits.push(x);
                        a = parent(x);
                    } else {
                        enters.push(y);
                        b = parent(y);
                    }
                }
                (Some(x), None) => {
                    exits.push(x);
                    a = parent(x);
                }
                (None, Some(y)) => {
                    enters.push(y);
                    b = parent(y);
                }
                (None, None) => break,
            }
        }
        enters.reverse();
    }
}

/// What the run did, as the tracker sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// The run went into a node.
    Enter { node: usize, frame: usize },
    /// A loop took its next item (it is on the top of the stack).
    Iterate { node: usize, frame: usize },
    /// The run left a node, which left a value of this many items.
    Exit { node: usize, frame: usize, items: usize },
    /// An axis step selected this many nodes.
    Stepped { node: usize, nodes: usize },
}

/// Follows a run through the nodes of its expression.
#[derive(Debug, Default)]
pub(crate) struct Tracker {
    /// The innermost node of each frame (the expression's own first).
    frames: Vec<Option<usize>>,
    /// The instruction that ran before this one: what it was, and its frame.
    previous: Option<(Op, usize, Option<usize>)>,
    /// How many items the value the last instruction that is not ignored left has.
    last_items: usize,
    exits: Vec<usize>,
    enters: Vec<usize>,
    /// Instructions seen.
    pub instructions: u64,
}

/// How many items the top of the stack has (0 for an absent value).
pub(crate) fn top_items(at: &At<'_, '_>) -> usize {
    match at.stack().last() {
        Some(stack::Value::Sequence(s)) => s.len(),
        _ => 0,
    }
}

impl Tracker {
    /// The innermost node the run is in, in the frame it is in.
    pub(crate) fn current(&self) -> Option<usize> {
        self.frames.last().copied().flatten()
    }

    /// The frames, the expression's own first: the innermost node of each.
    pub(crate) fn frames(&self) -> &[Option<usize>] {
        &self.frames
    }

    /// See the instruction about to run, and say what happened since the last one into `events`.
    pub(crate) fn observe(&mut self, nodes: &Nodes, at: &At<'_, '_>, events: &mut Vec<Event>) -> Code {
        events.clear();
        self.instructions = self.instructions.saturating_add(1);
        let depth = at.depth();
        let top = top_items(at);
        // What the instruction that just ran did.
        if let Some((op, frame, node)) = self.previous {
            if op != Op::Ignored {
                self.last_items = top;
            }
            match (op, node) {
                (Op::SequenceGet, Some(node)) if frame == depth => events.push(Event::Iterate { node, frame }),
                (Op::Step, Some(node)) if frame == depth => events.push(Event::Stepped { node, nodes: top }),
                _ => {}
            }
        }
        // A function returned: its frame's nodes are left, with the value it returned.
        while self.frames.len() > depth {
            let frame = self.frames.len();
            if let Some(Some(inner)) = self.frames.pop() {
                let mut at_node = Some(inner);
                while let Some(n) = at_node {
                    events.push(Event::Exit { node: n, frame, items: top });
                    at_node = nodes.node(n).and_then(|n| n.parent);
                }
            }
            self.last_items = top;
        }
        // A function was called: a frame with no node yet.
        while self.frames.len() < depth {
            self.frames.push(None);
        }
        let code = nodes.code(at.function(), at.ip());
        if let Some(node) = code.node.map(|n| n as usize) {
            let from = self.current();
            if from != Some(node) {
                nodes.path(from, node, &mut self.exits, &mut self.enters);
                for &n in &self.exits {
                    events.push(Event::Exit { node: n, frame: depth, items: self.last_items });
                }
                for &n in &self.enters {
                    events.push(Event::Enter { node: n, frame: depth });
                }
                if let Some(slot) = self.frames.last_mut() {
                    *slot = Some(node);
                }
            }
        }
        self.previous = Some((code.op, depth, self.current()));
        code
    }

    /// The run ended: every node still entered is left with the value at the top of the stack (`items`).
    pub(crate) fn finish(&mut self, nodes: &Nodes, items: usize, events: &mut Vec<Event>) {
        events.clear();
        while let Some(inner) = self.frames.pop() {
            let frame = self.frames.len() + 1;
            let mut at_node = inner;
            while let Some(n) = at_node {
                events.push(Event::Exit { node: n, frame, items });
                at_node = nodes.node(n).and_then(|n| n.parent);
            }
        }
    }
}

/// A short excerpt of `text`: one line, at most `max` characters.
pub(crate) fn excerpt(text: &str, max: usize) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            space = count > 0;
            continue;
        }
        if count >= max {
            out.push('…');
            break;
        }
        if space {
            out.push(' ');
            count += 1;
            space = false;
        }
        out.push(c);
        count += 1;
    }
    out
}
