//! `xpath_eval`: an XPath 3.1 expression evaluated over a document. `xpath_profile`: the same, profiled (what each part of the
//! expression was evaluated for and what it cost).
//!
//! The document is an open document (`id`), a file (`path`, through the root confinement) or `text`. Nothing is changed.

use serde_json::{Map, Value, json};
use xmlcraft_query::{Evaluated, Limits, Options, Phase, Prepared, Profile, QueryError, evaluate, trace};
use xmlcraft_text::Buffer;

use crate::documents::{fail, json_result, position_json, source, source_properties};
use crate::tools::ToolDef;
use crate::{Automation, Content, Result, ToolError};

/// The most items one answer lists.
const MAX_ITEMS: usize = 1000;
/// The most hot spots and nodes of the tree a profile lists.
const MAX_TOP: usize = 500;
/// The most instructions a profiled run may take.
const MAX_INSTRUCTIONS: u64 = 1_000_000_000;

/// The properties every XPath tool takes: the document, the expression, its prefixes and version, and how many items to list.
pub(crate) fn common_properties() -> Value {
    let mut properties = source_properties();
    if let Some(map) = properties.as_object_mut() {
        map.insert("expression".into(), json!({ "type": "string", "description": "The XPath expression" }));
        map.insert(
            "namespaces".into(),
            json!({ "type": "object", "additionalProperties": { "type": "string" }, "description": "Prefixes the expression uses, with their namespaces (xs, fn, map, array, math and xml are known)" }),
        );
        map.insert(
            "version".into(),
            json!({ "type": "string", "enum": ["1.0", "2.0", "3.0", "3.1"], "description": "The XPath version asked for (default 3.1). The engine is 3.1's: the older versions are subsets of it and run under 3.1's rules (1.0's compatibility mode is not implemented)" }),
        );
        map.insert("max_items".into(), json!({ "type": "integer", "minimum": 1, "maximum": MAX_ITEMS, "description": "The most items listed (default 100; the answer says how many there are)" }));
    }
    properties
}

pub(crate) fn tools() -> Vec<ToolDef> {
    let mut eval = common_properties();
    if let Some(map) = eval.as_object_mut() {
        map.insert("trace".into(), json!({ "type": "boolean", "description": "Also return `steps`: for a path, each step with the number of items the path leaves there and the time it took (an expression that is not a path is one step)" }));
    }
    let mut profiled = common_properties();
    if let Some(map) = profiled.as_object_mut() {
        map.insert(
            "top".into(),
            json!({ "type": "integer", "minimum": 1, "maximum": MAX_TOP, "description": "The most hot spots listed (default 20)" }),
        );
        map.insert(
            "max_instructions".into(),
            json!({ "type": "integer", "minimum": 1, "maximum": MAX_INSTRUCTIONS, "description": "Stop the run after this many engine instructions (default 50 000 000)" }),
        );
    }
    vec![
        ToolDef {
            name: "xpath_eval",
            title: "Evaluate an XPath expression",
            description: "Evaluate an XPath 3.1 expression with the document as the context. Returns `count` and `items`: for a node its `kind` (element, attribute, \
                          text, comment, processing-instruction, document), `name`, `path` (as fn:path writes it), `line` and `column` in the document, and its `text` as \
                          written; for a value its type (xs:string, xs:integer, …, map, array) and its XPath representation. An error has the W3C code (XPST0003 …), says \
                          whether the expression did not compile or failed running, and where in the expression.",
            input_schema: json!({ "type": "object", "properties": eval, "required": ["expression"], "additionalProperties": false }),
            read_only: true,
            destructive: false,
            handler: xpath_eval,
        },
        ToolDef {
            name: "xpath_profile",
            title: "Profile an XPath expression",
            description: "Evaluate an XPath 3.1 expression once with the profiler: for each part of the expression (a step, a predicate, a comparison, a call; \
                          `span` is its place in the expression, in bytes) `hits` (for a step, a predicate or a `for`: the items it was evaluated for; else the \
                          times it was evaluated), `items` it produced, `selected` (the nodes an axis step selected before its predicates), the engine \
                          instructions it ran itself and with what is inside it, and the time (`self_ms`, `total_ms`). Returns `hot` (the costliest first, \
                          at most `top`), `tree` (every part in the order of the text, with its `parent` and `depth`), the totals and the result as \
                          xpath_eval gives it. A run over `max_instructions` is stopped and says so (`stopped`).",
            input_schema: json!({ "type": "object", "properties": profiled, "required": ["expression"], "additionalProperties": false }),
            read_only: true,
            destructive: false,
            handler: xpath_profile,
        },
    ]
}

