//! The document and XML tools: open and edit documents, check them, outline them and format them.
//!
//! Positions are `line` and `column`, both counted from 1, the column in characters. Paths go through the root confinement. A tool
//! that writes a file never replaces one that changed on disk since it was read, or one whose bytes were not valid, without being told
//! to (`force`, or a different path).

use std::path::PathBuf;

use serde_json::{Value, json};
use xmlcraft_editor::find::{MAX_MATCHES, Matcher, Query};
use xmlcraft_engine::analysis::{Analysis, Skipped, analyze, analyze_guarded};
use xmlcraft_engine::folding::{FoldAction, FoldRange};
use xmlcraft_engine::{Document, SaveError};
use xmlcraft_format::{BlankLines, FormatError, Indent};
use xmlcraft_syntax::{Diagnostic, Severity};
use xmlcraft_text::encoding::{Encoding, decode, sniff};
use xmlcraft_text::{Buffer, Edit, Position};

use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

/// The most text one call returns.
const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
/// The most problems one call lists (the counts are always complete).
const MAX_PROBLEMS: usize = 500;
/// The most outline items one call lists.
const MAX_OUTLINE: usize = 5_000;

pub(crate) fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(ToolError::Failed(message.into()))
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

/// The properties that say where the XML is: an open document, a file, or text.
pub(crate) fn source_properties() -> Value {
    json!({
        "id": { "type": "integer", "minimum": 0, "description": "An open document (from doc_open, doc_new or doc_list)" },
        "path": { "type": "string", "description": "A file to read (it is not opened as a document)" },
        "text": { "type": "string", "description": "The XML itself" },
    })
}

/// The properties that say which text, and a place in it: `line` and `column` (from 1) or a byte `offset`.
pub(crate) fn source_and_position_properties() -> Value {
    let mut properties = source_properties();
    if let Some(map) = properties.as_object_mut() {
        map.insert("line".into(), json!({ "type": "integer", "minimum": 1 }));
        map.insert("column".into(), json!({ "type": "integer", "minimum": 1 }));
        map.insert(
            "offset".into(),
            json!({ "type": "integer", "minimum": 0, "description": "A byte offset of the text, instead of line and column" }),
        );
    }
    properties
}

/// The properties that say what to search for.
fn search_properties() -> Value {
    json!({
        "pattern": { "type": "string", "minLength": 1, "description": "The text, or a regular expression with `regex: true`" },
        "regex": { "type": "boolean" },
        "case_sensitive": { "type": "boolean", "description": "Default true" },
        "whole_word": { "type": "boolean" },
    })
}

