#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! XQuery 3.1 for XmlCraft, on the `xee` engine.
//!
//! - **Layer:** L4, no UI and no I/O, so it compiles for the web.
//! - `xee` implements XPath 3.1 (its data model, its function library, its bytecode interpreter) but has no XQuery. This crate
//!   reads XQuery itself ([`parse`]: a hand-written parser for the whole XQuery 3.1 grammar except Full-Text and Update) and
//!   compiles the tree to `xee`'s intermediate representation ([`compile`]), so XQuery runs on the same machinery as XmlCraft's
//!   XPath: the same functions, the same results, the same profiler and debugger hooks. `docs/architecture.md` ("Query engines")
//!   records the decision and what the vendored interpreter carries for it.
//! - [`evaluate`] runs a query with an optional context document and returns the items (as [`xmlcraft_query::ResultItem`]s:
//!   nodes of the document with their path and place, nodes the query built shown serialized) and, when asked, the result
//!   serialized as XML, text, JSON or the adaptive method.
//! - Errors carry their W3C code (`XPST0003`, `XPTY0004`, `XQDY0025`, …), say whether they came from compiling or running, and
//!   the place in the query.
//! - Not supported (refused with their codes, never silently ignored): schema import and `validate` (XQST0009, XQST0075: the
//!   engine is not schema-aware), module import (XQST0059), external functions, collations other than the codepoint
//!   collation (XQST0038), Update and Full-Text (not parsed).

pub mod ast;
mod compile;
pub mod highlight;
pub mod parse;
mod prelude;

use std::sync::OnceLock;

use xee_interpreter::context::StaticContextBuilder;
use xee_interpreter::interpreter::hook::{At, Hook};
use xee_interpreter::sequence::{QNameOrString, SerializationParameters};
use xee_xpath::Documents;
pub use xmlcraft_query::{Evaluated, ParamValue, Phase, QueryError, ResultItem};

pub use ast::Module;
pub use compile::CompileError;
pub use parse::ParseError;

/// The namespace of the errors XmlCraft's limits raise (`try`/`catch` does not catch them).
pub const LIMIT_NAMESPACE: &str = "http://xmlcraft.org/ns/limit";

/// How a result is serialized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Serialization {
    /// The XML output method (nodes as markup, atomic values as text, separated by spaces).
    Xml,
    /// The string values of the items, separated by spaces.
    Text,
    /// The JSON output method (maps, arrays, numbers, strings, booleans; nodes as XML strings).
    Json,
    /// The adaptive method: each item as XPath writes it, one per line.
    Adaptive,
}

impl Serialization {
    pub fn parse(s: &str) -> Option<Serialization> {
        match s {
            "xml" => Some(Serialization::Xml),
            "text" => Some(Serialization::Text),
            "json" => Some(Serialization::Json),
            "adaptive" => Some(Serialization::Adaptive),
            _ => None,
        }
    }
}

/// A value for an external variable (`declare variable $name external;`).
#[derive(Clone, Debug, PartialEq)]
pub struct Variable {
    /// The variable's name as the query writes it, without `$` (`limit`, or `Q{urn:x}limit`).
    pub name: String,
    pub value: ParamValue,
}

/// What to evaluate with.
#[derive(Clone, Debug)]
pub struct Options {
    /// Prefixes the query may use besides the predeclared ones: `(prefix, namespace)`.
    pub namespaces: Vec<(String, String)>,
    /// Values for external variables.
    pub variables: Vec<Variable>,
    /// Documents `fn:doc` can open, by URI: `(uri, xml)`. Nothing else is read (no files, no network).
    pub documents: Vec<(String, String)>,
    /// How the result is serialized, if at all.
    pub serialize: Option<Serialization>,
    /// The most interpreter steps a run may take before it is stopped (`XMLC0001`).
    pub max_steps: u64,
    /// The most items listed, characters of one item, characters in all (as [`xmlcraft_query::Options`]).
    pub max_items: usize,
    pub max_text: usize,
    pub max_total_text: usize,
    /// The most bytes of serialized output.
    pub max_output: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            namespaces: Vec::new(),
            variables: Vec::new(),
            documents: Vec::new(),
            serialize: None,
            max_steps: 50_000_000,
            max_items: 10_000,
            max_text: 100_000,
            max_total_text: 4_000_000,
            max_output: 16 * 1024 * 1024,
        }
    }
}

