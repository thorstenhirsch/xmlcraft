//! `xpath_debug_start`, `xpath_debug_step`, `xpath_debug_state` and `xpath_debug_stop`: the XPath debugger (`xmlcraft_query::debug`)
//! driven by tools. A session runs on its own worker thread and is held by the automation session (at most [`MAX_SESSIONS`]; an ended
//! one is replaced by a new one), and ends with `xpath_debug_stop` or when the automation session ends. Nothing is changed.

use std::time::Duration;

use serde_json::{Value, json};
use xmlcraft_query::ResultItem;
use xmlcraft_query::debug::{Action, CallFrame, DebugState, Debugger, EventKind, EventRecord, Session, Settings, Status};
use xmlcraft_text::Buffer;

use crate::documents::{fail, json_result};
use crate::tools::ToolDef;
use crate::xpath::{Asked, asked, common_properties, error_text, item_json, items_json};
use crate::{Automation, Content, Result, ToolError};

/// The most sessions held at once.
pub(crate) const MAX_SESSIONS: usize = 4;
/// How long a call waits for the run to stop, by default and at most (milliseconds).
const WAIT_MS: u64 = 5_000;
const MAX_WAIT_MS: u64 = 60_000;

/// The debugging sessions of an automation session, by id.
#[derive(Default)]
pub(crate) struct Sessions {
    next: u64,
    held: Vec<(u64, Session, String)>,
}

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut start = common_properties();
    if let Some(map) = start.as_object_mut() {
        map.insert(
            "breakpoints".into(),
            json!({ "type": "array", "items": { "type": "integer", "minimum": 0 }, "maxItems": 100, "description": "Places in the expression (byte offsets) to stop at: the innermost part of the expression there (on each item for a step, a predicate or a `for`)" }),
        );
        map.insert(
            "stop_at_start".into(),
            json!({ "type": "boolean", "description": "Stop at the first event (default true); false runs to the first breakpoint" }),
        );
        map.insert("wait_ms".into(), wait_property());
    }
    let session = json!({ "type": "integer", "minimum": 1, "description": "The session (from xpath_debug_start)" });
    vec![
        ToolDef {
            name: "xpath_debug_start",
            title: "Start debugging an XPath expression",
            description: "Start running an XPath 3.1 expression in the debugger, on its own thread, and wait (at most `wait_ms`) until it stops: at its first event, or \
                          at a breakpoint when `stop_at_start` is false. Returns the `session` id and the state (see xpath_debug_state). At most 4 sessions are \
                          held; end one with xpath_debug_stop.",
            input_schema: json!({ "type": "object", "properties": start, "required": ["expression"], "additionalProperties": false }),
            read_only: true,
            destructive: false,
            handler: xpath_debug_start,
        },
        ToolDef {
            name: "xpath_debug_step",
            title: "Step the XPath debugger",
            description: "Go on from a stop and wait (at most `wait_ms`) for the next: `into` stops at the next part of the expression gone into or item taken, `over` at \
                          the next one not inside the part stopped at, `out` at the next event outside it (the parent's next item or its end), `continue` at the \
                          next breakpoint, `pause` stops a running session at its next event. `breakpoints` replaces the breakpoints. Returns the state.",
            input_schema: json!({ "type": "object", "properties": {
                "session": session,
                "action": { "type": "string", "enum": ["into", "over", "out", "continue", "pause"] },
                "breakpoints": { "type": "array", "items": { "type": "integer", "minimum": 0 }, "maxItems": 100, "description": "New breakpoints (byte offsets), from the next stop" },
                "wait_ms": wait_property(),
            }, "required": ["session", "action"], "additionalProperties": false }),
            read_only: true,
            destructive: false,
            handler: xpath_debug_step,
        },
        ToolDef {
            name: "xpath_debug_state",
            title: "What the XPath debugger shows",
            description: "The state of a session: `status` (running, paused, finished, stopped), `at` (the event stopped at: enter, iterate or exit, with the `span` \
                          of the part of the expression in bytes and its `text`), `context` (the context item with its `position` and `last`), `variables` (read \
                          so far and still alive), `call_stack` (the innermost frame first), `partial` (the items of the innermost sequence being built), the \
                          last `events`, and once the run ended its `result` as xpath_eval gives it.",
            input_schema: json!({ "type": "object", "properties": { "session": session }, "required": ["session"], "additionalProperties": false }),
            read_only: true,
            destructive: false,
            handler: xpath_debug_state,
        },
        ToolDef {
            name: "xpath_debug_stop",
            title: "Stop the XPath debugger",
            description: "End a session: the run is stopped and the session forgotten.",
            input_schema: json!({ "type": "object", "properties": { "session": session }, "required": ["session"], "additionalProperties": false }),
            read_only: true,
            destructive: false,
            handler: xpath_debug_stop,
        },
    ]
}