/// The tools of this module.
pub(crate) fn tools() -> Vec<ToolDef> {
    let id = json!({ "type": "integer", "minimum": 0, "description": "The document's id" });
    let position = |what: &str| json!({ "type": "integer", "minimum": 1, "description": what });
    vec![
        ToolDef {
            name: "doc_open",
            title: "Open a file",
            description: "Open a file as a document (its encoding, byte-order mark and line endings are kept) and return its id. A file that is already open \
                          is not opened twice.",
            input_schema: object(json!({ "path": { "type": "string", "description": "The file to open" } }), &["path"]),
            read_only: false,
            destructive: false,
            handler: doc_open,
        },
        ToolDef {
            name: "doc_new",
            title: "New document",
            description: "Create an untitled document, optionally with text, and return its id.",
            input_schema: object(json!({ "text": { "type": "string", "description": "The first text" } }), &[]),
            read_only: false,
            destructive: false,
            handler: doc_new,
        },
        ToolDef {
            name: "doc_list",
            title: "List documents",
            description: "The open documents: id, name, path, whether it has unsaved changes, its encoding and line ending.",
            input_schema: object(json!({}), &[]),
            read_only: true,
            destructive: false,
            handler: doc_list,
        },
        ToolDef {
            name: "recent_files",
            title: "Recent files",
            description: "The files opened or saved lately in this session (a tool session starts with none), newest first, at most 12. With `clear` true the list is emptied \
                          after it is read. Only paths are listed; nothing is opened.",
            input_schema: object(json!({ "clear": { "type": "boolean" } }), &[]),
            read_only: false,
            destructive: false,
            handler: recent_files,
        },
        ToolDef {
            name: "doc_text",
            title: "Read a document's text",
            description: "The text of a document, or of the lines from `from_line` to `to_line` (counted from 1, both included). At most 4 MiB are returned.",
            input_schema: object(json!({ "id": id.clone(), "from_line": position("The first line"), "to_line": position("The last line") }), &["id"]),
            read_only: true,
            destructive: false,
            handler: doc_text,
        },
        ToolDef {
            name: "doc_edit",
            title: "Edit a document",
            description: "Replace parts of a document's text, as one undoable step. Each edit replaces the text from (line, column) up to (end_line, end_column) \
                          with `text` (counted from 1, columns in characters; the end is not included; leave the end out to insert). The edits are read against the \
                          text as it is now, so they must not overlap.",
            input_schema: object(
                json!({
                    "id": id.clone(),
                    "edits": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 10000,
                        "items": object(
                            json!({
                                "line": position("Where the replaced text starts"),
                                "column": position("Where the replaced text starts, in characters"),
                                "end_line": position("Where it ends (default: the start)"),
                                "end_column": position("Where it ends, in characters (default: the start)"),
                                "text": { "type": "string", "description": "What goes there" },
                            }),
                            &["line", "column", "text"],
                        ),
                    },
                }),
                &["id", "edits"],
            ),
            read_only: false,
            destructive: false,
            handler: doc_edit,
        },
        ToolDef {
            name: "doc_set_text",
            title: "Replace a document's text",
            description: "Replace the whole text of a document, as one undoable step.",
            input_schema: object(json!({ "id": id.clone(), "text": { "type": "string" } }), &["id", "text"]),
            read_only: false,
            destructive: false,
            handler: doc_set_text,
        },
        ToolDef {
            name: "doc_type",
            title: "Type text",
            description: "Type text into an open document as the keyboard does, with the typing aids: a quote typed after `=` in a tag writes the pair and the closing \
                          quote is stepped over. Typed at the caret, or at `line` and `column` (from 1) when they are given. A line feed is Enter and a tab is Tab. \
                          Returns what was typed and where the caret is. Use doc_edit to replace text without the aids.",
            input_schema: object(
                json!({ "id": id.clone(), "text": { "type": "string", "minLength": 1 }, "line": { "type": "integer", "minimum": 1 }, "column": { "type": "integer", "minimum": 1 } }),
                &["id", "text"],
            ),
            read_only: false,
            destructive: false,
            handler: doc_type,
        },
        ToolDef {
            name: "doc_key",
            title: "Press a key",
            description: "Press Backspace, Delete, Enter, Tab or Shift+Tab in an open document, `times` times (default 1), at the caret or at `line` and `column` (from 1). \
                          Backspace between the quotes of an empty attribute value takes both. Returns where the caret is.",
            input_schema: object(
                json!({
                    "id": id.clone(),
                    "key": { "type": "string", "enum": ["Backspace", "Delete", "Enter", "Tab", "Shift+Tab"] },
                    "times": { "type": "integer", "minimum": 1, "maximum": 1000 },
                    "line": { "type": "integer", "minimum": 1 },
                    "column": { "type": "integer", "minimum": 1 },
                }),
                &["id", "key"],
            ),
            read_only: false,
            destructive: false,
            handler: doc_key,
        },
        ToolDef {
            name: "doc_fold",
            title: "Fold or unfold",
            description: "Fold or unfold stretches of lines of an open document (see xml_folds for what can be folded). `fold` folds the innermost range at the caret (or at \
                          `line` and `column`, from 1) that is not folded; `unfold` opens the innermost folded one; `fold_all` and `unfold_all`; `level` folds the ranges \
                          of that level and nothing else. The text does not change; the result lists what is folded (`first_line` stands for the lines after it up to \
                          `last_line`) and how many lines are shown.",
            input_schema: object(
                json!({
                    "id": id.clone(),
                    "action": { "type": "string", "enum": ["fold", "unfold", "fold_all", "unfold_all", "level"] },
                    "level": { "type": "integer", "minimum": 1, "description": "For `level`: 1 is the root element" },
                    "line": { "type": "integer", "minimum": 1 },
                    "column": { "type": "integer", "minimum": 1 },
                }),
                &["id", "action"],
            ),
            read_only: false,
            destructive: false,
            handler: doc_fold,
        },
        ToolDef {
            name: "doc_select",
            title: "Select text",
            description: "Put the caret of an open document at `line` and `column` (from 1), or select from there to `end_line` and `end_column`. With `columns` true the selection \
                          is a rectangle (as a drag with Alt makes): on every line between the two, the text from the display column of one end to the display column of the other \
                          (a tab is as wide as it is drawn; a shorter line gets a caret at its end), so that typing edits every line at once. Returns where the selection is.",
            input_schema: object(
                json!({
                    "id": id.clone(),
                    "line": { "type": "integer", "minimum": 1 },
                    "column": { "type": "integer", "minimum": 1 },
                    "end_line": { "type": "integer", "minimum": 1 },
                    "end_column": { "type": "integer", "minimum": 1 },
                    "columns": { "type": "boolean" },
                }),
                &["id", "line", "column"],
            ),
            read_only: false,
            destructive: false,
            handler: doc_select,
        },
        ToolDef {
            name: "doc_expand_selection",
            title: "Expand the selection",
            description: "Grow the selection of an open document, `times` times (default 1), by what the syntax says: the word, the value, the attribute, the tag, the content of the element, \
                          the element and the one around it. Starts from `line` and `column` (and `end_line`, `end_column`) when they are given. Returns where the selection is and whether it grew.",
            input_schema: object(
                json!({
                    "id": id.clone(),
                    "times": { "type": "integer", "minimum": 1, "maximum": 100 },
                    "line": { "type": "integer", "minimum": 1 },
                    "column": { "type": "integer", "minimum": 1 },
                    "end_line": { "type": "integer", "minimum": 1 },
                    "end_column": { "type": "integer", "minimum": 1 },
                }),
                &["id"],
            ),
            read_only: false,
            destructive: false,
            handler: doc_expand_selection,
        },
        ToolDef {
            name: "doc_shrink_selection",
            title: "Shrink the selection",
            description: "Go back one step of doc_expand_selection (not after the text changed or the selection was set another way). Returns where the selection is and whether it shrank.",
            input_schema: object(json!({ "id": id.clone() }), &["id"]),
            read_only: false,
            destructive: false,
            handler: doc_shrink_selection,
        },
        ToolDef {
            name: "doc_toggle_comment",
            title: "Comment out or in",
            description: "Wrap the lines of the selection of an open document in `<!-- -->`, or take the markers off when they are one comment. The selection is the one in the document, or \
                          `line`, `column` (and `end_line`, `end_column`) when given. Refused, with nothing changed, when the text has `--` in it.",
            input_schema: object(
                json!({
                    "id": id.clone(),
                    "line": { "type": "integer", "minimum": 1 },
                    "column": { "type": "integer", "minimum": 1 },
                    "end_line": { "type": "integer", "minimum": 1 },
                    "end_column": { "type": "integer", "minimum": 1 },
                }),
                &["id"],
            ),
            read_only: false,
            destructive: false,
            handler: doc_toggle_comment,
        },
        ToolDef {
            name: "doc_undo",
            title: "Undo or redo",
            description: "Undo (or, with redo: true, redo) the last change of a document. Returns whether anything was undone.",
            input_schema: object(json!({ "id": id.clone(), "redo": { "type": "boolean" } }), &["id"]),
            read_only: false,
            destructive: false,
            handler: doc_undo,
        },
        ToolDef {
            name: "doc_set_encoding",
            title: "Change a document's encoding",
            description: "Write the document in another encoding from now on (its XML declaration is brought in line). A byte-order mark is only possible for \
                          UTF-8 and UTF-16.",
            input_schema: object(
                json!({ "id": id.clone(), "encoding": { "type": "string", "description": "A label such as UTF-8, UTF-16LE or windows-1252" }, "bom": { "type": "boolean" } }),
                &["id", "encoding"],
            ),
            read_only: false,
            destructive: false,
            handler: doc_set_encoding,
        },
        ToolDef {
            name: "doc_save",
            title: "Save a document",
            description: "Write a document to its file, or to `path` (the document then belongs to that file). Nothing is replaced silently: a file that changed on disk \
                          since it was read needs force: true; a file with bytes that were not valid cannot be overwritten (save it elsewhere); a character the encoding \
                          cannot hold is an error unless unmappable is \"references\", which writes it as a character reference where XML allows one.",
            input_schema: object(
                json!({
                    "id": id.clone(),
                    "path": { "type": "string", "description": "Where to save (default: the document's own file)" },
                    "force": { "type": "boolean", "description": "Replace the file even though it changed on disk since it was read" },
                    "unmappable": { "type": "string", "enum": ["error", "references"] },
                }),
                &["id"],
            ),
            read_only: false,
            destructive: true,
            handler: doc_save,
        },
        ToolDef {
            name: "doc_close",
            title: "Close a document",
            description: "Close a document. One with unsaved changes is only closed with discard: true.",
            input_schema: object(json!({ "id": id, "discard": { "type": "boolean" } }), &["id"]),
            read_only: false,
            destructive: true,
            handler: doc_close,
        },
        ToolDef {
            name: "doc_find",
            title: "Find text",
            description: "Find text or a regular expression in an open document (`id`), a file (`path`) or text. Returns each match with its position (line and column \
                          from 1), its text (cut after 200 characters, then `text_truncated`), and whether there were more than `limit`. In a regular expression `^` and `$` match at every line.",
            input_schema: {
                let mut properties = source_properties();
                if let Some(map) = properties.as_object_mut() {
                    for (key, value) in search_properties().as_object().into_iter().flatten() {
                        map.insert(key.clone(), value.clone());
                    }
                    map.insert(
                        "limit".into(),
                        json!({ "type": "integer", "minimum": 1, "maximum": 10000, "description": "At most this many matches (default 1000)" }),
                    );
                }
                object(properties, &["pattern"])
            },
            read_only: true,
            destructive: false,
            handler: doc_find,
        },
        ToolDef {
            name: "doc_replace",
            title: "Replace text",
            description: "Replace every match of a text or regular expression in an open document, as one undoable step. In a regular expression the replacement may use \
                          $1 or ${name} for groups ($$ is a dollar sign). Nothing is changed when there are more than 10000 matches or the replacements would add up to more than 16 MiB.",
            input_schema: {
                let mut properties = search_properties();
                if let Some(map) = properties.as_object_mut() {
                    map.insert("id".into(), json!({ "type": "integer", "minimum": 0, "description": "The document's id" }));
                    map.insert("replacement".into(), json!({ "type": "string" }));
                }
                object(properties, &["id", "pattern", "replacement"])
            },
            read_only: false,
            destructive: false,
            handler: doc_replace,
        },
        ToolDef {
            name: "xml_wellformed",
            title: "Check well-formedness",
            description: "Check XML 1.0 well-formedness and Namespaces, of an open document (`id`), a file (`path`) or text. Returns the counts and the problems with \
                          their position and message.",
            input_schema: object(source_properties(), &[]),
            read_only: true,
            destructive: false,
            handler: xml_wellformed,
        },
        ToolDef {
            name: "xml_matching_tag",
            title: "Find the matching tag",
            description: "The other tag of the element a position is in or next to, in an open document (`id`), a file (`path`) or text: give a position as `line` and \
                          `column` (from 1) or as a byte `offset`. Returns the tag and its name, the other tag and its name, and where the caret would go \
                          (`jump_to`); `match` is null where the position is in no tag, or the tag has no other (an empty-element tag, an element that is not \
                          closed).",
            input_schema: object(source_and_position_properties(), &[]),
            read_only: true,
            destructive: false,
            handler: xml_matching_tag,
        },
        ToolDef {
            name: "xml_folds",
            title: "What can be folded",
            description: "The stretches of lines that can be folded in an open document (`id`), a file (`path`) or text: elements whose content is on lines of their own, \
                          and comments, CDATA sections, processing instructions and DOCTYPEs over several lines. Each has the line that stands for it (`first_line`), the \
                          last line it hides (`last_line`), the line it ends on, its level (1 is the root element) and its kind. Lines count from 1.",
            input_schema: object(source_properties(), &[]),
            read_only: true,
            destructive: false,
            handler: xml_folds,
        },
        ToolDef {
            name: "xml_outline",
            title: "Outline of the elements",
            description: "The element tree of an open document (`id`), a file (`path`) or text: each element with its depth, name, a hint from its id, name or key \
                          attribute, and where it starts and ends.",
            input_schema: object(source_properties(), &[]),
            read_only: true,
            destructive: false,
            handler: xml_outline,
        },
        ToolDef {
            name: "xml_format",
            title: "Pretty-print or minify",
            description: "Change only the white space between nodes: pretty-print (one node per line, indented) or minify. Text with mixed content and elements with \
                          xml:space=\"preserve\" are left alone, and a document that is not well-formed is refused. On an open document (`id`) it is one undoable edit; \
                          for a file or text the formatted text is returned and nothing is written.",
            input_schema: {
                let mut properties = source_properties();
                if let Some(map) = properties.as_object_mut() {
                    map.insert("minify".into(), json!({ "type": "boolean", "description": "Strip insignificant white space instead of indenting" }));
                    map.insert(
                        "indent".into(),
                        json!({ "type": "integer", "minimum": 0, "maximum": 16, "description": "Spaces per level (default: the document's)" }),
                    );
                    map.insert("tabs".into(), json!({ "type": "boolean", "description": "Indent with tabs" }));
                    map.insert("blank_lines".into(), json!({ "type": "string", "enum": ["keep", "remove"], "description": "A run of blank lines between nodes becomes one (keep, the default) or none" }));
                }
                object(properties, &[])
            },
            read_only: false,
            destructive: false,
            handler: xml_format,
        },
    ]
}