/// The result of a query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub result: Evaluated,
    /// The result serialized as [`Options::serialize`] asked (cut at [`Options::max_output`] bytes).
    pub serialized: Option<String>,
}

fn compile_error(code: &str, message: String, span: Option<std::ops::Range<usize>>) -> QueryError {
    QueryError { phase: Phase::Compile, code: code.to_owned(), message, span }
}

fn from_parse(e: ParseError) -> QueryError {
    compile_error(e.code, e.message, Some(e.span))
}

fn from_compile(e: CompileError) -> QueryError {
    compile_error(&e.code, e.message, e.span)
}

fn run_error(e: &xee_interpreter::error::SpannedError) -> QueryError {
    let code = e.error.code();
    let message = e.error.message();
    let span = e.span.map(|s| s.range()).filter(|r| r.end > 0);
    QueryError { phase: Phase::Run, message: if message.is_empty() { code.clone() } else { message.to_owned() }, code, span }
}

/// Read a query: its syntax tree, or the first static error the parser finds.
pub fn parse(query: &str, options: &Options) -> Result<Module, QueryError> {
    parse::parse(query, &options.namespaces).map_err(from_parse)
}

fn prelude() -> Result<&'static Module, QueryError> {
    static PRELUDE: OnceLock<Result<Module, ParseError>> = OnceLock::new();
    PRELUDE
        .get_or_init(|| parse::parse(prelude::PRELUDE, &[]))
        .as_ref()
        .map_err(|e| compile_error("XMLC0002", format!("the XQuery prelude does not parse: {}", e.message), None))
}

fn variable_name(v: &Variable, namespaces: &[(String, String)]) -> Result<ast::Name, QueryError> {
    let n = v.name.trim().trim_start_matches('$');
    if let Some(rest) = n.strip_prefix("Q{")
        && let Some((uri, local)) = rest.split_once('}')
    {
        return Ok(ast::Name::new(local.to_owned(), uri.to_owned(), String::new()));
    }
    if let Some((prefix, local)) = n.split_once(':') {
        let uri = namespaces
            .iter()
            .find(|(p, _)| p == prefix)
            .map(|(_, u)| u.clone())
            .or_else(|| parse::PREDECLARED.iter().find(|(p, _)| *p == prefix).map(|(_, u)| (*u).to_owned()))
            .ok_or_else(|| compile_error("XPST0081", format!("the prefix '{prefix}' of ${n} is not declared"), None))?;
        return Ok(ast::Name::new(local.to_owned(), uri, prefix.to_owned()));
    }
    Ok(ast::Name::new(n.to_owned(), String::new(), String::new()))
}

/// The prefixes the module declares in its prolog, with the caller's.
fn module_namespaces(module: &Module, options: &Options) -> Vec<(String, String)> {
    let mut out = options.namespaces.clone();
    for decl in &module.prolog {
        if let ast::Decl::Namespace { prefix, uri } = &decl.value {
            out.push((prefix.clone(), uri.clone()));
        }
    }
    if let Some((p, u)) = &module.library {
        out.push((p.clone(), u.clone()));
    }
    out
}

/// A compiled query.
struct Compiled {
    program: xee_interpreter::interpreter::Program,
    supplied: Vec<(ast::Name, ParamValue)>,
}

