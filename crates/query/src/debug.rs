//! The debugger: an expression run on a worker thread that stops where it is asked to, and says what it is doing.
//!
//! The run is followed through the nodes of the expression (see [`crate::probe`]): it stops when it goes into a node (*enter*), when a
//! node that loops takes its next item (*iterate*: an axis step for each context item, a predicate for each item it tests, a `for`),
//! and, when stepping out, when it leaves one (*exit*). A stop says where the run is (the node's span in the expression), the context
//! item with its position and size, the variables read so far that are still alive, the call stack of inline functions, the items
//! the innermost sequence being built has so far, and the last events.
//!
//! - [`Action::Into`] stops at the next node gone into or item taken; [`Action::Over`] at the next one that is not inside the node
//!   stopped at; [`Action::Out`] at the next event outside it (the parent's next item or its end); [`Action::Continue`] at the next
//!   breakpoint. A breakpoint is a place in the expression: it stops at the innermost node that covers it (on each item, for a node
//!   that loops; else each time the run goes into it). Breakpoints stop the steps over and out too.
//! - [`Session::pause`] stops a running session at its next event, [`Session::stop`] (or dropping the session) ends the run.
//!   Nothing waits without a timeout: a paused worker wakes every 100 ms to see whether it was stopped, and ends on its own after an
//!   hour paused; the app's calls only lock for a copy of the state. A built-in function cannot be interrupted while it runs (its
//!   work is one instruction), so a stop takes effect when it returns.
//! - Variables are shown from their first read (the engine's code does not name the slots of its variables): a `for`, `some` or
//!   `every` variable from its first item.
//! - On the web there are no threads: [`Debugger::start`] says so.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use std::collections::{HashSet, VecDeque};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use xee_interpreter::error::Result as EngineResult;
use xee_interpreter::interpreter::hook::{At, Hook};
use xee_interpreter::sequence::Item;
use xee_interpreter::stack;

use crate::probe::{Event, Nodes, Op, Tracker, excerpt};
use crate::profile::stopped_error;
use crate::{Evaluated, Limits, Options, Phase, Prepared, QueryError, ResultItem, SourceMap};

/// The most events kept (the most recent).
pub const MAX_EVENTS: usize = 200;
/// The most items of a value shown (a variable, the context, the sequence being built).
pub const MAX_SHOWN: usize = 20;
/// The most variables shown.
const MAX_VARIABLES: usize = 50;
/// The most characters of an item's text.
const MAX_TEXT: usize = 400;
/// How often a paused worker looks whether it was stopped.
const TICK: Duration = Duration::from_millis(100);
/// How long a session stays paused before it ends on its own.
const MAX_PAUSE: Duration = Duration::from_secs(3600);

/// What to do from a stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Stop at the next node gone into or item taken.
    Into,
    /// Stop at the next one not inside the node stopped at.
    Over,
    /// Stop at the next event outside the node stopped at.
    Out,
    /// Run to the next breakpoint (or the end).
    Continue,
}

/// Where a session is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Running (between stops).
    Running,
    /// Stopped at an event, waiting for an [`Action`].
    Paused,
    /// The run ended (see [`DebugState::result`]).
    Finished,
    /// The run was ended by [`Session::stop`] (or its limit).
    Stopped,
}

/// What a run did at a stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// It went into a node.
    Enter,
    /// A node that loops took its next item.
    Iterate,
    /// It left a node.
    Exit,
}

/// One event of a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventRecord {
    pub kind: EventKind,
    /// The node's place in the expression (bytes).
    pub span: Range<usize>,
    /// The node's text, on one line and cut.
    pub text: String,
    /// The frame (1 for the expression, one more for each inline function called).
    pub frame: usize,
    /// How deep the node is in the expression.
    pub depth: usize,
    /// For an exit: the items of the value the node left.
    pub items: Option<usize>,
}