// --- helpers -------------------------------------------------------------------------------------------------------------------

fn id_arg(args: &Value) -> Result<u64> {
    args.get("id").and_then(Value::as_u64).ok_or_else(|| ToolError::InvalidArgs("id is required".into()))
}

fn text_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn flag(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// The index of the document with this id.
pub(crate) fn index_of(a: &Automation, id: u64) -> Result<usize> {
    a.session.position_of(id).ok_or_else(|| ToolError::Failed(format!("no document has the id {id} (doc_list shows the open ones)")))
}

pub(crate) fn describe(doc: &Document) -> Value {
    let encoding = doc.encoding();
    let endings = doc.line_endings();
    json!({
        "id": doc.id(),
        "name": doc.name(),
        "path": doc.path().map(|p| p.display().to_string()),
        "dirty": doc.is_dirty(),
        "lossy": doc.is_lossy(),
        "bytes": doc.editor().buffer().len(),
        "lines": doc.editor().buffer().line_count(),
        "encoding": encoding.encoding.name(),
        "bom": encoding.bom,
        "line_ending": doc.editor().settings().eol.label(),
        "mixed_line_endings": endings.is_mixed(),
    })
}

pub(crate) fn json_result(value: Value) -> Result<Vec<Content>> {
    Ok(vec![Content::Json(value)])
}

/// The place in `text` that `line` and `column` (from 1) or `offset` (a byte offset on a character boundary) name.
pub(crate) fn position_arg(text: &str, buffer: &Buffer, args: &Value) -> Result<usize> {
    match args.get("offset").and_then(Value::as_u64) {
        Some(offset) => usize::try_from(offset)
            .ok()
            .filter(|o| *o <= text.len() && text.is_char_boundary(*o))
            .ok_or_else(|| ToolError::InvalidArgs(format!("offset: not a position in the text ({} bytes, on a character boundary)", text.len()))),
        None => offset_from(buffer, args.get("line").and_then(Value::as_u64), args.get("column").and_then(Value::as_u64), "position"),
    }
}

/// A 1-based (line, column) argument pair to an offset; the end defaults to the start.
fn offset_from(buffer: &Buffer, line: Option<u64>, column: Option<u64>, what: &str) -> Result<usize> {
    let to_index = |v: Option<u64>, name: &str| -> Result<usize> {
        let v = v.ok_or_else(|| ToolError::InvalidArgs(format!("{what}: {name} is missing")))?;
        usize::try_from(v.saturating_sub(1)).map_err(|_| ToolError::InvalidArgs(format!("{what}: {name} is too large")))
    };
    let (line, col) = (to_index(line, "line")?, to_index(column, "column")?);
    if line >= buffer.line_count() {
        return fail(format!("{what}: the document has {} lines, there is no line {}", buffer.line_count(), line + 1));
    }
    Ok(buffer.offset_of(Position { line, col }))
}

pub(crate) fn position_json(buffer: &Buffer, offset: usize) -> (usize, usize) {
    let p = buffer.position(offset);
    (p.line + 1, p.col + 1)
}

pub(crate) fn severity_name(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
    }
}

