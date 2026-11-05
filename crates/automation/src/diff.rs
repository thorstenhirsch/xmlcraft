//! `diff_compare` and `diff_merge`: two documents compared, and a base with two versions merged.
//!
//! Each document is an open document (`id`), a file (`path`, through the root confinement) or `text`. Nothing is written: the merged
//! text is returned for the caller to open or save.

use serde_json::{Value, json};
use xmlcraft_diff::{Choice, Family, HunkKind, LineOptions, TreeOptions, Whitespace, compare_mixed, diff_lines, diff_words, merge3};
use xmlcraft_engine::language::Language;
use xmlcraft_text::Buffer;

use crate::documents::{fail, json_result, position_json, source};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

/// The most differences one answer lists (the rest are counted).
const MAX_LISTED: usize = 1000;

fn document_schema(what: &str) -> Value {
    json!({
        "type": "object",
        "description": what,
        "properties": {
            "id": { "type": "integer", "minimum": 0, "description": "An open document" },
            "path": { "type": "string", "description": "A file to read" },
            "text": { "type": "string", "description": "The text itself" },
        },
        "additionalProperties": false
    })
}

fn option_properties() -> Value {
    json!({
        "format": { "type": "string", "enum": ["xml", "json", "yaml"], "description": "What the documents are written in (default: by the name of an open document or a file, else xml). It decides how a tree is read" },
        "mode": { "type": "string", "enum": ["auto", "lines", "tree"], "description": "lines: line by line, with the words that differ; tree: elements, attributes and text (XML) or objects, arrays and values (JSON, YAML) by their place; auto (default): a tree when both documents can be read as one, else lines" },
        "whitespace": { "type": "string", "enum": ["exact", "ends", "collapse", "all"], "description": "What counts as the same white space (default exact)" },
        "ignore_case": { "type": "boolean" },
        "ignore_blank_lines": { "type": "boolean", "description": "Lines mode: blank lines are not differences" },
        "ignore_comments": { "type": "boolean", "description": "Tree mode, XML" },
        "ignore_processing_instructions": { "type": "boolean", "description": "Tree mode, XML" },
        "ignore_child_order": { "type": "boolean", "description": "Tree mode, XML: the children of an element may be in any order (the members of an object never matter)" },
        "numbers_by_value": { "type": "boolean", "description": "Tree mode, JSON and YAML: 1, 1.0 and 1e0 are the same number" },
        "ignore_prefixes": { "type": "boolean", "description": "Tree mode, XML: names are the same when their namespaces and local names are, whatever the prefixes" },
    })
}

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut compare = option_properties();
    let mut merge = option_properties();
    if let (Some(c), Some(m)) = (compare.as_object_mut(), merge.as_object_mut()) {
        c.insert("old".into(), document_schema("The first document"));
        c.insert("new".into(), document_schema("The second document"));
        c.insert(
            "context".into(),
            json!({ "type": "integer", "minimum": 0, "maximum": 100, "description": "Lines mode: lines of context in the unified diff (default 3)" }),
        );
        for key in ["base", "ours", "theirs"] {
            m.insert(key.into(), document_schema("One of the three documents"));
        }
        m.insert(
            "conflicts".into(),
            json!({ "type": "string", "enum": ["markers", "ours", "theirs", "both", "base"], "description": "What to write for a conflict (default markers: <<<<<<< ours, ||||||| base, =======, >>>>>>> theirs)" }),
        );
        m.remove("mode");
    }
    vec![
        ToolDef {
            name: "diff_compare",
            title: "Compare two documents",
            description: "Compare two documents (open, files or text). In lines mode the answer lists the hunks (lines of each document, 1-based, the kind added, \
                          removed or changed, and for a change the words that differ) and a unified diff; in tree mode it lists the differences of the trees \
                          (added, removed or changed, with a path - an XPath-like path for XML, a JSON Pointer for JSON and YAML - the line and column in \
                          each document and the values). Options say what counts as the same. Nothing is written.",
            input_schema: json!({ "type": "object", "properties": compare, "required": ["old", "new"], "additionalProperties": false }),
            read_only: true,
            destructive: false,
            handler: diff_compare,
        },
        ToolDef {
            name: "diff_merge",
            title: "Merge two versions of a document",
            description: "Three-way merge: `base` and the two versions made from it, `ours` and `theirs`, line by line. Changes of only one version are taken; \
                          the same change in both is taken once; different changes to the same lines are conflicts, written as `conflicts` says. Returns the \
                          merged `text` and the number of conflicts. Nothing is written: open the text with doc_new or save it with doc_save.",
            input_schema: json!({ "type": "object", "properties": merge, "required": ["base", "ours", "theirs"], "additionalProperties": false }),
            read_only: true,
            destructive: false,
            handler: diff_merge,
        },
    ]
}