/// The context item at a stop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    pub item: ResultItem,
    /// `position()` and `last()`.
    pub position: Option<u64>,
    pub last: Option<u64>,
    /// The span of the step or predicate that made it the context (none for the document the expression starts from).
    pub from: Option<Range<usize>>,
}

/// A variable in scope at a stop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variable {
    /// `$x`.
    pub name: String,
    /// Its first items (at most [`MAX_SHOWN`]).
    pub items: Vec<ResultItem>,
    /// How many items it has.
    pub count: usize,
}

/// A frame of the call stack: the expression, or an inline function called from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallFrame {
    /// `the expression` or the function's text (`function($a) { $a + 1 }`).
    pub function: String,
    /// Where the function is defined in the expression.
    pub definition: Option<Range<usize>>,
    /// Where it was called from (in the frame below).
    pub call: Option<Range<usize>>,
    /// The node the frame is in.
    pub at: Option<Range<usize>>,
}

/// What a session shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebugState {
    pub status: Status,
    /// The event stopped at (when paused).
    pub at: Option<EventRecord>,
    pub context: Option<Context>,
    pub variables: Vec<Variable>,
    /// The innermost frame first.
    pub call_stack: Vec<CallFrame>,
    /// The items of the innermost sequence being built (a step's, a predicate's, a `for`'s), the first [`MAX_SHOWN`].
    pub partial: Vec<ResultItem>,
    pub partial_count: usize,
    /// The last events, the oldest first (at most [`MAX_EVENTS`]).
    pub events: Vec<EventRecord>,
    /// How many times the run stopped.
    pub stops: u64,
    /// The engine instructions run so far.
    pub instructions: u64,
    /// The spans the breakpoints resolved to.
    pub breakpoints: Vec<Range<usize>>,
    /// What the run returned, once it ended.
    pub result: Option<Result<Evaluated, QueryError>>,
}

impl Default for DebugState {
    fn default() -> Self {
        Self {
            status: Status::Running,
            at: None,
            context: None,
            variables: Vec::new(),
            call_stack: Vec::new(),
            partial: Vec::new(),
            partial_count: 0,
            events: Vec::new(),
            stops: 0,
            instructions: 0,
            breakpoints: Vec::new(),
            result: None,
        }
    }
}

/// How a session starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// Places in the expression (byte offsets) to stop at.
    pub breakpoints: Vec<usize>,
    /// Stop at the first event (else run to the first breakpoint).
    pub stop_at_start: bool,
    pub limits: Limits,
}

impl Default for Settings {
    fn default() -> Self {
        Self { breakpoints: Vec::new(), stop_at_start: true, limits: Limits { max_instructions: 2_000_000_000 } }
    }
}

#[derive(Default)]
struct Inner {
    state: DebugState,
    command: Option<Action>,
    /// New breakpoints, for the worker to take at its next stop.
    breakpoints: Option<Vec<usize>>,
    worker_done: bool,
}

#[derive(Default)]
struct Shared {
    inner: Mutex<Inner>,
    changed: Condvar,
    stop: AtomicBool,
    pause: AtomicBool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A debugging session: the run on its worker thread, and what it shows.
pub struct Session {
    shared: Arc<Shared>,
    #[cfg(not(target_arch = "wasm32"))]
    handle: Option<std::thread::JoinHandle<()>>,
}

/// Starts debugging sessions.
pub struct Debugger;

impl Debugger {
    /// Start debugging `expression` on the document `xml`. An expression that does not compile is an error at once; anything else
    /// (a document that cannot be read, an error while running) ends the session with that result.
    pub fn start(xml: &str, expression: &str, options: &Options, settings: Settings) -> Result<Session, QueryError> {
        crate::compile(expression, options)?;
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (xml, settings);
            Err(QueryError {
                phase: Phase::Run,
                code: String::new(),
                message: "the debugger needs a thread to run on, and the web version has none".to_owned(),
                span: None,
            })
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let shared = Arc::new(Shared::default());
            let worker = shared.clone();
            let (xml, expression, options) = (xml.to_owned(), expression.to_owned(), options.clone());
            let handle = std::thread::Builder::new()
                .name("xpath debugger".into())
                .spawn(move || work(&worker, &xml, &expression, &options, &settings))
                .map_err(|e| QueryError {
                    phase: Phase::Run,
                    code: String::new(),
                    message: format!("the debugger could not start: {e}"),
                    span: None,
                })?;
            Ok(Session { shared, handle: Some(handle) })
        }
    }
}