fn problem_json(buffer: &Buffer, d: &Diagnostic) -> Value {
    let (line, column) = position_json(buffer, d.span.start);
    let (end_line, end_column) = position_json(buffer, d.span.end);
    json!({
        "severity": severity_name(d.severity()),
        "code": d.code.key(),
        "message": d.message(),
        "line": line, "column": column, "end_line": end_line, "end_column": end_column,
        "start": d.span.start, "end": d.span.end,
    })
}

/// The XML a tool works on: the text, and the open document it came from.
pub(crate) struct Source {
    pub(crate) text: String,
    pub(crate) id: Option<u64>,
    /// A file whose bytes were not valid in its encoding was read (they are shown as U+FFFD).
    pub(crate) lossy: bool,
    /// Where it is, when it is a file (the document is saved, or it was named by `path`).
    pub(crate) path: Option<std::path::PathBuf>,
}

pub(crate) fn source(a: &Automation, args: &Value) -> Result<Source> {
    let id = args.get("id").and_then(Value::as_u64);
    let path = text_arg(args, "path");
    let text = text_arg(args, "text");
    match (id, path, text) {
        (Some(id), None, None) => {
            let doc = a.session.document(index_of(a, id)?).ok_or_else(|| ToolError::Failed(format!("no document has the id {id}")))?;
            Ok(Source { text: doc.editor().text(), id: Some(id), lossy: doc.is_lossy(), path: doc.path().map(std::path::Path::to_path_buf) })
        }
        (None, Some(path), None) => {
            let path = a.confinement.resolve_read(path)?;
            let loaded = xmlcraft_platform::files::read_file(&path).map_err(|e| ToolError::Failed(format!("{}: {e}", path.display())))?;
            let decoded = decode(&loaded.bytes, &sniff(&loaded.bytes));
            Ok(Source { text: decoded.text, id: None, lossy: decoded.lossy, path: Some(path) })
        }
        (None, None, Some(text)) => Ok(Source { text: text.to_owned(), id: None, lossy: false, path: None }),
        _ => Err(ToolError::InvalidArgs("give exactly one of id, path and text".into())),
    }
}

fn analysis_of(source: &Source) -> Result<Analysis> {
    let analysis = analyze(&source.text, 0);
    match analysis.skipped {
        Some(Skipped::TooLarge) => fail("the text is too large to analyse"),
        Some(Skipped::Failed) => fail("the analysis failed (a bug); nothing was changed"),
        None => Ok(analysis),
    }
}

// --- documents ---------------------------------------------------------------------------------------------------------------

fn doc_open(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let path = text_arg(args, "path").unwrap_or_default();
    let path = a.confinement.resolve_read(path)?;
    let loaded = xmlcraft_platform::files::read_file(&path).map_err(|e| ToolError::Failed(format!("{}: {e}", path.display())))?;
    let before = a.session.documents().len();
    let index = a.session.open(Some(path), &loaded.bytes, Some(loaded.stamp)).map_err(|e| ToolError::Failed(e.to_string()))?;
    let opened_now = a.session.documents().len() > before;
    let Some(doc) = a.session.document(index) else { return fail("the document disappeared") };
    let mut value = describe(doc);
    value["already_open"] = json!(!opened_now);
    json_result(value)
}

fn doc_new(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let index = a.session.new_document();
    if let Some(text) = text_arg(args, "text")
        && let Some(doc) = a.session.document_mut(index)
    {
        doc.editor_mut().load(text);
    }
    let Some(doc) = a.session.document(index) else { return fail("the document disappeared") };
    json_result(describe(doc))
}

