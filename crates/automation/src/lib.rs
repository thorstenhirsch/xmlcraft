#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! xmlcraft-automation: agent control for XmlCraft.
//!
//! - **Layer:** L7. Headless: it depends on the engine, never on a UI toolkit.
//! - [`Automation`] is a tool set over an engine [`Session`]. Every tool has a JSON Schema ([`tools`]) and takes and returns JSON,
//!   so the same table drives the MCP server ([`mcp`]), `xmlcraft-cli run` and scripts.
//! - [`control`] is the transport of the opt-in UI control channel (the egui half is in `xmlcraft-ui-egui`).
//! - An optional root folder confines every path a tool reads or writes ([`file_access`]).
//! - A tool failure is an error an agent can act on: it says what was wrong and what is allowed. Tools never panic: a panic is caught
//!   and reported (AGENTS.md section 5).

mod documents;
pub mod file_access;
mod lines;
mod schema;
mod script;
mod tools;

#[cfg(not(target_arch = "wasm32"))]
pub mod control;
#[cfg(feature = "mcp")]
pub mod mcp;

use std::path::Path;

use serde_json::Value;
use xmlcraft_engine::Session;

pub use schema::validate as validate_arguments;
pub use script::{StepResult, run_script};
pub use tools::{ToolDef, find as find_tool, tools};

use file_access::Confinement;

/// One piece of a tool's result.
#[derive(Clone, Debug, PartialEq)]
pub enum Content {
    /// Structured data (MCP: a text block with the JSON plus `structuredContent`).
    Json(Value),
}

/// Why a tool call failed.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolError {
    /// No tool has this name.
    UnknownTool(String),
    /// The arguments don't match the tool's schema.
    InvalidArgs(String),
    /// The tool ran and failed (the message is for the agent to read).
    Failed(String),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolError::UnknownTool(t) => write!(f, "unknown tool {t:?}"),
            ToolError::InvalidArgs(m) | ToolError::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for ToolError {}

pub(crate) type Result<T> = std::result::Result<T, ToolError>;

/// A headless XmlCraft session driven by tool calls.
pub struct Automation {
    session: Session,
    confinement: Confinement,
}

impl Default for Automation {
    fn default() -> Self {
        Self::new()
    }
}

impl Automation {
    /// A session with no root: paths are not confined (a person at their own terminal).
    pub fn new() -> Self {
        Self { session: Session::new(), confinement: Confinement::none() }
    }