/// The document, the expression, its options and how many items to list, read from the arguments of an XPath tool.
pub(crate) struct Asked {
    pub(crate) text: String,
    pub(crate) expression: String,
    pub(crate) options: Options,
}

pub(crate) fn asked(a: &Automation, args: &Value, tool: &str) -> Result<Asked> {
    let expression = args.get("expression").and_then(Value::as_str).ok_or_else(|| ToolError::InvalidArgs("`expression` is required".into()))?;
    let s = source(a, args)?;
    if !s.language.is_xml() && args.get("text").is_none() {
        return fail(format!("{tool} reads XML documents (a JSON or YAML document is not XML)"));
    }
    let mut options = Options::default();
    // (The version is checked and given back; the engine is 3.1's whatever is asked.)
    if let Some(version) = args.get("version").and_then(Value::as_str)
        && !matches!(version, "1.0" | "2.0" | "3.0" | "3.1")
    {
        return Err(ToolError::InvalidArgs(format!("version is 1.0, 2.0, 3.0 or 3.1, not {version:?}")));
    }
    match args.get("namespaces") {
        None => {}
        Some(Value::Object(map)) => {
            for (prefix, uri) in map {
                let uri = uri.as_str().ok_or_else(|| ToolError::InvalidArgs(format!("the namespace of {prefix} is a string")))?;
                options.namespaces.push((prefix.clone(), uri.to_owned()));
            }
        }
        Some(_) => return Err(ToolError::InvalidArgs("namespaces is an object of prefixes and namespaces".into())),
    }
    options.max_items = bounded(args, "max_items", 100, MAX_ITEMS)?;
    Ok(Asked { text: s.text, expression: expression.to_owned(), options })
}

/// A whole-number argument from 1 to `max`, `default` when it is not given.
pub(crate) fn bounded(args: &Value, name: &str, default: usize, max: usize) -> Result<usize> {
    match args.get(name) {
        None => Ok(default),
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| (1..=max).contains(n))
            .ok_or_else(|| ToolError::InvalidArgs(format!("{name} is a whole number from 1 to {max}"))),
    }
}

/// The items of a result as the XPath tools list them, with their lines and columns in `text`.
pub(crate) fn items_json(done: &Evaluated, text: &str) -> Vec<Value> {
    let buffer = Buffer::from_text(text);
    done.items.iter().map(|i| item_json(i, &buffer)).collect()
}

pub(crate) fn item_json(i: &xmlcraft_query::ResultItem, buffer: &Buffer) -> Value {
    let mut item = Map::new();
    item.insert("kind".into(), json!(i.kind));
    if !i.name.is_empty() {
        item.insert("name".into(), json!(i.name));
    }
    if let Some(path) = &i.path {
        item.insert("path".into(), json!(path));
    }
    if i.unmapped {
        item.insert("unmapped".into(), json!(true));
    }
    if let Some(span) = &i.span {
        let (line, column) = position_json(buffer, span.start.min(buffer.len()));
        item.insert("line".into(), json!(line));
        item.insert("column".into(), json!(column));
    }
    item.insert("text".into(), json!(i.text));
    if i.cut {
        item.insert("cut".into(), json!(true));
    }
    Value::Object(item)
}

