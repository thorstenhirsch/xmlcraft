//! `files_find`: Find in Files. The files under a folder (inside the root) whose names fit the patterns are searched for a text or a
//! regular expression with the same matcher the editor uses; nothing is opened or changed.

use serde_json::{Value, json};
use xmlcraft_editor::find::Query;
use xmlcraft_platform::search::{Options, search_folder};

use crate::documents::{json_result, text_arg};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

pub(crate) fn tools() -> Vec<ToolDef> {
    vec![ToolDef {
        name: "files_find",
        title: "Find in Files",
        description: "Search the text files under a folder (inside the root) for `pattern` (text, or a regular expression with `regex`), \
                      optionally only files whose names fit `glob` (`*.xml;*.xsd`). Folders that start with a dot, node_modules and target are skipped, links are not followed, and \
                      files that are not text are skipped. Returns the files with their hits (line and column from 1, and the line), and whether a limit cut the list short. Nothing is changed.",
        input_schema: json!({
            "type": "object",
            "properties": {
                "folder": { "type": "string", "description": "The folder to search" },
                "pattern": { "type": "string", "description": "The text or regular expression to find" },
                "regex": { "type": "boolean", "description": "The pattern is a regular expression (default false)" },
                "case_sensitive": { "type": "boolean", "description": "Default true" },
                "whole_word": { "type": "boolean", "description": "A match must start and end at a word boundary (default false)" },
                "glob": { "type": "string", "description": "File name patterns separated by `;` (default: every file)" },
                "max_hits": { "type": "integer", "minimum": 1, "maximum": 10000, "description": "The most hits listed (default 500)" },
            },
            "required": ["folder", "pattern"],
            "additionalProperties": false,
        }),
        read_only: true,
        destructive: false,
        handler: files_find,
    }]
}

fn flag(args: &Value, key: &str, default: bool) -> Result<bool> {
    match args.get(key) {
        None => Ok(default),
        Some(v) => v.as_bool().ok_or_else(|| ToolError::InvalidArgs(format!("`{key}` is true or false"))),
    }
}

fn files_find(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let folder = text_arg(args, "folder").ok_or_else(|| ToolError::InvalidArgs("`folder` is required".into()))?;
    let pattern = text_arg(args, "pattern").ok_or_else(|| ToolError::InvalidArgs("`pattern` is required".into()))?;
    let max_hits = match args.get("max_hits") {
        None => 500,
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| (1..=10_000).contains(n))
            .ok_or_else(|| ToolError::InvalidArgs("max_hits is a whole number from 1 to 10000".into()))?,
    };
    let query = Query {
        pattern: pattern.to_owned(),
        regex: flag(args, "regex", false)?,
        case_sensitive: flag(args, "case_sensitive", true)?,
        whole_word: flag(args, "whole_word", false)?,
    };
    let real = a.confinement.resolve_read(folder)?;
    if !real.is_dir() {
        return Err(ToolError::Failed(format!("{folder}: not a folder")));
    }
    let mut options = Options::new(query);
    options.max_hits = max_hits;
    if let Some(glob) = text_arg(args, "glob") {
        options.glob = glob.to_owned();
    }
    let found = search_folder(&real, &options).map_err(|e| ToolError::InvalidArgs(e.to_string()))?;
    let shown = |p: &std::path::Path| p.strip_prefix(&real).unwrap_or(p).to_string_lossy().replace('\\', "/");
    let files: Vec<Value> = found
        .files
        .iter()
        .map(|f| json!({ "path": shown(&f.path), "hits": f.hits.iter().map(|h| json!({ "line": h.line, "column": h.column, "text": h.text })).collect::<Vec<_>>() }))
        .collect();
    json_result(json!({
        "folder": real.to_string_lossy(),
        "files_with_hits": files.len(),
        "hits": found.files.iter().map(|f| f.hits.len()).sum::<usize>(),
        "searched": found.searched,
        "skipped": found.skipped,
        "truncated": found.truncated,
        "files": files,
    }))
}