impl Session {
    /// Go on from a stop. False when the session is not paused (running or ended).
    pub fn step(&self, action: Action) -> bool {
        let mut inner = self.shared.lock();
        if inner.state.status != Status::Paused {
            return false;
        }
        inner.command = Some(action);
        inner.state.status = Status::Running;
        drop(inner);
        self.shared.changed.notify_all();
        true
    }

    /// Stop a running session at its next event.
    pub fn pause(&self) {
        self.shared.pause.store(true, Ordering::SeqCst);
    }

    /// End the run.
    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        let mut inner = self.shared.lock();
        // A paused run is ending now: [`Session::wait`] waits for its end.
        if inner.state.status == Status::Paused {
            inner.state.status = Status::Running;
        }
        drop(inner);
        self.shared.changed.notify_all();
    }

    /// Replace the breakpoints (byte offsets in the expression); they apply from the next stop.
    pub fn set_breakpoints(&self, offsets: Vec<usize>) {
        self.shared.lock().breakpoints = Some(offsets);
    }

    /// A copy of what the session shows now.
    pub fn state(&self) -> DebugState {
        self.shared.lock().state.clone()
    }

    /// Wait until the session is not running (paused or ended), at most `timeout`; then what it shows.
    pub fn wait(&self, timeout: Duration) -> DebugState {
        let started = std::time::Instant::now();
        let mut inner = self.shared.lock();
        while inner.state.status == Status::Running {
            let Some(left) = timeout.checked_sub(started.elapsed()) else { break };
            inner = self.shared.changed.wait_timeout(inner, left.min(TICK)).unwrap_or_else(PoisonError::into_inner).0;
        }
        inner.state.clone()
    }

    /// The worker thread has ended.
    pub fn is_done(&self) -> bool {
        self.shared.lock().worker_done
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
        #[cfg(not(target_arch = "wasm32"))]
        {
            // The worker sees the stop at its next instruction or tick; wait a little for it, and leave it to end on its own if a
            // built-in function is still busy (it ends when that returns).
            let started = std::time::Instant::now();
            let mut inner = self.shared.lock();
            while !inner.worker_done && started.elapsed() < Duration::from_secs(1) {
                inner = self.shared.changed.wait_timeout(inner, TICK).unwrap_or_else(PoisonError::into_inner).0;
            }
            let done = inner.worker_done;
            drop(inner);
            if done && let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn work(shared: &Arc<Shared>, xml: &str, expression: &str, options: &Options, settings: &Settings) {
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(shared, xml, expression, options, settings)));
    let mut inner = shared.lock();
    if run.is_err() {
        inner.state.status = Status::Finished;
        inner.state.result = Some(Err(QueryError {
            phase: Phase::Run,
            code: String::new(),
            message: "the debugged run failed (a bug); nothing was changed".to_owned(),
            span: None,
        }));
    }
    inner.worker_done = true;
    drop(inner);
    shared.changed.notify_all();
}

