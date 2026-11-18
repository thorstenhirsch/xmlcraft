//! The debugger: stepping, breakpoints, the state at a stop, and sessions that end when asked.

use std::time::{Duration, Instant};

use xmlcraft_query::Options;
use xmlcraft_query::debug::{Action, DebugState, Debugger, EventKind, Session, Settings, Status};

const WAIT: Duration = Duration::from_secs(10);

/// Three items costing 20, 5 and 30.
const SHOP: &str = "<shop><item><price>20</price><name>a</name></item><item><price>5</price><name>b</name></item><item><price>30</price><name>c</name></item></shop>";
const EXPRESSION: &str = "//item[price > 10]/name";

fn start(expression: &str, settings: Settings) -> Session {
    Debugger::start(SHOP, expression, &Options::default(), settings).expect("compiles")
}

fn at_text(state: &DebugState) -> String {
    state.at.as_ref().map(|a| a.text.clone()).unwrap_or_default()
}

fn kind(state: &DebugState) -> Option<EventKind> {
    state.at.as_ref().map(|a| a.kind)
}

/// Step with `action` until the run ends; every stop on the way.
fn run_through(session: &Session, action: Action) -> (Vec<DebugState>, DebugState) {
    let mut stops = Vec::new();
    let mut state = session.wait(WAIT);
    while state.status == Status::Paused {
        assert!(stops.len() < 2000, "too many stops");
        stops.push(state.clone());
        assert!(session.step(action));
        state = session.wait(WAIT);
    }
    (stops, state)
}

#[test]
fn stepping_into_visits_the_nodes_in_order_and_the_loops_once_per_item() {
    let session = start(EXPRESSION, Settings::default());
    let (stops, end) = run_through(&session, Action::Into);
    assert_eq!(end.status, Status::Finished);
    let result = end.result.expect("a result").expect("runs");
    assert_eq!(result.items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["<name>a</name>", "<name>c</name>"]);
    let first = &stops[0];
    assert_eq!((at_text(first), kind(first)), (EXPRESSION.to_owned(), Some(EventKind::Enter)), "the run starts in the whole expression");
    let context = first.context.as_ref().expect("a context");
    assert_eq!((context.item.kind.as_str(), context.position, context.last), ("document", Some(1), Some(1)));
    // The predicate is tested on each item, as the context item, in order.
    let tested: Vec<(String, Option<u64>)> = stops
        .iter()
        .filter(|s| at_text(s) == "price > 10" && kind(s) == Some(EventKind::Iterate))
        .map(|s| {
            let c = s.context.as_ref().expect("a context");
            (c.item.path.clone().unwrap_or_default(), c.position)
        })
        .collect();
    assert_eq!(
        tested,
        [("/shop[1]/item[1]".to_owned(), Some(1)), ("/shop[1]/item[2]".to_owned(), Some(2)), ("/shop[1]/item[3]".to_owned(), Some(3))]
    );
    // After each item comes its price, then 10: the operands in order. (The predicate is gone into once for each node `//` gives,
    // and the first one, the document, has no item children: nothing to test.)
    let first = stops.iter().position(|s| at_text(s) == "price > 10" && kind(s) == Some(EventKind::Iterate)).expect("an item tested");
    assert!(stops[..first].iter().any(|s| at_text(s) == "price > 10" && kind(s) == Some(EventKind::Enter)));
    let next: Vec<String> = stops[first + 1..first + 3].iter().map(at_text).collect();
    assert_eq!(next, ["price", "10"]);
    let named: Vec<String> = stops
        .iter()
        .filter(|s| at_text(s) == "name" && kind(s) == Some(EventKind::Iterate))
        .map(|s| s.context.as_ref().and_then(|c| c.item.path.clone()).unwrap_or_default())
        .collect();
    assert_eq!(named, ["/shop[1]/item[1]", "/shop[1]/item[3]"], "the last step runs for the items that passed");
    assert!(end.events.len() <= xmlcraft_query::debug::MAX_EVENTS && !end.events.is_empty());
    assert!(session.is_done() || session.wait(WAIT).status == Status::Finished);
}

#[test]
fn a_breakpoint_on_the_predicate_stops_once_per_item_and_shows_what_was_kept_so_far() {
    let at = EXPRESSION.find('>').expect("a >");
    let session = start(EXPRESSION, Settings { breakpoints: vec![at], stop_at_start: false, ..Settings::default() });
    let (stops, end) = run_through(&session, Action::Continue);
    assert_eq!(end.status, Status::Finished);
    assert_eq!(stops.len(), 3, "{:?}", stops.iter().map(at_text).collect::<Vec<_>>());
    for (i, s) in stops.iter().enumerate() {
        assert_eq!((at_text(s), kind(s)), ("price > 10".to_owned(), Some(EventKind::Iterate)));
        assert_eq!(s.context.as_ref().and_then(|c| c.position), Some(i as u64 + 1));
        assert_eq!((s.breakpoints.len(), s.breakpoints.first()), (1, Some(&(7..17))));
    }
    assert_eq!(stops[2].partial_count, 1, "the first item passed, the second did not");
    assert_eq!(stops[2].partial[0].path.as_deref(), Some("/shop[1]/item[1]"));
}