fn compile_query(query: &str, has_context: bool, options: &Options) -> Result<Compiled, QueryError> {
    let module = parse(query, options)?;
    if module.library.is_some() {
        return Err(compile_error("XPST0003", "a library module has no query body to run".into(), Some(0..0)));
    }
    let prelude = prelude()?;
    let namespaces = module_namespaces(&module, options);
    // external variables declared by the query and given by the caller
    let mut supplied: Vec<(ast::Name, ParamValue)> = Vec::new();
    for v in &options.variables {
        let name = variable_name(v, &namespaces)?;
        let declared = module.prolog.iter().any(|d| matches!(&d.value, ast::Decl::Variable { name: n, external: true, .. } if *n == name));
        if declared && !supplied.iter().any(|(n, _)| *n == name) {
            supplied.push((name, v.value.clone()));
        }
    }
    let names: Vec<ast::Name> = supplied.iter().map(|(n, _)| n.clone()).collect();
    let mut builder = StaticContextBuilder::default();
    for (p, u) in &namespaces {
        builder.add_namespace(p, u);
    }
    for decl in &module.prolog {
        if let ast::Decl::DefaultElementNamespace(uri) = &decl.value {
            builder.default_element_namespace(uri);
        }
    }
    builder.variable_names(names.clone());
    let sc = builder.build();
    // The engine passes the variables in the order of its set of names: the compiler takes them in that order too.
    let ordered: Vec<ast::Name> = sc.variable_names().iter().cloned().collect();
    let setup = compile::Setup { supplied: &ordered, has_context, namespaces: &namespaces };
    let ir = compile::Compiler::new(&sc).module(&module, prelude, &setup).map_err(from_compile)?;
    let program = xee_ir::compile_xpath(ir, sc).map_err(|e| {
        let mut q = run_error(&e);
        q.phase = Phase::Compile;
        q
    })?;
    Ok(Compiled { program, supplied })
}

/// Compile `query` without running it: whether it reads and compiles (its syntax, its prolog, its functions and variables).
pub fn check(query: &str, options: &Options) -> Result<(), QueryError> {
    compile_query(query, true, options).map(|_| ())
}

/// Stops a run after a number of interpreter steps.
struct Budget {
    left: u64,
}

impl Hook for Budget {
    fn before(&mut self, _at: &At<'_, '_>) -> xee_interpreter::error::Result<()> {
        if self.left == 0 {
            let name = xot::xmlname::OwnedName::new("XMLC0001".to_owned(), LIMIT_NAMESPACE.to_owned(), String::new());
            return Err(xee_interpreter::error::Error::Application(Box::new(xee_interpreter::error::ApplicationError::new(
                name,
                "the query took too many steps and was stopped".to_owned(),
            ))));
        }
        self.left -= 1;
        Ok(())
    }
}

fn param_sequence(value: &ParamValue) -> xee_interpreter::sequence::Sequence {
    use xee_interpreter::atomic::Atomic;
    let atomic: Atomic = match value {
        ParamValue::String(s) => s.clone().into(),
        ParamValue::Boolean(b) => (*b).into(),
        ParamValue::Number(t) => {
            let t = t.trim();
            if let Ok(i) = t.parse::<ibig::IBig>() { i.into() } else { t.parse::<f64>().unwrap_or(f64::NAN).into() }
        }
    };
    atomic.into()
}

/// Evaluate `query`, with the document `context` (XML text) as the context item when there is one.
pub fn evaluate(query: &str, context: Option<&str>, options: &Options) -> Result<Outcome, QueryError> {
    let compiled = compile_query(query, context.is_some(), options)?;
    let (mut documents, context_root) = documents_of(context, options)?;
    let mut builder = compiled.program.dynamic_context_builder();
    builder.documents(documents.documents().clone());
    if let Some(root) = context_root {
        builder.context_node(root);
    }
    let mut variables = xee_interpreter::context::Variables::default();
    for (name, value) in &compiled.supplied {
        variables.insert(name.clone(), param_sequence(value));
    }
    builder.variables(variables);
    let dynamic = builder.build();
    let runnable = compiled.program.runnable(&dynamic);
    let mut budget = Budget { left: options.max_steps };
    let sequence = runnable.many_with_hook(documents.xot_mut(), &mut budget).map_err(|e| run_error(&e))?;
    // Constructed elements get prefixes for namespaces nothing declared, so that they serialize.
    let xot = documents.xot_mut();
    for item in sequence.iter() {
        if let xee_interpreter::sequence::Item::Node(node) = item
            && Some(xot.root(node)) != context_root
        {
            fix_prefixes(xot, node);
        }
    }
    let serialized = match options.serialize {
        None => None,
        Some(method) => Some(serialize(&sequence, method, documents.xot_mut(), &dynamic, options.max_output).map_err(|e| QueryError {
            phase: Phase::Run,
            code: e.code(),
            message: e.message().to_owned(),
            span: None,
        })?),
    };
    let query_options = xmlcraft_query::Options {
        namespaces: options.namespaces.clone(),
        max_items: options.max_items,
        max_text: options.max_text,
        max_total_text: options.max_total_text,
    };
    let source_map;
    let readable;
    let document = match (context, context_root) {
        (Some(xml), Some(root)) => {
            source_map = xmlcraft_query::SourceMap::of(xml);
            readable = xml;
            Some((&source_map, readable, root))
        }
        _ => None,
    };
    let result = xmlcraft_query::describe_items(&sequence, documents.xot(), document, &query_options);
    Ok(Outcome { result, serialized })
}

