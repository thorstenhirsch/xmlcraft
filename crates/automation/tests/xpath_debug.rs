//! `xpath_debug_start`, `xpath_debug_step`, `xpath_debug_state` and `xpath_debug_stop`.

use serde_json::{Value, json};
use xmlcraft_automation::{Automation, Content, ToolError};

fn call(a: &mut Automation, tool: &str, args: Value) -> Result<Value, ToolError> {
    a.call(tool, &args).map(|content| match content.into_iter().next() {
        Some(Content::Json(v)) => v,
        None => Value::Null,
    })
}

const SHOP: &str = "<shop>\n<item><price>20</price><name>a</name></item>\n<item><price>5</price><name>b</name></item>\n</shop>";

#[test]
fn a_session_stops_at_a_breakpoint_steps_and_ends() {
    let mut a = Automation::new();
    let expression = "//item[price > 10]/name";
    let at = expression.find('>').unwrap_or(0);
    let r = call(&mut a, "xpath_debug_start", json!({ "text": SHOP, "expression": expression, "breakpoints": [at], "stop_at_start": false }))
        .expect("starts");
    let id = r["session"].as_u64().expect("an id");
    assert_eq!(
        (r["status"].as_str(), r["at"]["kind"].as_str(), r["at"]["text"].as_str()),
        (Some("paused"), Some("iterate"), Some("price > 10")),
        "{r}"
    );
    assert_eq!(r["at"]["span"], json!([7, 17]));
    let context = &r["context"];
    assert_eq!(
        (context["item"]["path"].as_str(), context["item"]["line"].as_u64(), context["position"].as_u64()),
        (Some("/shop[1]/item[1]"), Some(2), Some(1))
    );
    assert_eq!(r["call_stack"][0]["function"], "the expression");
    let r = call(&mut a, "xpath_debug_step", json!({ "session": id, "action": "into" })).expect("steps");
    assert_eq!(r["at"]["text"].as_str(), Some("price"), "into the first operand");
    let r = call(&mut a, "xpath_debug_step", json!({ "session": id, "action": "continue" })).expect("steps");
    assert_eq!(r["context"]["position"].as_u64(), Some(2), "the breakpoint again, on the second item");
    let r = call(&mut a, "xpath_debug_step", json!({ "session": id, "action": "continue" })).expect("steps");
    assert_eq!(r["status"].as_str(), Some("finished"));
    assert_eq!(r["result"]["count"].as_u64(), Some(1));
    let e = call(&mut a, "xpath_debug_step", json!({ "session": id, "action": "into" })).expect_err("ended");
    assert!(e.to_string().contains("not paused"), "{e}");
    let s = call(&mut a, "xpath_debug_state", json!({ "session": id })).expect("state");
    assert_eq!(s["status"].as_str(), Some("finished"));
    call(&mut a, "xpath_debug_stop", json!({ "session": id })).expect("stops");
    assert!(call(&mut a, "xpath_debug_state", json!({ "session": id })).is_err(), "a stopped session is forgotten");
}

#[test]
fn sessions_are_capped_a_runaway_is_paused_and_errors_are_said() {
    let mut a = Automation::new();
    let mut ids = Vec::new();
    for _ in 0..4 {
        let r = call(
            &mut a,
            "xpath_debug_start",
            json!({ "text": "<r/>", "expression": "count(for $a in 1 to 1000000, $b in 1 to 1000 return $a)", "stop_at_start": false, "wait_ms": 0 }),
        )
        .expect("starts");
        ids.push(r["session"].as_u64().expect("an id"));
    }
    let e = call(&mut a, "xpath_debug_start", json!({ "text": "<r/>", "expression": "1" })).expect_err("capped");
    assert!(e.to_string().contains("stop one first"), "{e}");
    let r = call(&mut a, "xpath_debug_step", json!({ "session": ids[0], "action": "pause" })).expect("pauses");
    assert_eq!(r["status"].as_str(), Some("paused"), "{r}");
    for id in &ids {
        call(&mut a, "xpath_debug_stop", json!({ "session": id })).expect("stops");
    }
    let e = call(&mut a, "xpath_debug_start", json!({ "text": "<r/>", "expression": "/r[" })).expect_err("syntax");
    assert!(e.to_string().contains("XPST0003"), "{e}");
    assert!(matches!(call(&mut a, "xpath_debug_step", json!({ "session": 1, "action": "sideways" })), Err(ToolError::InvalidArgs(_))));
    assert!(call(&mut a, "xpath_debug_state", json!({ "session": 999 })).is_err());
}