fn doc_list(a: &mut Automation, _: &Value) -> Result<Vec<Content>> {
    let documents: Vec<Value> = a.session.documents().iter().map(describe).collect();
    json_result(json!({ "documents": documents, "count": documents.len() }))
}

fn recent_files(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let files: Vec<String> = a.session.recent().paths().iter().map(|path| path.display().to_string()).collect();
    if flag(args, "clear") {
        a.session.clear_recent();
    }
    json_result(json!({ "files": files, "count": files.len(), "cleared": flag(args, "clear") }))
}

fn doc_text(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let index = index_of(a, id_arg(args)?)?;
    let Some(doc) = a.session.document(index) else { return fail("the document disappeared") };
    let buffer = doc.editor().buffer();
    let line_count = buffer.line_count();
    let from = args.get("from_line").and_then(Value::as_u64).map_or(1, |v| usize::try_from(v).unwrap_or(usize::MAX));
    let to = args.get("to_line").and_then(Value::as_u64).map_or(line_count, |v| usize::try_from(v).unwrap_or(usize::MAX)).min(line_count);
    if from > to {
        return fail(format!("from_line {from} is after to_line {to} (the document has {line_count} lines)"));
    }
    let start = buffer.line_start(from - 1).unwrap_or(buffer.len());
    let end = buffer.line_range_with_break(to - 1).map_or(buffer.len(), |r| r.end);
    if end.saturating_sub(start) > MAX_TEXT_BYTES {
        return fail(format!("that is {} bytes; at most {MAX_TEXT_BYTES} are returned at once (ask for fewer lines)", end - start));
    }
    let text = buffer.slice(start..end).map_err(|e| ToolError::Failed(e.to_string()))?;
    json_result(json!({ "id": doc.id(), "text": text, "from_line": from, "to_line": to, "lines": line_count }))
}

fn doc_edit(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let index = index_of(a, id_arg(args)?)?;
    let Some(doc) = a.session.document_mut(index) else { return fail("the document disappeared") };
    let mut edits = Vec::new();
    for (i, e) in args.get("edits").and_then(Value::as_array).into_iter().flatten().enumerate() {
        let what = format!("edit {}", i + 1);
        let get = |k: &str| e.get(k).and_then(Value::as_u64);
        let buffer = doc.editor().buffer();
        let start = offset_from(buffer, get("line"), get("column"), &what)?;
        let end = if get("end_line").is_some() || get("end_column").is_some() {
            offset_from(buffer, get("end_line").or(get("line")), get("end_column").or(get("column")), &what)?
        } else {
            start
        };
        if end < start {
            return fail(format!("{what}: the end is before the start"));
        }
        edits.push(Edit::replace(start..end, text_arg(e, "text").unwrap_or_default().to_owned()));
    }
    edits.sort_by_key(|e| (e.range.start, e.range.end));
    if edits.windows(2).any(|w| w[0].range.end > w[1].range.start) {
        return fail("the edits overlap");
    }
    let changed = doc.editor_mut().apply_edits(&edits);
    json_result(json!({ "changed": changed, "document": describe(doc) }))
}

/// Put the caret at the `line` and `column` (from 1) of `args`, when they are given (both or neither).
fn place_caret(doc: &mut Document, args: &Value) -> Result<()> {
    match (args.get("line"), args.get("column")) {
        (None, None) => return Ok(()),
        (Some(_), Some(_)) => {}
        _ => return Err(ToolError::InvalidArgs("line and column go together: give both, or neither for the caret".into())),
    }
    let at = offset_from(doc.editor().buffer(), args.get("line").and_then(Value::as_u64), args.get("column").and_then(Value::as_u64), "position")?;
    doc.editor_mut().place_caret(at, false);
    Ok(())
}

/// Put the selection at `line`/`column` (to `end_line`/`end_column` when those are given), when they are given.
fn place_selection(doc: &mut Document, args: &Value) -> Result<()> {
    match (args.get("end_line"), args.get("end_column")) {
        (None, None) => return place_caret(doc, args),
        (Some(_), Some(_)) => {}
        _ => return Err(ToolError::InvalidArgs("end_line and end_column go together: give both, or neither".into())),
    }
    let buffer = doc.editor().buffer();
    let from = offset_from(buffer, args.get("line").and_then(Value::as_u64), args.get("column").and_then(Value::as_u64), "start")?;
    let to = offset_from(buffer, args.get("end_line").and_then(Value::as_u64), args.get("end_column").and_then(Value::as_u64), "end")?;
    doc.editor_mut().set_selection(from, to);
    Ok(())
}

fn doc_select(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let index = index_of(a, id_arg(args)?)?;
    let Some(doc) = a.session.document_mut(index) else { return fail("the document disappeared") };
    if flag(args, "columns") {
        if args.get("end_line").is_none() || args.get("end_column").is_none() {
            return Err(ToolError::InvalidArgs("columns needs end_line and end_column: the other corner of the rectangle".into()));
        }
        let buffer = doc.editor().buffer();
        let from = offset_from(buffer, args.get("line").and_then(Value::as_u64), args.get("column").and_then(Value::as_u64), "start")?;
        let to = offset_from(buffer, args.get("end_line").and_then(Value::as_u64), args.get("end_column").and_then(Value::as_u64), "end")?;
        doc.editor_mut().select_columns(from, to);
    } else {
        place_selection(doc, args)?;
    }
    json_result(json!({ "carets": carets_json(doc) }))
}

fn doc_expand_selection(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let index = index_of(a, id_arg(args)?)?;
    let Some(doc) = a.session.document_mut(index) else { return fail("the document disappeared") };
    place_selection(doc, args)?;
    let times = args.get("times").and_then(Value::as_u64).map_or(1, |t| usize::try_from(t).unwrap_or(1)).min(100);
    let mut grew = false;
    for _ in 0..times {
        grew |= doc.editor_mut().expand_selection();
    }
    json_result(json!({ "grew": grew, "carets": carets_json(doc) }))
}