fn fix_prefixes(xot: &mut xot::Xot, node: xot::Node) {
    let element = if xot.is_document(node) {
        let elements: Vec<xot::Node> = xot.children(node).filter(|c| xot.is_element(*c)).collect();
        match elements.as_slice() {
            [one] => Some(*one),
            _ => None,
        }
    } else if xot.is_element(node) {
        Some(node)
    } else {
        None
    };
    if let Some(e) = element {
        // (Fails only for a node that is not an element, which `element` is not.)
        let _ = xot.create_missing_prefixes(e);
        // A namespace declared again below where it is already in scope (each constructor declares the prefix of its name).
        xot.deduplicate_namespaces(e);
    }
}

fn serialize(
    sequence: &xee_interpreter::sequence::Sequence,
    method: Serialization,
    xot: &mut xot::Xot,
    dynamic: &xee_interpreter::context::DynamicContext,
    max: usize,
) -> Result<String, xee_interpreter::error::Error> {
    let text = match method {
        Serialization::Xml | Serialization::Json => {
            let mut params = SerializationParameters::new();
            params.omit_xml_declaration = true;
            if method == Serialization::Json {
                params.method = QNameOrString::String("json".to_owned());
            }
            sequence.serialize(params, xot)?
        }
        Serialization::Text => {
            let mut parts = Vec::new();
            for item in sequence.iter() {
                if let xee_interpreter::sequence::Item::Function(_) = item {
                    return Err(xee_interpreter::error::Error::SENR0001);
                }
                parts.push(item.string_value(xot)?);
            }
            parts.join(" ")
        }
        Serialization::Adaptive => {
            let mut lines = Vec::new();
            for item in sequence.iter() {
                let line = match &item {
                    xee_interpreter::sequence::Item::Node(n) => xot.to_string(*n).unwrap_or_else(|_| xot.string_value(*n)),
                    _ => item.display_representation(xot, dynamic).unwrap_or_default(),
                };
                lines.push(line);
            }
            lines.join("\n")
        }
    };
    Ok(cut_bytes(text, max))
}

fn cut_bytes(mut s: String, max: usize) -> String {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    s
}

/// The documents of a run: the context document (and its document node) and the documents `fn:doc` may open.
fn documents_of(context: Option<&str>, options: &Options) -> Result<(Documents, Option<xot::Node>), QueryError> {
    let mut documents = Documents::new();
    let mut context_root = None;
    if let Some(xml) = context {
        let readable = xmlcraft_query::readable_document(xml);
        let handle = documents.add_string_without_uri(&readable).map_err(|e| QueryError {
            phase: Phase::Document,
            code: String::new(),
            message: format!("the document cannot be read: {e}"),
            span: None,
        })?;
        context_root = documents.document_node(handle);
    }
    for (uri, xml) in &options.documents {
        let Ok(iri) = <&iri_string::types::IriStr>::try_from(uri.as_str()) else {
            return Err(QueryError { phase: Phase::Document, code: "FODC0005".into(), message: format!("'{uri}' is not a URI"), span: None });
        };
        documents.add_string(iri, &xmlcraft_query::readable_document(xml)).map_err(|e| QueryError {
            phase: Phase::Document,
            code: "FODC0002".into(),
            message: format!("the document {uri} cannot be read: {e}"),
            span: None,
        })?;
    }
    Ok((documents, context_root))
}