    /// A session whose tools may only touch files under `root`.
    pub fn with_root(root: &Path) -> Result<Self> {
        Ok(Self { session: Session::new(), confinement: Confinement::new(root)? })
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// The folder every path is confined to, if any.
    pub fn root(&self) -> Option<&Path> {
        self.confinement.root()
    }

    /// The path confinement, for tools that read or write files.
    pub fn confinement(&self) -> &Confinement {
        &self.confinement
    }

    /// Call a tool. Omitted arguments (`null`) mean none. The arguments are checked against the tool's schema first.
    pub fn call(&mut self, name: &str, args: &Value) -> Result<Vec<Content>> {
        let def = tools::find(name).ok_or_else(|| ToolError::UnknownTool(name.to_owned()))?;
        let empty = Value::Object(serde_json::Map::new());
        let args = if args.is_null() { &empty } else { args };
        schema::validate(&def.input_schema, args).map_err(|e| ToolError::InvalidArgs(format!("{name}: {e}")))?;
        // The schema takes `2.0` for an integer, so the handlers must too: they read integers with `as_u64`. (Arguments with no such
        // number are not copied: they can hold a whole document.)
        let normalized = integral(args);
        let args = normalized.as_ref().unwrap_or(args);
        match xmlcraft_engine::guard(|| (def.handler)(self, args)) {
            Ok(result) => result,
            Err(message) => {
                log::error!("tool {name} panicked: {message}");
                Err(ToolError::Failed(format!("{name}: an internal error stopped it ({message}); your files are unchanged")))
            }
        }
    }
}

/// `value` with every number that is a whole number written as an integer (`2.0` is `2`), so the handlers' `as_u64` reads what the
/// schema's "integer" lets through; `None` when there is no such number (nothing is copied then).
fn integral(value: &Value) -> Option<Value> {
    match value {
        Value::Number(n) if !(n.is_i64() || n.is_u64()) => match n.as_f64() {
            // Whole and below 2^53, so the conversions lose nothing.
            Some(f) if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.0e15 => {
                Some(if f >= 0.0 { Value::from(f as u64) } else { Value::from(f as i64) })
            }
            _ => None,
        },
        Value::Array(items) => {
            let changed: Vec<Option<Value>> = items.iter().map(integral).collect();
            changed
                .iter()
                .any(Option::is_some)
                .then(|| Value::Array(items.iter().zip(changed).map(|(old, new)| new.unwrap_or_else(|| old.clone())).collect()))
        }
        Value::Object(map) => {
            let changed: Vec<Option<Value>> = map.values().map(integral).collect();
            changed
                .iter()
                .any(Option::is_some)
                .then(|| Value::Object(map.iter().zip(changed).map(|((key, old), new)| (key.clone(), new.unwrap_or_else(|| old.clone()))).collect()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn whole_numbers_written_as_floats_are_integers_to_the_handlers() {
        let normalized =
            integral(&json!({"a": 2.0, "b": [3.0, 1.5, -4.0], "c": {"d": 7.0}, "e": "x", "f": 5})).expect("there are whole numbers to change");
        assert_eq!(normalized, json!({"a": 2, "b": [3, 1.5, -4], "c": {"d": 7}, "e": "x", "f": 5}));
        // Nothing to change, nothing copied: text, integers, fractions, numbers too big to be integers.
        assert_eq!(integral(&json!({"text": "x".repeat(1000), "n": 5, "f": 1.5, "big": 1.0e300, "list": [1, "a", null, true]})), None);
        assert_eq!(integral(&json!(1.0e300)), None, "too big to be an integer stays as it is");
        let mut a = Automation::new();
        let id = json_of(a.call("doc_new", &json!({ "text": "ab\ncd" })).expect("opens"))["id"].clone();
        let selected = a.call("doc_select", &json!({ "id": id, "line": 2.0, "column": 2.0 })).expect("2.0 is an integer");
        assert_eq!(json_of(selected)["carets"][0]["start"], 4);
    }

    fn json_of(content: Vec<Content>) -> Value {
        match content.into_iter().next() {
            Some(Content::Json(v)) => v,
            None => Value::Null,
        }
    }

    #[test]
    fn app_info_lists_the_tools_and_reports_the_root() {
        let mut a = Automation::new();
        let info = json_of(a.call("app_info", &Value::Null).expect("app_info runs"));
        assert_eq!(info["name"], "XmlCraft");
        assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
        assert!(info["tools"].as_array().is_some_and(|t| t.iter().any(|n| n == "app_info")));
        assert_eq!(info["tool_count"], tools().len());
        assert_eq!(info["root"], Value::Null);

        let dir = tempfile::tempdir().expect("temp dir");
        let mut confined = Automation::with_root(dir.path()).expect("root");
        let info = json_of(confined.call("app_info", &json!({})).expect("app_info runs"));
        assert!(info["root"].as_str().is_some_and(|r| !r.is_empty()));
    }

    #[test]
    fn command_list_filters_by_menu() {
        let mut a = Automation::new();
        let all = json_of(a.call("command_list", &json!({})).expect("runs"));
        let view = json_of(a.call("command_list", &json!({"menu": "View"})).expect("runs"));
        assert!(all["count"].as_u64() > view["count"].as_u64());
        assert!(view["commands"].as_array().is_some_and(|c| c.iter().all(|c| c["menu"] == "View")));
        assert!(view["commands"].as_array().is_some_and(|c| c.iter().any(|c| c["id"] == "view.palette" && c["shortcut"] == "Ctrl+K")));
    }

    #[test]
    fn an_unknown_tool_is_an_error_naming_it() {
        let mut a = Automation::new();
        assert_eq!(a.call("nope", &json!({})), Err(ToolError::UnknownTool("nope".into())));
        assert_eq!(a.call("", &json!({})), Err(ToolError::UnknownTool(String::new())));
    }

    #[test]
    fn arguments_are_checked_against_the_schema_before_the_tool_runs() {
        let mut a = Automation::new();
        let err = a.call("command_list", &json!({"menu": "Nope"})).expect_err("not a menu").to_string();
        assert!(err.contains("command_list") && err.contains("expected one of"), "{err}");
        let err = a.call("app_info", &json!({"extra": 1})).expect_err("no arguments allowed").to_string();
        assert!(err.contains("unknown argument") && err.contains("takes no arguments"), "{err}");
        let err = a.call("app_info", &json!([1])).expect_err("not an object").to_string();
        assert!(err.contains("expected an object"), "{err}");
    }

    #[test]
    fn a_root_that_is_not_a_folder_is_refused() {
        assert!(Automation::with_root(Path::new("/no/such/folder/at/all")).is_err());
    }
}