fn run(shared: &Arc<Shared>, xml: &str, expression: &str, options: &Options, settings: &Settings) {
    let finish = |result: Result<Evaluated, QueryError>, stepper: Option<&Stepper<'_>>| {
        let mut inner = shared.lock();
        let stopped = shared.stop.load(Ordering::SeqCst) || stepper.is_some_and(|s| s.over_limit);
        inner.state.status = if stopped { Status::Stopped } else { Status::Finished };
        inner.state.at = None;
        if let Some(s) = stepper {
            inner.state.events = s.ring.iter().cloned().collect();
            inner.state.instructions = s.tracker.instructions;
            inner.state.stops = s.stops;
        }
        inner.state.result = Some(if stepper.is_some_and(|s| s.over_limit) {
            Err(QueryError {
                phase: Phase::Run,
                code: String::new(),
                message: format!("the run was stopped after {} instructions (the limit)", settings.limits.max_instructions),
                span: None,
            })
        } else if stopped {
            Err(QueryError { phase: Phase::Run, code: String::new(), message: "the run was stopped".to_owned(), span: None })
        } else {
            result
        });
        drop(inner);
        shared.changed.notify_all();
    };
    let mut prepared = match Prepared::new(xml) {
        Ok(p) => p,
        Err(e) => return finish(Err(e), None),
    };
    let debug_options = Options { max_text: MAX_TEXT, max_items: MAX_SHOWN, ..options.clone() };
    let outcome = prepared.run_hooked(expression, options, |program, map, source| {
        let nodes = Nodes::of(program, expression);
        let mut stepper = Stepper {
            tracker: Tracker::default(),
            events: Vec::new(),
            mode: if settings.stop_at_start { Mode::Into } else { Mode::Continue },
            breakpoints: HashSet::new(),
            ring: VecDeque::new(),
            loops: Vec::new(),
            reads: Vec::new(),
            stops: 0,
            over_limit: false,
            limit: settings.limits.max_instructions,
            shared: shared.clone(),
            map,
            source,
            expression,
            options: debug_options,
            nodes,
        };
        stepper.set_breakpoints(&settings.breakpoints);
        stepper
    });
    match outcome {
        Ok((result, stepper)) => finish(result, Some(&stepper)),
        Err(e) => finish(Err(e), None),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Into,
    Over((usize, usize)),
    Out((usize, usize)),
    Continue,
}

/// A loop's current item: the context item of a step or predicate, or the variable of a `for`, `some` or `every`.
struct Loop {
    frame: usize,
    /// The stack slot of `last` (the loop is over once the stack is no longer above it).
    last_slot: usize,
    node: usize,
    item: Item,
    position: Option<u64>,
    last: Option<u64>,
    variable: Option<String>,
}

/// A variable the run read: where it lives.
enum Read {
    Local { frame: usize, slot: usize, name: u32 },
    Closure { frame: usize, index: u16, name: u32 },
}

struct Stepper<'p> {
    nodes: Nodes,
    tracker: Tracker,
    events: Vec<Event>,
    mode: Mode,
    breakpoints: HashSet<usize>,
    ring: VecDeque<EventRecord>,
    loops: Vec<Loop>,
    reads: Vec<Read>,
    stops: u64,
    over_limit: bool,
    limit: u64,
    shared: Arc<Shared>,
    map: &'p SourceMap,
    source: &'p str,
    expression: &'p str,
    options: Options,
}

/// The names a `for`, `some` or `every` binds, in order (`$a` and `$b` of `for $a in …, $b in …`), at the outermost level of its text.
fn binding_names(text: &str) -> Vec<String> {
    let t = text.trim_start();
    if !["for", "some", "every"].iter().any(|k| t.strip_prefix(k).is_some_and(|rest| rest.starts_with(|c: char| c.is_whitespace() || c == '$'))) {
        return Vec::new();
    }
    let mut names = Vec::new();
    let mut depth = 0usize;
    let mut chars = t.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '(' | '[' | '{' => depth = depth.saturating_add(1),
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            '$' if depth == 0 => {
                let rest = t.get(i + 1..).unwrap_or_default();
                let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':')).collect();
                let after = rest.get(name.len()..).unwrap_or_default().trim_start();
                if !name.is_empty()
                    && after.starts_with("in")
                    && after.get(2..).is_some_and(|r| r.starts_with(|c: char| c.is_whitespace() || c == '('))
                {
                    names.push(format!("${name}"));
                }
            }
            _ => {}
        }
        let _ = chars.peek();
    }
    names
}