/// Run a compiled query over `documents`; `result` gives the value of one of its external variables as a sequence (a check of
/// another query's result).
fn run_compiled(
    compiled: &Compiled,
    documents: &mut Documents,
    root: Option<xot::Node>,
    result: Option<(&ast::Name, &xee_interpreter::sequence::Sequence)>,
    max_steps: u64,
) -> Result<xee_interpreter::sequence::Sequence, QueryError> {
    let mut builder = compiled.program.dynamic_context_builder();
    builder.documents(documents.documents().clone());
    if let Some(root) = root {
        builder.context_node(root);
    }
    let mut variables = xee_interpreter::context::Variables::default();
    for (name, value) in &compiled.supplied {
        let value = match result {
            Some((n, s)) if n == name => s.clone(),
            _ => param_sequence(value),
        };
        variables.insert(name.clone(), value);
    }
    builder.variables(variables);
    let dynamic = builder.build();
    let runnable = compiled.program.runnable(&dynamic);
    let mut budget = Budget { left: max_steps };
    runnable.many_with_hook(documents.xot_mut(), &mut budget).map_err(|e| run_error(&e))
}

/// How a result is checked (for test suites such as the W3C's QT3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    /// An expression over `$result` (the result of the query), true when it holds.
    Holds(String),
    /// The result serialized as XML is this XML (both read and written again, so that quoting and empty-element tags agree).
    SerializesAs(String),
}

/// XML read and written again by the same writer, for comparing (`None` when it does not read).
fn canonical_xml(xot: &mut xot::Xot, xml: &str) -> Option<String> {
    let wrapped = format!("<xmlcraft-check>{xml}</xmlcraft-check>");
    let doc = xot.parse(&wrapped).ok()?;
    let text = xot.to_string(doc).ok();
    let _ = xot.remove(doc);
    text
}

/// Run `query`, then judge its result by each of `checks`. The outer error is the query's own; each check answers whether it
/// holds, or the error checking it raised.
pub fn evaluate_and_check(
    query: &str,
    context: Option<&str>,
    options: &Options,
    checks: &[Check],
) -> Result<Vec<Result<bool, QueryError>>, QueryError> {
    let compiled = compile_query(query, context.is_some(), options)?;
    let (mut documents, root) = documents_of(context, options)?;
    let sequence = run_compiled(&compiled, &mut documents, root, None, options.max_steps)?;
    {
        let xot = documents.xot_mut();
        for item in sequence.iter() {
            if let xee_interpreter::sequence::Item::Node(node) = item
                && Some(xot.root(node)) != root
            {
                fix_prefixes(xot, node);
            }
        }
    }
    let result_name = ast::Name::new("result".to_owned(), String::new(), String::new());
    let check_options = Options {
        variables: vec![Variable { name: "result".to_owned(), value: ParamValue::String(String::new()) }],
        documents: Vec::new(),
        ..options.clone()
    };
    let mut answers = Vec::new();
    for check in checks {
        let answer = match check {
            Check::Holds(expression) => {
                let q = format!("declare variable $result external;\nboolean(({expression}\n))");
                compile_query(&q, false, &check_options).and_then(|c| {
                    let s = run_compiled(&c, &mut documents, None, Some((&result_name, &sequence)), options.max_steps)?;
                    let xot = documents.xot();
                    Ok(s.iter().next().is_some_and(|i| i.string_value(xot).is_ok_and(|v| v == "true")))
                })
            }
            Check::SerializesAs(expected) => {
                let mut params = SerializationParameters::new();
                params.omit_xml_declaration = true;
                let xot = documents.xot_mut();
                match sequence.serialize(params, xot) {
                    Ok(text) => Ok(canonical_xml(xot, &text).is_some_and(|a| Some(a) == canonical_xml(xot, expected))),
                    Err(e) => Err(QueryError { phase: Phase::Run, code: e.code(), message: e.message().to_owned(), span: None }),
                }
            }
        };
        answers.push(answer);
    }
    Ok(answers)
}