/// An error of an evaluation as the XPath tools say it.
pub(crate) fn error_text(e: &QueryError) -> String {
    let phase = match e.phase {
        Phase::Document => "document",
        Phase::Compile => "compile",
        Phase::Run => "run",
    };
    let place = e.span.as_ref().map(|r| format!(" (at {}..{} of the expression)", r.start, r.end)).unwrap_or_default();
    format!("{phase} error {e}{place}")
}

fn xpath_eval(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let Asked { text, expression, options } = asked(a, args, "xpath_eval")?;
    let want_trace = match args.get("trace") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(ToolError::InvalidArgs("trace is true or false".into())),
    };
    match evaluate(&text, &expression, &options) {
        Ok(done) => {
            let items = items_json(&done, &text);
            let mut answer = json!({ "count": done.count, "truncated": done.count > items.len(), "items": items, "version": args.get("version").cloned().unwrap_or(json!("3.1")), "engine": "XPath 3.1" });
            if want_trace && let Ok(mut prepared) = Prepared::new(&text) {
                let t = trace(&mut prepared, &expression, &options);
                answer["steps"] = json!(t.steps.iter().map(|st| json!({ "step": st.step, "expression": st.expression, "count": st.count, "micros": st.micros, "error": st.error.as_ref().map(ToString::to_string) })).collect::<Vec<_>>());
                answer["is_path"] = json!(t.is_path);
            }
            json_result(answer)
        }
        Err(e) => fail(error_text(&e)),
    }
}

fn millis(nanos: u64) -> f64 {
    (nanos as f64 / 1_000.0).round() / 1_000.0
}

fn node_json(p: &Profile, index: usize) -> Value {
    let Some(n) = p.nodes.get(index) else { return Value::Null };
    let mut v = json!({
        "index": index,
        "text": n.text,
        "span": [n.span.start, n.span.end],
        "depth": n.depth,
        "parent": n.parent,
        "hits": n.hits,
        "items": n.items,
        "self_instructions": n.self_instructions,
        "total_instructions": n.total_instructions,
    });
    if n.is_loop {
        v["loops"] = json!(true);
        v["entries"] = json!(n.entries);
    }
    if n.selected > 0 {
        v["selected"] = json!(n.selected);
    }
    if p.timed {
        v["self_ms"] = json!(millis(n.self_nanos));
        v["total_ms"] = json!(millis(n.total_nanos));
    }
    v
}

fn xpath_profile(a: &mut Automation, args: &Value) -> Result<Vec<Content>> {
    let Asked { text, expression, options } = asked(a, args, "xpath_profile")?;
    let top = bounded(args, "top", 20, MAX_TOP)?;
    let max_instructions = match args.get("max_instructions") {
        None => Limits::default().max_instructions,
        Some(v) => v
            .as_u64()
            .filter(|n| (1..=MAX_INSTRUCTIONS).contains(n))
            .ok_or_else(|| ToolError::InvalidArgs(format!("max_instructions is a whole number from 1 to {MAX_INSTRUCTIONS}")))?,
    };
    let p = xmlcraft_query::profile(&text, &expression, &options, &Limits { max_instructions });
    let mut answer = json!({
        "instructions": p.instructions,
        "stopped": p.stopped,
        "timed": p.timed,
        "hot": p.hot.iter().take(top).map(|&i| node_json(&p, i)).collect::<Vec<_>>(),
        "tree": (0..p.nodes.len().min(MAX_TOP)).map(|i| node_json(&p, i)).collect::<Vec<_>>(),
        "nodes": p.nodes.len(),
    });
    if p.timed {
        answer["ms"] = json!(millis(p.nanos));
    }
    match &p.result {
        Ok(done) => {
            let items = items_json(done, &text);
            answer["count"] = json!(done.count);
            answer["truncated"] = json!(done.count > items.len());
            answer["items"] = json!(items);
        }
        // A run that was stopped or failed still has what was counted until then; one that did not compile has nothing.
        Err(e) if p.nodes.is_empty() => return fail(error_text(e)),
        Err(e) => answer["error"] = json!(error_text(e)),
    }
    json_result(answer)
}