fn flag(args: &Value, key: &str) -> Result<bool> {
    match args.get(key) {
        None => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(ToolError::InvalidArgs(format!("{key} is true or false"))),
    }
}

fn whitespace(args: &Value) -> Result<Whitespace> {
    Ok(match args.get("whitespace").and_then(Value::as_str) {
        None | Some("exact") => Whitespace::Exact,
        Some("ends") => Whitespace::Ends,
        Some("collapse") => Whitespace::Collapse,
        Some("all") => Whitespace::All,
        Some(other) => return Err(ToolError::InvalidArgs(format!("whitespace is exact, ends, collapse or all, not {other:?}"))),
    })
}

fn line_options(args: &Value) -> Result<LineOptions> {
    Ok(LineOptions {
        whitespace: whitespace(args)?,
        ignore_case: flag(args, "ignore_case")?,
        ignore_blank_lines: flag(args, "ignore_blank_lines")?,
        ..LineOptions::default()
    })
}

fn tree_options(args: &Value) -> Result<TreeOptions> {
    Ok(TreeOptions {
        whitespace: whitespace(args)?,
        ignore_case: flag(args, "ignore_case")?,
        ignore_comments: flag(args, "ignore_comments")?,
        ignore_processing_instructions: flag(args, "ignore_processing_instructions")?,
        ignore_child_order: flag(args, "ignore_child_order")?,
        numbers_by_value: flag(args, "numbers_by_value")?,
        ignore_prefixes: flag(args, "ignore_prefixes")?,
        ..TreeOptions::default()
    })
}

fn family_of(language: Language) -> Family {
    match language {
        Language::Xml => Family::Xml,
        Language::Json(_) => Family::Json,
        Language::Yaml => Family::Yaml,
    }
}

fn document(a: &Automation, args: &Value, key: &str) -> Result<crate::documents::Source> {
    let item = args.get(key).ok_or_else(|| ToolError::InvalidArgs(format!("`{key}` is required")))?;
    if !item.is_object() {
        return Err(ToolError::InvalidArgs(format!("{key} is an object with an id, a path or text")));
    }
    source(a, item)
}

/// What the two documents are written in: the `format` argument for both, else each by its name (an open document or a file), else
/// that of the other, else XML.
fn formats_of(args: &Value, named: [Option<Family>; 2]) -> Result<(Family, Family)> {
    match args.get("format") {
        Some(Value::String(name)) => {
            let family = Family::parse(name).ok_or_else(|| ToolError::InvalidArgs(format!("format is xml, json or yaml, not {name:?}")))?;
            Ok((family, family))
        }
        Some(_) => Err(ToolError::InvalidArgs("format is xml, json or yaml".into())),
        None => Ok(match named {
            [Some(a), Some(b)] => (a, b),
            [Some(a), None] | [None, Some(a)] => (a, a),
            [None, None] => (Family::Xml, Family::Xml),
        }),
    }
}

fn family_name(family: Family) -> &'static str {
    match family {
        Family::Xml => "xml",
        Family::Json => "json",
        Family::Yaml => "yaml",
    }
}

fn place(buffer: &Buffer, offset: usize) -> Value {
    let (line, column) = position_json(buffer, offset.min(buffer.len()));
    json!({ "line": line, "column": column })
}

fn lines_json(first: usize, end: usize) -> Value {
    // 1-based, inclusive; an empty range (an addition) is null with the line it goes before.
    if end > first { json!({ "first": first + 1, "last": end }) } else { json!({ "first": null, "before": first + 1 }) }
}