#[test]
fn stepping_over_goes_to_the_next_sibling_and_stepping_out_back_to_the_parent() {
    let price = EXPRESSION.find("price").expect("price");
    let session = start(EXPRESSION, Settings { breakpoints: vec![price], stop_at_start: false, ..Settings::default() });
    let state = session.wait(WAIT);
    assert_eq!((at_text(&state), kind(&state)), ("price".to_owned(), Some(EventKind::Enter)));
    let depth = state.at.as_ref().map(|a| a.depth).expect("a depth");
    assert!(session.step(Action::Over));
    let state = session.wait(WAIT);
    assert_eq!(at_text(&state), "10", "over the first operand to the second");
    assert!(session.step(Action::Out));
    let state = session.wait(WAIT);
    assert_eq!((at_text(&state), kind(&state)), ("price > 10".to_owned(), Some(EventKind::Iterate)), "out to the predicate, at its next item");
    assert_eq!(state.at.as_ref().map(|a| a.depth), Some(depth - 1));
    assert_eq!(state.context.as_ref().and_then(|c| c.position), Some(2));
    session.stop();
    let state = session.wait(WAIT);
    assert_eq!(state.status, Status::Stopped);
    assert!(!session.step(Action::Into), "an ended session takes no steps");
}

#[test]
fn a_function_call_shows_its_frame_and_its_variables() {
    let expression = "let $f := function($a) { $a * 2 } return for $x in (1, 2) return $f($x)";
    let star = expression.find('*').expect("*");
    let session = start(expression, Settings { breakpoints: vec![star], stop_at_start: false, ..Settings::default() });
    let state = session.wait(WAIT);
    assert_eq!(at_text(&state), "$a * 2");
    assert_eq!(state.call_stack.len(), 2, "{:?}", state.call_stack);
    assert_eq!(state.call_stack[0].function, "function($a) { $a * 2 }", "the innermost frame first");
    assert_eq!(state.call_stack[1].function, "the expression");
    assert!(state.context.is_none(), "an inline function has no context item");
    let a = state.variables.iter().find(|v| v.name == "$a").expect("$a is read");
    assert_eq!((a.count, a.items[0].text.as_str()), (1, "1"));
    assert!(session.step(Action::Continue));
    let state = session.wait(WAIT);
    let a = state.variables.iter().find(|v| v.name == "$a").expect("$a");
    assert_eq!(a.items[0].text, "2", "the second call");
    assert!(session.step(Action::Out));
    let state = session.wait(WAIT);
    assert_eq!(state.call_stack.len(), 1, "out of the function: back in the expression ({})", at_text(&state));
    let x = state.variables.iter().find(|v| v.name == "$x").expect("the for's variable");
    assert_eq!(x.items[0].text, "2");
    let (_, end) = run_through(&session, Action::Continue);
    assert_eq!(end.result.expect("ends").expect("runs").items.iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["2", "4"]);
}

#[test]
fn a_runaway_expression_is_paused_and_stopped_and_its_worker_ends() {
    let session = start("count(for $a in 1 to 1000000, $b in 1 to 1000 return $a)", Settings { stop_at_start: false, ..Settings::default() });
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(session.state().status, Status::Running);
    session.pause();
    let state = session.wait(WAIT);
    assert_eq!(state.status, Status::Paused, "pause stops it at its next event");
    assert!(state.instructions > 0);
    session.stop();
    let started = Instant::now();
    while !session.is_done() {
        assert!(started.elapsed() < WAIT, "the worker ends");
        std::thread::sleep(Duration::from_millis(5));
    }
    let state = session.state();
    assert_eq!(state.status, Status::Stopped);
    assert!(state.result.expect("ended").is_err());
    // Stopping a running session (not paused) ends it too.
    let session = start("count(for $a in 1 to 1000000, $b in 1 to 1000 return $a)", Settings { stop_at_start: false, ..Settings::default() });
    std::thread::sleep(Duration::from_millis(20));
    session.stop();
    let started = Instant::now();
    while !session.is_done() {
        assert!(started.elapsed() < WAIT, "the worker ends");
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(session);
}

#[test]
fn dropping_a_paused_session_ends_it_and_errors_come_back() {
    let session = start(EXPRESSION, Settings::default());
    assert_eq!(session.wait(WAIT).status, Status::Paused);
    let started = Instant::now();
    drop(session);
    assert!(started.elapsed() < Duration::from_secs(2), "dropping does not hang");
    let e = Debugger::start(SHOP, "//item[", &Options::default(), Settings::default()).err().expect("does not compile");
    assert_eq!(e.code, "XPST0003");
    let session = Debugger::start("not xml", "1", &Options::default(), Settings::default()).expect("compiles");
    let state = session.wait(WAIT);
    assert_eq!(state.status, Status::Finished);
    assert!(state.result.expect("ended").is_err(), "the document cannot be read");
    let session = start("1 div 0", Settings { stop_at_start: false, ..Settings::default() });
    let state = session.wait(WAIT);
    assert_eq!(state.result.expect("ended").expect_err("fails").code, "FOAR0001");
    let session = start(
        "count((1 to 100000) ! .)",
        Settings { stop_at_start: false, limits: xmlcraft_query::Limits { max_instructions: 1000 }, ..Settings::default() },
    );
    let state = session.wait(WAIT);
    assert_eq!(state.status, Status::Stopped);
    assert!(state.result.expect("ended").expect_err("over its limit").message.contains("limit"));
}
