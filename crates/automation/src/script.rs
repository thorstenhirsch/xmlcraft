//! Scripts: a list of tool calls run in order (`xmlcraft-cli run --script`).
//!
//! A script is a JSON array of steps `{"tool": "name", "args": {…}}`, or an object `{"steps": [...]}`. Each step's result is recorded;
//! by default the run stops at the first failure, because later steps usually depend on earlier ones.

use serde_json::{Value, json};

use crate::{Automation, Content, ToolError};

/// The most steps one script may have.
pub const MAX_STEPS: usize = 10_000;

/// What one step did.
#[derive(Clone, Debug, PartialEq)]
pub struct StepResult {
    pub tool: String,
    /// The tool's results as JSON, or the error message.
    pub outcome: Result<Vec<Value>, String>,
}

impl StepResult {
    pub fn to_json(&self) -> Value {
        match &self.outcome {
            Ok(content) => json!({ "tool": self.tool, "ok": true, "result": content }),
            Err(message) => json!({ "tool": self.tool, "ok": false, "error": message }),
        }
    }
}

/// Run `script`. Returns one result per step run; a malformed script is an `Err` before anything runs.
pub fn run_script(automation: &mut Automation, script: &Value, keep_going: bool) -> Result<Vec<StepResult>, String> {
    let steps = match script {
        Value::Array(steps) => steps,
        Value::Object(o) => o.get("steps").and_then(Value::as_array).ok_or("a script object needs a \"steps\" array")?,
        _ => return Err("a script is an array of steps, or an object with a \"steps\" array".into()),
    };
    if steps.len() > MAX_STEPS {
        return Err(format!("a script may have at most {MAX_STEPS} steps, this one has {}", steps.len()));
    }
    // Check every step's shape first, so a typo in step 40 does not leave steps 1 to 39 already done.
    let mut parsed = Vec::with_capacity(steps.len());
    for (i, step) in steps.iter().enumerate() {
        let n = i + 1;
        let object = step.as_object().ok_or_else(|| format!("step {n}: expected an object like {{\"tool\": \"app_info\", \"args\": {{}}}}"))?;
        if let Some(key) = object.keys().find(|k| *k != "tool" && *k != "args") {
            return Err(format!("step {n}: unknown key {key:?} (use \"tool\" and \"args\")"));
        }
        let tool = object.get("tool").and_then(Value::as_str).ok_or_else(|| format!("step {n}: missing the \"tool\" name"))?;
        parsed.push((tool, object.get("args").cloned().unwrap_or(Value::Null)));
    }
    let mut results = Vec::with_capacity(parsed.len());
    for (tool, args) in parsed {
        let outcome = match automation.call(tool, &args) {
            Ok(content) => Ok(content.into_iter().map(|Content::Json(v)| v).collect()),
            Err(ToolError::UnknownTool(t)) => Err(format!("unknown tool {t:?}")),
            Err(e) => Err(e.to_string()),
        };
        let failed = outcome.is_err();
        results.push(StepResult { tool: tool.to_owned(), outcome });
        if failed && !keep_going {
            break;
        }
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_run_in_order_and_each_result_is_recorded() {
        let mut a = Automation::new();
        let results = run_script(&mut a, &json!([{"tool": "app_info"}, {"tool": "command_list", "args": {"menu": "File"}}]), false).expect("runs");
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| r.outcome.is_ok()));
        assert_eq!(results[1].tool, "command_list");
        assert_eq!(results[1].to_json()["ok"], true);
    }

    #[test]
    fn the_run_stops_at_the_first_failure_unless_told_to_go_on() {
        let script = json!([{"tool": "app_info"}, {"tool": "no_such_tool"}, {"tool": "app_info"}]);
        let stopped = run_script(&mut Automation::new(), &script, false).expect("runs");
        assert_eq!(stopped.len(), 2);
        assert_eq!(stopped[1].outcome, Err("unknown tool \"no_such_tool\"".into()));
        let all = run_script(&mut Automation::new(), &script, true).expect("runs");
        assert_eq!(all.len(), 3);
        assert!(all[2].outcome.is_ok());
    }

    #[test]
    fn a_script_object_with_steps_works_too() {
        let r = run_script(&mut Automation::new(), &json!({"steps": [{"tool": "app_info"}]}), false).expect("runs");
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn a_malformed_script_is_refused_before_anything_runs() {
        for (script, needle) in [
            (json!("x"), "array of steps"),
            (json!({"nope": []}), "\"steps\" array"),
            (json!([1]), "step 1: expected an object"),
            (json!([{"tool": "app_info"}, {"args": {}}]), "step 2: missing the \"tool\" name"),
            (json!([{"tool": "app_info", "arg": {}}]), "unknown key \"arg\""),
            (json!([{"tool": 3}]), "missing the \"tool\" name"),
        ] {
            let err = run_script(&mut Automation::new(), &script, false).expect_err(&script.to_string());
            assert!(err.contains(needle), "{script}: {err}");
        }
    }

    #[test]
    fn a_runaway_script_is_refused() {
        let steps: Vec<Value> = (0..=MAX_STEPS).map(|_| json!({"tool": "app_info"})).collect();
        assert!(run_script(&mut Automation::new(), &Value::Array(steps), false).expect_err("too long").contains("at most"));
    }

    #[test]
    fn an_empty_script_does_nothing() {
        assert_eq!(run_script(&mut Automation::new(), &json!([]), false), Ok(Vec::new()));
    }
}