fn doc_shrink_selection(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let index = index_of(a, id_arg(args)?)?;
    let Some(doc) = a.session.document_mut(index) else { return fail("the document disappeared") };
    let shrank = doc.editor_mut().shrink_selection();
    json_result(json!({ "shrank": shrank, "carets": carets_json(doc) }))
}

fn doc_toggle_comment(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let index = index_of(a, id_arg(args)?)?;
    let Some(doc) = a.session.document_mut(index) else { return fail("the document disappeared") };
    let before = doc.editor().selections();
    place_selection(doc, args)?;
    let changed = match doc.editor_mut().toggle_comment() {
        Ok(changed) => changed,
        Err(refused) => {
            // Nothing is changed: not the text, and not where the selection was either.
            doc.editor_mut().set_selections(&before);
            return Err(ToolError::Failed(format!("{refused}: nothing was changed")));
        }
    };
    json_result(json!({ "changed": changed, "carets": carets_json(doc), "document": describe(doc) }))
}

/// Where the carets of `doc` are, as the tools write positions.
fn carets_json(doc: &Document) -> Vec<Value> {
    let buffer = doc.editor().buffer();
    doc.editor().carets().iter().map(|c| range_json(buffer, &(c.start()..c.end()))).collect()
}

/// The most characters `doc_type` types in one call.
const MAX_TYPED: usize = 100_000;

fn doc_type(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let index = index_of(a, id_arg(args)?)?;
    let Some(doc) = a.session.document_mut(index) else { return fail("the document disappeared") };
    let text = text_arg(args, "text").unwrap_or_default();
    if text.chars().count() > MAX_TYPED {
        return fail(format!("at most {MAX_TYPED} characters can be typed at once (use doc_edit for more)"));
    }
    // What is typed is checked first: a call that cannot be done does nothing. A line break of any kind is one Enter.
    if let Some(c) = text.chars().find(|c| c.is_control() && !matches!(c, '\n' | '\t' | '\r')) {
        return fail(format!("the control character U+{:04X} cannot be typed", u32::from(c)));
    }
    place_caret(doc, args)?;
    // As in the window: typing in the name of a tag renames the other tag of the element too.
    doc.link_tag_names();
    let mut typed = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let editor = doc.editor_mut();
        match c {
            '\r' | '\n' => {
                if c == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                let _ = editor.insert_newline();
            }
            '\t' => drop(editor.indent()),
            c => drop(editor.type_input(&c.to_string())),
        }
        typed += 1;
    }
    json_result(json!({ "typed": typed, "carets": carets_json(doc), "document": describe(doc) }))
}

fn doc_key(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let index = index_of(a, id_arg(args)?)?;
    let Some(doc) = a.session.document_mut(index) else { return fail("the document disappeared") };
    let key = text_arg(args, "key").unwrap_or_default();
    let times = args.get("times").and_then(Value::as_u64).map_or(1, |t| usize::try_from(t).unwrap_or(1)).min(1000);
    place_caret(doc, args)?;
    doc.link_tag_names();
    for _ in 0..times {
        let editor = doc.editor_mut();
        match key {
            "Backspace" => drop(editor.backspace()),
            "Delete" => drop(editor.delete_forward()),
            "Enter" => drop(editor.insert_newline()),
            "Tab" => drop(editor.indent()),
            "Shift+Tab" => drop(editor.outdent()),
            other => return fail(format!("{other}: no such key (Backspace, Delete, Enter, Tab, Shift+Tab)")),
        }
    }
    json_result(json!({ "carets": carets_json(doc), "document": describe(doc) }))
}

fn fold_json(range: &FoldRange) -> Value {
    json!({
        "first_line": range.first_line + 1, "last_line": range.last_line + 1, "end_line": range.end_line + 1,
        "level": range.level, "kind": range.kind.name(),
    })
}

fn xml_folds(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let source = source(a, args)?;
    let analysis = analysis_of(&source)?;
    let folds: Vec<Value> = analysis.folds.iter().map(fold_json).collect();
    json_result(json!({
        "folds": folds, "count": folds.len(), "truncated": analysis.folds_truncated,
        "well_formed": analysis.is_well_formed(), "lossy": source.lossy,
    }))
}

fn doc_fold(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let index = index_of(a, id_arg(args)?)?;
    let Some(doc) = a.session.document_mut(index) else { return fail("the document disappeared") };
    let action = match text_arg(args, "action").unwrap_or_default() {
        "fold" => FoldAction::Fold,
        "unfold" => FoldAction::Unfold,
        "fold_all" => FoldAction::FoldAll,
        "unfold_all" => FoldAction::UnfoldAll,
        "level" => match args.get("level").and_then(Value::as_u64) {
            Some(level) => FoldAction::Level(usize::try_from(level).unwrap_or(usize::MAX)),
            None => return Err(ToolError::InvalidArgs("level: the level is missing".into())),
        },
        other => return fail(format!("{other}: no such action (fold, unfold, fold_all, unfold_all, level)")),
    };
    // `line` and `column` say which line to fold or unfold at; the caret is not moved (a caret in a hidden line would open the fold).
    let line = match (args.get("line"), args.get("column")) {
        (None, None) => doc.editor().buffer().line_of(doc.editor().primary().head()),
        (Some(_), Some(_)) => {
            let at =
                offset_from(doc.editor().buffer(), args.get("line").and_then(Value::as_u64), args.get("column").and_then(Value::as_u64), "position")?;
            doc.editor().buffer().line_of(at)
        }
        _ => return Err(ToolError::InvalidArgs("line and column go together: give both, or neither for the caret's line".into())),
    };
    // Opening everything needs no analysis (and works on a text too large to have one).
    let (ranges, truncated) = if action == FoldAction::UnfoldAll {
        (Vec::new(), false)
    } else {
        let analysis = analyze_guarded(&doc.editor().text(), 0);
        if analysis.skipped.is_some() {
            return fail("the text is too large to analyse, or the analysis failed: nothing was folded");
        }
        (analysis.folds, analysis.folds_truncated)
    };
    let changed = xmlcraft_engine::folding::apply_at(doc.editor_mut(), &ranges, action, line);
    let buffer = doc.editor().buffer();
    let folded: Vec<Value> = doc
        .editor()
        .folds()
        .iter()
        .map(|f| {
            let (first, last) = (buffer.line_of(f.hidden.start), buffer.line_of(f.hidden.end));
            json!({ "first_line": first + 1, "last_line": last + 1 })
        })
        .collect();
    json_result(json!({
        "changed": changed, "folded": folded, "visible_lines": doc.editor().visible_rows(), "truncated": truncated,
        "carets": carets_json(doc), "document": describe(doc),
    }))
}