fn wait_property() -> Value {
    json!({ "type": "integer", "minimum": 0, "maximum": MAX_WAIT_MS, "description": "How long to wait for the run to stop, in milliseconds (default 5000)" })
}

fn wait_arg(args: &Value) -> Result<Duration> {
    match args.get("wait_ms") {
        None => Ok(Duration::from_millis(WAIT_MS)),
        Some(v) => v
            .as_u64()
            .filter(|n| *n <= MAX_WAIT_MS)
            .map(Duration::from_millis)
            .ok_or_else(|| ToolError::InvalidArgs(format!("wait_ms is a whole number from 0 to {MAX_WAIT_MS}"))),
    }
}

fn breakpoints_arg(args: &Value) -> Result<Option<Vec<usize>>> {
    match args.get("breakpoints") {
        None => Ok(None),
        Some(Value::Array(list)) => list
            .iter()
            .map(|v| v.as_u64().and_then(|n| usize::try_from(n).ok()).ok_or_else(|| ToolError::InvalidArgs("a breakpoint is a byte offset".into())))
            .collect::<Result<Vec<_>>>()
            .map(Some),
        Some(_) => Err(ToolError::InvalidArgs("breakpoints is a list of byte offsets".into())),
    }
}

fn session_arg(args: &Value) -> Result<u64> {
    args.get("session").and_then(Value::as_u64).ok_or_else(|| ToolError::InvalidArgs("`session` is required (from xpath_debug_start)".into()))
}

fn status_name(status: &Status) -> &'static str {
    match status {
        Status::Running => "running",
        Status::Paused => "paused",
        Status::Finished => "finished",
        Status::Stopped => "stopped",
    }
}

fn event_json(e: &EventRecord) -> Value {
    let kind = match e.kind {
        EventKind::Enter => "enter",
        EventKind::Iterate => "iterate",
        EventKind::Exit => "exit",
    };
    let mut v = json!({ "kind": kind, "span": [e.span.start, e.span.end], "text": e.text, "frame": e.frame, "depth": e.depth });
    if let Some(items) = e.items {
        v["items"] = json!(items);
    }
    v
}

fn span_json(span: Option<&std::ops::Range<usize>>) -> Value {
    span.map_or(Value::Null, |s| json!([s.start, s.end]))
}

fn frame_json(f: &CallFrame) -> Value {
    json!({ "function": f.function, "definition": span_json(f.definition.as_ref()), "call": span_json(f.call.as_ref()), "at": span_json(f.at.as_ref()) })
}