fn diff_compare(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let (old, new) = (document(a, args, "old")?, document(a, args, "new")?);
    let by_name = |key: &str, d: &crate::documents::Source| args[key].get("text").is_none().then(|| family_of(d.language));
    let (old_family, new_family) = formats_of(args, [by_name("old", &old), by_name("new", &new)])?;
    let mode = args.get("mode").and_then(Value::as_str).unwrap_or("auto");
    if !matches!(mode, "auto" | "lines" | "tree") {
        return Err(ToolError::InvalidArgs(format!("mode is auto, lines or tree, not {mode:?}")));
    }
    let tree_options = tree_options(args)?;
    let line_options = line_options(args)?;
    if mode != "lines" {
        match compare_mixed(&old.text, old_family, &new.text, new_family, &tree_options) {
            Ok(changes) => {
                let (old_buffer, new_buffer) = (Buffer::from_text(&old.text), Buffer::from_text(&new.text));
                let listed: Vec<Value> = changes
                    .iter()
                    .take(MAX_LISTED)
                    .map(|c| {
                        let kind = match c.kind {
                            xmlcraft_diff::ChangeKind::Added => "added",
                            xmlcraft_diff::ChangeKind::Removed => "removed",
                            xmlcraft_diff::ChangeKind::Changed => "changed",
                        };
                        json!({
                            "kind": kind,
                            "path": c.path,
                            "old": c.old.as_ref().map(|r| place(&old_buffer, r.start)),
                            "new": c.new.as_ref().map(|r| place(&new_buffer, r.start)),
                            "old_value": c.old_text,
                            "new_value": c.new_text,
                        })
                    })
                    .collect();
                return json_result(json!({
                    "mode": "tree",
                    "format": { "old": family_name(old_family), "new": family_name(new_family) },
                    "identical": changes.is_empty(),
                    "count": changes.len(),
                    "truncated": changes.len() > MAX_LISTED,
                    "changes": listed,
                }));
            }
            Err(e) if mode == "tree" => return fail(format!("a document cannot be compared as a tree: {e}")),
            Err(_) => {}
        }
    }
    let diff = diff_lines(&old.text, &new.text, &line_options);
    let context = args.get("context").and_then(Value::as_u64).map_or(Ok(3), |n| {
        usize::try_from(n).ok().filter(|n| *n <= 100).ok_or_else(|| ToolError::InvalidArgs("context is a whole number from 0 to 100".into()))
    })?;
    let hunks: Vec<Value> = diff
        .hunks
        .iter()
        .take(MAX_LISTED)
        .map(|h| {
            let kind = match h.kind() {
                HunkKind::Added => "added",
                HunkKind::Removed => "removed",
                HunkKind::Changed => "changed",
            };
            let mut hunk = json!({ "kind": kind, "old": lines_json(h.old.start, h.old.end), "new": lines_json(h.new.start, h.new.end) });
            // A change of one line to one line says which words.
            if h.old.len() == 1 && h.new.len() == 1 {
                let (o, n) = (old.text.get(diff.old_span(h)).unwrap_or_default(), new.text.get(diff.new_span(h)).unwrap_or_default());
                let (ow, nw) = diff_words(o, n);
                hunk["words"] = json!({
                    "old": ow.iter().map(|r| o.get(r.clone()).unwrap_or_default()).collect::<Vec<_>>(),
                    "new": nw.iter().map(|r| n.get(r.clone()).unwrap_or_default()).collect::<Vec<_>>(),
                });
            }
            hunk
        })
        .collect();
    let unified = diff.unified(&old.text, &new.text, "old", "new", context);
    json_result(json!({
        "mode": "lines",
        "identical": diff.is_identical(),
        "count": diff.hunks.len(),
        "truncated": diff.hunks.len() > MAX_LISTED,
        "hunks": hunks,
        "unified": unified,
    }))
}

fn diff_merge(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let (base, ours, theirs) = (document(a, args, "base")?, document(a, args, "ours")?, document(a, args, "theirs")?);
    let choice = match args.get("conflicts").and_then(Value::as_str) {
        None | Some("markers") => Choice::Markers,
        Some("ours") => Choice::Ours,
        Some("theirs") => Choice::Theirs,
        Some("both") => Choice::Both,
        Some("base") => Choice::Base,
        Some(other) => return Err(ToolError::InvalidArgs(format!("conflicts is markers, ours, theirs, both or base, not {other:?}"))),
    };
    let merged = merge3(&base.text, &ours.text, &theirs.text, &line_options(args)?);
    json_result(json!({ "text": merged.render(choice), "conflicts": merged.conflict_count(), "clean": merged.conflict_count() == 0 }))
}