fn number(item: Option<Item>) -> Option<u64> {
    match item {
        Some(Item::Atomic(a)) => a.xpath_representation().parse().ok(),
        _ => None,
    }
}

impl Stepper<'_> {
    fn set_breakpoints(&mut self, offsets: &[usize]) {
        self.breakpoints = offsets.iter().filter_map(|&o| self.nodes.at_offset(o)).collect();
    }

    fn record(&self, event: &Event) -> Option<EventRecord> {
        let (kind, node, frame, items) = match *event {
            Event::Enter { node, frame } => (EventKind::Enter, node, frame, None),
            Event::Iterate { node, frame } => (EventKind::Iterate, node, frame, None),
            Event::Exit { node, frame, items } => (EventKind::Exit, node, frame, Some(items)),
            Event::Stepped { .. } => return None,
        };
        let n = self.nodes.node(node)?;
        Some(EventRecord {
            kind,
            span: n.span.clone(),
            text: excerpt(self.expression.get(n.span.clone()).unwrap_or_default(), 80),
            frame,
            depth: n.depth,
            items,
        })
    }

    fn describe(&self, at: &At<'_, '_>, item: &Item) -> ResultItem {
        crate::describe(item, at.xot(), self.map, &self.options, self.source, Some(at.dynamic_context()), crate::Owner::All)
    }

    fn value(&self, at: &At<'_, '_>, value: Option<&stack::Value>) -> (Vec<ResultItem>, usize) {
        match value {
            Some(stack::Value::Sequence(s)) => (s.iter().take(MAX_SHOWN).map(|i| self.describe(at, &i)).collect(), s.len()),
            _ => (Vec::new(), 0),
        }
    }

    /// Forget the loops and variables whose frame returned or whose slots are gone.
    fn prune(&mut self, at: &At<'_, '_>) {
        let (depth, height) = (at.depth(), at.stack().len());
        self.loops.retain(|l| l.frame < depth || (l.frame == depth && l.last_slot < height));
        self.reads.retain(|r| match *r {
            Read::Local { frame, slot, .. } => frame < depth || (frame == depth && slot < height),
            Read::Closure { frame, .. } => frame <= depth,
        });
    }

    /// What the stop shows.
    fn snapshot(&self, at: &At<'_, '_>, event: EventRecord) -> DebugState {
        let depth = at.depth();
        let stack = at.stack();
        let context = match self.loops.iter().rev().find(|l| l.frame == depth && l.variable.is_none()) {
            Some(l) => Some(Context {
                item: self.describe(at, &l.item),
                position: l.position,
                last: l.last,
                from: self.nodes.node(l.node).map(|n| n.span.clone()),
            }),
            // The expression starts with the document as its context: the bottom of the stack.
            None if depth == 1 => match stack.first() {
                Some(stack::Value::Sequence(s)) => {
                    s.get(0).map(|item| Context { item: self.describe(at, &item), position: Some(1), last: Some(1), from: None })
                }
                _ => None,
            },
            None => None,
        };
        let mut variables: Vec<Variable> = Vec::new();
        let mut add = |name: String, items: Vec<ResultItem>, count: usize| {
            variables.retain(|v| v.name != name);
            if variables.len() < MAX_VARIABLES {
                variables.push(Variable { name, items, count });
            }
        };
        for l in self.loops.iter().filter(|l| l.frame == depth) {
            if let Some(name) = &l.variable {
                add(name.clone(), vec![self.describe(at, &l.item)], 1);
            }
        }
        for read in &self.reads {
            match *read {
                Read::Local { frame, slot, name } if frame == depth => {
                    let (items, count) = self.value(at, stack.get(slot));
                    add(self.nodes.names.get(name as usize).cloned().unwrap_or_default(), items, count);
                }
                Read::Closure { frame, index, name } if frame == depth => {
                    let value = at.closure_value(usize::from(index));
                    let (items, count) = self.value(at, value.as_ref());
                    add(self.nodes.names.get(name as usize).cloned().unwrap_or_default(), items, count);
                }
                _ => {}
            }
        }
        let frames = at.frames();
        let span_of = |node: Option<usize>| node.and_then(|n| self.nodes.node(n)).map(|n| n.span.clone());
        let mut call_stack: Vec<CallFrame> = frames
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let definition = if i == 0 { None } else { self.nodes.definitions.get(f.function).cloned().flatten() };
                let function = match &definition {
                    None => "the expression".to_owned(),
                    Some(span) => excerpt(self.expression.get(span.clone()).unwrap_or("function"), 60),
                };
                let call = i
                    .checked_sub(1)
                    .and_then(|below| frames.get(below))
                    .and_then(|caller| at.span_of(caller.function, caller.ip.checked_sub(1)?))
                    .map(|s| s.range())
                    .filter(|s| !s.is_empty());
                CallFrame { function, definition, call, at: span_of(self.tracker.frames().get(i).copied().flatten()) }
            })
            .collect();
        call_stack.reverse();
        let builds = at.builds();
        let (partial, partial_count) = match builds.last() {
            Some(items) => (items.iter().take(MAX_SHOWN).map(|i| self.describe(at, i)).collect(), items.len()),
            None => (Vec::new(), 0),
        };
        DebugState {
            status: Status::Paused,
            at: Some(event),
            context,
            variables,
            call_stack,
            partial,
            partial_count,
            events: self.ring.iter().cloned().collect(),
            stops: self.stops,
            instructions: self.tracker.instructions,
            breakpoints: self.breakpoints.iter().filter_map(|&n| self.nodes.node(n)).map(|n| n.span.clone()).collect(),
            result: None,
        }
    }

    fn wants_stop(&self, event: &Event, record: &EventRecord) -> bool {
        let node = match *event {
            Event::Enter { node, .. } | Event::Iterate { node, .. } | Event::Exit { node, .. } => node,
            Event::Stepped { .. } => return false,
        };
        let is_loop = self.nodes.node(node).is_some_and(|n| n.is_loop);
        let breakpoint = self.breakpoints.contains(&node)
            && match record.kind {
                EventKind::Iterate => is_loop,
                EventKind::Enter => !is_loop,
                EventKind::Exit => false,
            };
        let level = (record.frame, record.depth);
        let stepping = match self.mode {
            Mode::Into => record.kind != EventKind::Exit,
            Mode::Over(at) => record.kind != EventKind::Exit && level <= at,
            Mode::Out(at) => level < at,
            Mode::Continue => false,
        };
        let paused = record.kind != EventKind::Exit && self.shared.pause.load(Ordering::SeqCst);
        breakpoint || stepping || paused
    }

    /// Publish the stop and wait for what to do. An error ends the run (stopped, or paused for too long).
    fn stop_here(&mut self, at: &At<'_, '_>, record: EventRecord) -> EngineResult<()> {
        self.stops = self.stops.saturating_add(1);
        self.shared.pause.store(false, Ordering::SeqCst);
        let level = (record.frame, record.depth);
        let snapshot = self.snapshot(at, record);
        let mut inner = self.shared.lock();
        inner.state = snapshot;
        inner.command = None;
        self.shared.changed.notify_all();
        let started = std::time::Instant::now();
        let action = loop {
            if self.shared.stop.load(Ordering::SeqCst) || started.elapsed() > MAX_PAUSE {
                return Err(stopped_error("the debugged run was stopped"));
            }
            if let Some(action) = inner.command.take() {
                break action;
            }
            inner = self.shared.changed.wait_timeout(inner, TICK).unwrap_or_else(PoisonError::into_inner).0;
        };
        let breakpoints = inner.breakpoints.take();
        inner.state.status = Status::Running;
        drop(inner);
        self.shared.changed.notify_all();
        if let Some(offsets) = breakpoints {
            self.set_breakpoints(&offsets);
        }
        self.mode = match action {
            Action::Into => Mode::Into,
            Action::Over => Mode::Over(level),
            Action::Out => Mode::Out(level),
            Action::Continue => Mode::Continue,
        };
        Ok(())
    }
}