fn doc_set_text(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let index = index_of(a, id_arg(args)?)?;
    let Some(doc) = a.session.document_mut(index) else { return fail("the document disappeared") };
    let new = text_arg(args, "text").unwrap_or_default().to_owned();
    let len = doc.editor().buffer().len();
    let changed = doc.editor_mut().apply_edits(&[Edit::replace(0..len, new)]);
    json_result(json!({ "changed": changed, "document": describe(doc) }))
}

fn doc_undo(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let index = index_of(a, id_arg(args)?)?;
    let Some(doc) = a.session.document_mut(index) else { return fail("the document disappeared") };
    let done = if flag(args, "redo") { doc.editor_mut().redo() } else { doc.editor_mut().undo() };
    json_result(json!({ "done": done, "document": describe(doc) }))
}

fn doc_set_encoding(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let index = index_of(a, id_arg(args)?)?;
    let label = text_arg(args, "encoding").unwrap_or_default();
    let Some(encoding) = Encoding::from_label(label) else { return fail(format!("{label:?} is not an encoding XmlCraft knows")) };
    let Some(doc) = a.session.document_mut(index) else { return fail("the document disappeared") };
    let changed = doc.set_encoding(encoding, flag(args, "bom"));
    json_result(json!({ "changed": changed, "document": describe(doc) }))
}

fn doc_save(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    use xmlcraft_platform::files::{self, DiskChange};
    let index = index_of(a, id_arg(args)?)?;
    let own = a.session.document(index).and_then(|d| d.path().map(PathBuf::from));
    let target: PathBuf = match text_arg(args, "path") {
        Some(path) => a.confinement.resolve_write(path)?,
        None => own.clone().ok_or_else(|| ToolError::Failed("the document has no file yet: give a path".into()))?,
    };
    let Some(doc) = a.session.document_mut(index) else { return fail("the document disappeared") };
    // The same file by its identity, not by how the path is spelled (a link to it, another route to the same folder).
    let same_file = own.as_deref() == Some(target.as_path())
        || matches!((doc.stamp().and_then(|s| s.file_id), files::stamp_of(&target).ok().and_then(|s| s.file_id)), (Some(a), Some(b)) if a == b);
    if same_file && !doc.can_overwrite_source() {
        return fail(
            "the file had bytes that were not valid in its encoding, and saving over it would replace them for good: save under another path",
        );
    }
    if same_file
        && !flag(args, "force")
        && let Some(stamp) = doc.stamp()
    {
        match files::changed_on_disk(&target, stamp) {
            DiskChange::Unchanged => {}
            DiskChange::Modified => {
                return fail(format!(
                    "{} changed on disk since it was read: reopen it, save under another path, or pass force: true to replace it",
                    target.display()
                ));
            }
            DiskChange::Missing => {
                return fail(format!(
                    "{} is gone (or cannot be read) since it was read: save under another path, or pass force: true to write it again",
                    target.display()
                ));
            }
        }
    }
    let bytes = match doc.to_bytes() {
        Ok(bytes) => bytes,
        Err(SaveError::Unmappable(e)) if text_arg(args, "unmappable") == Some("references") => {
            doc.apply_character_references().map_err(|_| unmappable_error(doc, e))?;
            doc.to_bytes().map_err(|_| unmappable_error(doc, e))?
        }
        Err(SaveError::Unmappable(e)) => return Err(unmappable_error(doc, e)),
        Err(SaveError::Lossy) => return fail("the document cannot be saved over its file"),
    };
    let stamp = files::write_file(&target, &bytes).map_err(|e| ToolError::Failed(format!("{}: {e}", target.display())))?;
    doc.mark_saved(target.clone(), Some(stamp));
    let described = describe(doc);
    a.session.remember_recent(target.clone());
    json_result(json!({ "saved": target.display().to_string(), "bytes": bytes.len(), "document": described }))
}

fn unmappable_error(doc: &Document, e: xmlcraft_text::encoding::EncodeError) -> ToolError {
    let line = doc.editor().buffer().line_of(e.offset) + 1;
    ToolError::Failed(format!(
        "the character U+{:04X} on line {line} cannot be written in {}: change the encoding (doc_set_encoding) or pass unmappable: \"references\" \
         (a character reference is only possible in text and attribute values)",
        u32::from(e.ch),
        doc.encoding().encoding.name()
    ))
}

fn doc_close(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let index = index_of(a, id_arg(args)?)?;
    let dirty = a.session.document(index).is_some_and(Document::is_dirty);
    if dirty && !flag(args, "discard") {
        return fail("the document has unsaved changes: save it (doc_save) or pass discard: true");
    }
    let closed = a.session.close(index);
    json_result(json!({ "closed": closed.is_some(), "discarded_changes": dirty }))
}

// --- find and replace --------------------------------------------------------------------------------------------------------

/// The most characters of a match `doc_find` reports as its text.
const MATCH_TEXT_CHARS: usize = 200;

fn matcher_of(args: &Value) -> Result<Matcher> {
    let query = Query {
        pattern: text_arg(args, "pattern").unwrap_or_default().to_owned(),
        regex: flag(args, "regex"),
        case_sensitive: args.get("case_sensitive").and_then(Value::as_bool).unwrap_or(true),
        whole_word: flag(args, "whole_word"),
    };
    Matcher::new(&query).map_err(|e| ToolError::InvalidArgs(e.to_string()))
}