fn state_json(id: u64, state: &DebugState, text: &str) -> Value {
    let buffer = Buffer::from_text(text);
    let items = |list: &[ResultItem]| list.iter().map(|i| item_json(i, &buffer)).collect::<Vec<_>>();
    let mut v = json!({
        "session": id,
        "status": status_name(&state.status),
        "at": state.at.as_ref().map_or(Value::Null, event_json),
        "context": state.context.as_ref().map_or(Value::Null, |c| json!({
            "item": item_json(&c.item, &buffer),
            "position": c.position,
            "last": c.last,
            "from": span_json(c.from.as_ref()),
        })),
        "variables": state.variables.iter().map(|v| json!({ "name": v.name, "count": v.count, "items": items(&v.items) })).collect::<Vec<_>>(),
        "call_stack": state.call_stack.iter().map(frame_json).collect::<Vec<_>>(),
        "partial": { "count": state.partial_count, "items": items(&state.partial) },
        "events": state.events.iter().map(event_json).collect::<Vec<_>>(),
        "stops": state.stops,
        "instructions": state.instructions,
        "breakpoints": state.breakpoints.iter().map(|s| json!([s.start, s.end])).collect::<Vec<_>>(),
    });
    match &state.result {
        None => {}
        Some(Ok(done)) => v["result"] = json!({ "count": done.count, "items": items_json(done, text) }),
        Some(Err(e)) => v["error"] = json!(error_text(e)),
    }
    v
}

impl Sessions {
    fn get(&self, id: u64) -> Result<&(u64, Session, String)> {
        self.held
            .iter()
            .find(|(i, _, _)| *i == id)
            .ok_or_else(|| ToolError::Failed(format!("no debugging session has the id {id} (it ended, or was never started)")))
    }
}

fn xpath_debug_start(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let Asked { text, expression, options } = asked(a, args, "xpath_debug_start")?;
    let wait = wait_arg(args)?;
    let stop_at_start = match args.get("stop_at_start") {
        None => true,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(ToolError::InvalidArgs("stop_at_start is true or false".into())),
    };
    let breakpoints = breakpoints_arg(args)?.unwrap_or_default();
    // An ended session gives way to a new one; four that are still running or paused do not.
    if a.debug.held.len() >= MAX_SESSIONS {
        match a.debug.held.iter().position(|(_, s, _)| matches!(s.state().status, Status::Finished | Status::Stopped)) {
            Some(i) => {
                a.debug.held.remove(i);
            }
            None => return fail(format!("{MAX_SESSIONS} debugging sessions are open: stop one first (xpath_debug_stop)")),
        }
    }
    let settings = Settings { breakpoints, stop_at_start, ..Settings::default() };
    let session = Debugger::start(&text, &expression, &options, settings).map_err(|e| ToolError::Failed(error_text(&e)))?;
    let state = session.wait(wait);
    a.debug.next = a.debug.next.saturating_add(1);
    let id = a.debug.next;
    let answer = state_json(id, &state, &text);
    a.debug.held.push((id, session, text));
    json_result(answer)
}

fn xpath_debug_step(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let id = session_arg(args)?;
    let wait = wait_arg(args)?;
    let breakpoints = breakpoints_arg(args)?;
    let action = match args.get("action").and_then(Value::as_str) {
        Some("into") => Some(Action::Into),
        Some("over") => Some(Action::Over),
        Some("out") => Some(Action::Out),
        Some("continue") => Some(Action::Continue),
        Some("pause") => None,
        _ => return Err(ToolError::InvalidArgs("action is into, over, out, continue or pause".into())),
    };
    let (_, session, text) = a.debug.get(id)?;
    if let Some(offsets) = breakpoints {
        session.set_breakpoints(offsets);
    }
    match action {
        Some(action) => {
            if !session.step(action) {
                let state = session.state();
                return fail(format!("the session is {}, not paused: nothing to step", status_name(&state.status)));
            }
        }
        None => session.pause(),
    }
    let state = session.wait(wait);
    json_result(state_json(id, &state, text))
}

fn xpath_debug_state(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let id = session_arg(args)?;
    let (_, session, text) = a.debug.get(id)?;
    json_result(state_json(id, &session.state(), text))
}

fn xpath_debug_stop(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let id = session_arg(args)?;
    a.debug.get(id)?;
    a.debug.held.retain(|(i, session, _)| {
        if *i == id {
            session.stop();
        }
        *i != id
    });
    json_result(json!({ "session": id, "stopped": true }))
}
