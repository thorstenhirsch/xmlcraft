//! The tool table: every headless capability, by name, with its JSON Schema and handler.
//!
//! The same table drives `xmlcraft-cli run`, the MCP server and the control channel's headless calls, so an agent, a script and a
//! person at the terminal see the same tools. A tool is listed only when it works; `parity/xmlspy-features.toml` names the tools a
//! feature is reachable through, and `cargo xtask parity` checks that they exist.

use std::sync::OnceLock;

use serde_json::{Value, json};

use crate::{Automation, Content, Result};

/// One tool: how it is described to an agent, and what it does.
pub struct ToolDef {
    /// `[a-z_]` only, so it is a valid identifier in every agent framework.
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// A JSON Schema (`type: object`, `additionalProperties: false`).
    pub input_schema: Value,
    /// Reads only: never changes a document or a file.
    pub read_only: bool,
    /// May discard data (overwrite a file, drop unsaved edits).
    pub destructive: bool,
    pub(crate) handler: fn(&mut Automation, &Value) -> Result<Vec<Content>>,
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

/// Every tool, in the order they are listed.
pub fn tools() -> &'static [ToolDef] {
    static TOOLS: OnceLock<Vec<ToolDef>> = OnceLock::new();
    TOOLS.get_or_init(|| {
        let mut table = vec![
            ToolDef {
                name: "app_info",
                title: "About this XmlCraft",
                description: "The version, the number of tools and registered commands, and the tool names. Start here to see what is available.",
                input_schema: object(json!({}), &[]),
                read_only: true,
                destructive: false,
                handler: app_info,
            },
            ToolDef {
                name: "command_list",
                title: "List commands",
                description: "The commands the app registers (the ones menus, shortcuts and the palette run), optionally only those of one menu. \
                              Each has an id, a label, its menu, its shortcut, and whether it toggles or chooses.",
                input_schema: object(
                    json!({ "menu": { "type": "string", "description": "Only this top-level menu, e.g. \"View\"", "enum": xmlcraft_engine::commands::MENUS } }),
                    &[],
                ),
                read_only: true,
                destructive: false,
                handler: command_list,
            },
        ];
        table.extend(crate::documents::tools());
        table.extend(crate::catalog::tools());
        table.extend(crate::validate::tools());
        table.extend(crate::entry_helpers::tools());
        table.extend(crate::infer::tools());
        table.extend(crate::sample::tools());
        table
    })
}

/// The tool with this name.
pub fn find(name: &str) -> Option<&'static ToolDef> {
    tools().iter().find(|t| t.name == name)
}

fn app_info(a: &mut Automation, _: &Value) -> Result<Vec<Content>> {
    Ok(vec![Content::Json(json!({
        "name": "XmlCraft",
        "version": env!("CARGO_PKG_VERSION"),
        "tools": tools().iter().map(|t| t.name).collect::<Vec<_>>(),
        "tool_count": tools().len(),
        "command_count": xmlcraft_engine::commands::COMMANDS.len(),
        "root": a.root().map(|r| r.display().to_string()),
    }))])
}

fn command_list(_: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    use xmlcraft_engine::commands::{COMMANDS, Kind};
    let menu = args.get("menu").and_then(Value::as_str);
    let list: Vec<Value> = COMMANDS
        .iter()
        .filter(|c| menu.is_none_or(|m| c.menu == Some(m)))
        .map(|c| {
            json!({
                "id": c.id,
                "label": c.label,
                "menu": c.menu,
                "shortcut": c.shortcut.map(|s| s.label(false)),
                "kind": match c.kind { Kind::Action => "action", Kind::Toggle => "toggle", Kind::Choice => "choice" },
            })
        })
        .collect();
    Ok(vec![Content::Json(json!({ "commands": list, "count": list.len() }))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn names_are_unique_snake_case_identifiers() {
        let mut seen = HashSet::new();
        for t in tools() {
            assert!(seen.insert(t.name), "duplicate tool {}", t.name);
            assert!(!t.name.is_empty() && t.name.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{}", t.name);
            assert!(!t.title.is_empty() && !t.description.is_empty(), "{}", t.name);
        }
    }

    #[test]
    fn every_schema_is_a_closed_object() {
        for t in tools() {
            assert_eq!(t.input_schema["type"], "object", "{}", t.name);
            assert_eq!(t.input_schema["additionalProperties"], false, "{} must refuse unknown arguments", t.name);
            assert!(t.input_schema["properties"].is_object(), "{}", t.name);
            for r in t.input_schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                assert!(t.input_schema["properties"].get(r).is_some(), "{}: {r} is required but not a property", t.name);
            }
        }
    }

    #[test]
    fn a_tool_that_destroys_data_is_not_read_only() {
        for t in tools() {
            assert!(!(t.read_only && t.destructive), "{}", t.name);
        }
    }

    #[test]
    fn lookup_finds_a_tool_and_not_a_missing_one() {
        assert!(find("app_info").is_some());
        assert!(find("nope").is_none() && find("").is_none());
    }
}