impl Hook for Stepper<'_> {
    fn before(&mut self, at: &At<'_, '_>) -> EngineResult<()> {
        if self.shared.stop.load(Ordering::Relaxed) {
            return Err(stopped_error("the debugged run was stopped"));
        }
        if self.tracker.instructions >= self.limit {
            self.over_limit = true;
            return Err(stopped_error("the debugged run was stopped: it ran more instructions than its limit"));
        }
        let mut events = std::mem::take(&mut self.events);
        let code = self.tracker.observe(&self.nodes, at, &mut events);
        self.prune(at);
        let depth = at.depth();
        match code.op {
            Op::Var(slot, name) => {
                let slot = at.base().saturating_add(usize::from(slot));
                self.reads.retain(|r| !matches!(*r, Read::Local { frame, slot: s, .. } if frame == depth && s == slot));
                self.reads.push(Read::Local { frame: depth, slot, name });
            }
            Op::ClosureVar(index, name) => {
                self.reads.retain(|r| !matches!(*r, Read::Closure { frame, index: i, .. } if frame == depth && i == index));
                self.reads.push(Read::Closure { frame: depth, index, name });
            }
            _ => {}
        }
        let mut result = Ok(());
        for event in &events {
            if let Event::Iterate { node, frame } = *event {
                // (The loop's `last`, `position` and item are the top three values: see the engine's loops.)
                let stack = at.stack();
                let height = stack.len();
                let get = |back: usize| match height.checked_sub(back).and_then(|i| stack.get(i)) {
                    Some(stack::Value::Sequence(s)) => s.get(0),
                    _ => None,
                };
                if let (Some(item), Some(last_slot)) = (get(1), height.checked_sub(3)) {
                    let text = self.nodes.node(node).and_then(|n| self.expression.get(n.span.clone())).unwrap_or_default();
                    let names = binding_names(text);
                    let variable = if names.is_empty() {
                        None
                    } else {
                        let nested = self.loops.iter().filter(|l| l.frame == frame && l.node == node && l.last_slot < last_slot).count();
                        names.get(nested).or(names.last()).cloned()
                    };
                    self.loops.retain(|l| !(l.frame == frame && l.last_slot == last_slot));
                    self.loops.push(Loop { frame, last_slot, node, item, position: number(get(2)), last: number(get(3)), variable });
                }
            }
            let Some(record) = self.record(event) else { continue };
            if self.ring.len() >= MAX_EVENTS {
                self.ring.pop_front();
            }
            self.ring.push_back(record.clone());
            if self.wants_stop(event, &record)
                && let Err(e) = self.stop_here(at, record)
            {
                result = Err(e);
                break;
            }
        }
        self.events = events;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::binding_names;

    #[test]
    fn the_names_a_for_binds_are_found_at_its_outermost_level() {
        assert_eq!(binding_names("for $x in 1 to 3 return $x"), ["$x"]);
        assert_eq!(binding_names("for $a in (1, 2), $b in (for $c in 1 return $c) return $a + $b"), ["$a", "$b"]);
        assert_eq!(binding_names("some $i in //item satisfies $i/price > 3"), ["$i"]);
        assert!(binding_names("//item[$x in 1]").is_empty(), "not a for");
        assert!(binding_names("format-number(1, '0')").is_empty());
    }
}