fn doc_find(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let matcher = matcher_of(args)?;
    let source = source(a, args)?;
    let limit = args.get("limit").and_then(Value::as_u64).map_or(1000, |l| usize::try_from(l).unwrap_or(MAX_MATCHES)).min(MAX_MATCHES);
    let (found, more) = matcher.find_all(&source.text, 0..source.text.len(), limit);
    let buffer = Buffer::from_text(&source.text);
    let matches: Vec<Value> = found
        .iter()
        .map(|range| {
            let (line, column) = position_json(&buffer, range.start);
            let (end_line, end_column) = position_json(&buffer, range.end);
            let whole = source.text.get(range.clone()).unwrap_or_default();
            let text: String = whole.chars().take(MATCH_TEXT_CHARS).collect();
            // `start` and `end` are the whole match; `text` is cut for a match longer than MATCH_TEXT_CHARS.
            let truncated = text.len() < whole.len();
            json!({
                "line": line, "column": column, "end_line": end_line, "end_column": end_column,
                "start": range.start, "end": range.end, "text": text, "text_truncated": truncated,
            })
        })
        .collect();
    json_result(json!({ "matches": matches, "count": matches.len(), "more": more, "lossy": source.lossy }))
}

fn doc_replace(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let matcher = matcher_of(args)?;
    let index = index_of(a, id_arg(args)?)?;
    let Some(doc) = a.session.document_mut(index) else { return fail("the document disappeared") };
    let replacement = text_arg(args, "replacement").unwrap_or_default();
    let count = doc
        .editor_mut()
        .replace_all(&matcher, replacement, false)
        .map_err(|refused| ToolError::Failed(format!("{refused}: nothing was replaced (make the pattern narrower)")))?;
    json_result(json!({ "replaced": count, "document": describe(doc) }))
}

// --- XML ---------------------------------------------------------------------------------------------------------------------

fn xml_wellformed(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let source = source(a, args)?;
    let analysis = analysis_of(&source)?;
    let buffer = Buffer::from_text(&source.text);
    let problems: Vec<Value> = analysis.diagnostics.iter().take(MAX_PROBLEMS).map(|d| problem_json(&buffer, d)).collect();
    json_result(json!({
        "well_formed": analysis.is_well_formed(),
        "errors": analysis.error_count(),
        "warnings": analysis.warning_count(),
        "problems": problems,
        "truncated": analysis.diagnostics.len() > MAX_PROBLEMS,
        "lossy": source.lossy,
    }))
}

fn range_json(buffer: &Buffer, range: &std::ops::Range<usize>) -> Value {
    let (line, column) = position_json(buffer, range.start);
    let (end_line, end_column) = position_json(buffer, range.end);
    json!({ "line": line, "column": column, "end_line": end_line, "end_column": end_column, "start": range.start, "end": range.end })
}

fn xml_matching_tag(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let source = source(a, args)?;
    let buffer = Buffer::from_text(&source.text);
    let offset = position_arg(&source.text, &buffer, args)?;
    let analysis = analysis_of(&source)?;
    let found = analysis.matching_tag(offset).map(|m| {
        json!({
            "tag": range_json(&buffer, &m.tag),
            "name": range_json(&buffer, &m.name),
            "partner": range_json(&buffer, &m.partner),
            "partner_name": range_json(&buffer, &m.partner_name),
            "jump_to": range_json(&buffer, &(m.jump_target(offset)..m.jump_target(offset))),
        })
    });
    json_result(json!({ "match": found, "lossy": source.lossy }))
}

fn xml_outline(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let source = source(a, args)?;
    let analysis = analysis_of(&source)?;
    let buffer = Buffer::from_text(&source.text);
    let items: Vec<Value> = analysis
        .outline
        .iter()
        .take(MAX_OUTLINE)
        .map(|item| {
            let (line, column) = position_json(&buffer, item.name_range.start);
            let (end_line, end_column) = position_json(&buffer, item.range.end);
            json!({
                "depth": item.depth, "name": item.name, "label": item.label,
                "line": line, "column": column, "end_line": end_line, "end_column": end_column,
                "start": item.range.start, "end": item.range.end,
            })
        })
        .collect();
    json_result(json!({
        "items": items,
        "count": analysis.outline.len(),
        "truncated": analysis.outline.len() > MAX_OUTLINE || analysis.outline_truncated,
        "well_formed": analysis.is_well_formed(),
    }))
}

fn xml_format(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let source = source(a, args)?;
    let minify = flag(args, "minify");
    // The text's own style: its indentation and its line ending (an open document knows them; for a file or text they are read from it).
    let mut options = match source.id.and_then(|id| a.session.position_of(id)).and_then(|i| a.session.document(i)) {
        Some(doc) => doc.format_options(),
        None => Document::from_bytes(0, None, source.text.as_bytes(), None).map(|doc| doc.format_options()).unwrap_or_default(),
    };
    if flag(args, "tabs") {
        options.indent = Indent::Tab;
    } else if let Some(n) = args.get("indent").and_then(Value::as_u64) {
        options.indent = Indent::Spaces(usize::try_from(n).unwrap_or(usize::MAX));
    }
    options.blank_lines = match text_arg(args, "blank_lines") {
        Some("remove") => BlankLines::Remove,
        _ => BlankLines::Keep,
    };
    let edits = if minify { xmlcraft_format::minify(&source.text) } else { xmlcraft_format::pretty_print_settled(&source.text, &options) };
    let edits = edits.map_err(|FormatError::NotWellFormed { message, offset }| {
        let (line, column) = position_json(&Buffer::from_text(&source.text), offset);
        ToolError::Failed(format!("the document is not well-formed, so it is left as it is (line {line}, column {column}: {message})"))
    })?;
    match source.id {
        Some(id) => {
            let index = index_of(a, id)?;
            let Some(doc) = a.session.document_mut(index) else { return fail("the document disappeared") };
            let changed = !edits.is_empty() && doc.editor_mut().apply_edits(&edits);
            json_result(json!({ "changed": changed, "edits": edits.len(), "document": describe(doc) }))
        }
        None => {
            let formatted = xmlcraft_format::apply(&source.text, &edits);
            if formatted.len() > MAX_TEXT_BYTES {
                return fail(format!(
                    "the result is {} bytes; at most {MAX_TEXT_BYTES} are returned at once (open the file and save it instead)",
                    formatted.len()
                ));
            }
            json_result(json!({ "changed": !edits.is_empty(), "edits": edits.len(), "text": formatted, "lossy": source.lossy }))
        }
    }
}
