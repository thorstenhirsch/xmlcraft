#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! ISO Schematron: rules (`.sch`) read and checked on a document with XPath 3.1 (`xmlcraft-query`).
//!
//! - **Layer:** L4, no UI and no I/O (an `include` is read through a function the caller gives), so it compiles for the web.
//! - [`Schema::parse`] reads a Schematron schema: `ns`, `let`, `phase`, `pattern`, `rule` (also abstract rules with `extends`, and
//!   abstract patterns with `is-a` and `param`), `assert`, `report` with `id`, `flag`, `role`, and messages with `name` and `value-of`.
//!   The query language is XPath 3.1 whatever the `queryBinding` says (XSLT 1 and 2 expressions are subsets).
//! - [`Schema::validate`] fires the rules on a document: a node is matched by the first rule of a pattern whose context it is in, an
//!   `assert` whose test is false and a `report` whose test is true are findings, each at the node that fired it (its path and its place
//!   in the text).
//! - What cannot be evaluated (a rule whose context or test is no valid XPath, an expression that fails) is not a panic or a stop: it is a
//!   problem of the report, and the other rules still run.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use xmlcraft_query::{Options, Prepared};
use xmlcraft_syntax::{decode_references, parse};
use xmlcraft_xsd::dom::{self, Dom, El};

/// How deep `include`s and abstract patterns may go.
const MAX_INCLUDE_DEPTH: usize = 8;
/// The most rules, checks and expressions a schema may hold (a schema larger than this is not read).
const MAX_ITEMS: usize = 200_000;
/// The most nodes one rule may fire on.
const MAX_NODES: usize = 100_000;
/// The most findings a report holds (the rest are counted).
pub const MAX_FINDINGS: usize = 10_000;

/// A schema that cannot be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaError {
    pub message: String,
    /// Where in the schema text, when it says.
    pub span: Option<Range<usize>>,
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SchemaError {}

fn fail<T>(message: impl Into<String>, span: Option<Range<usize>>) -> Result<T, SchemaError> {
    Err(SchemaError { message: message.into(), span })
}

/// What a check is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckKind {
    /// A finding when the test is false.
    Assert,
    /// A finding when the test is true.
    Report,
}

/// How serious a finding is, by its `flag` or `role` (`fatal` and `error` and anything unknown: an error; `warning`: a warning;
/// `info` and `information`: information). A `report` with no flag or role is information.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Part {
    Text(String),
    ValueOf(String),
    Name(Option<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Check {
    kind: CheckKind,
    test: String,
    id: Option<String>,
    flag: Option<String>,
    role: Option<String>,
    message: Vec<Part>,
    diagnostics: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Let {
    name: String,
    value: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Rule {
    id: Option<String>,
    context: String,
    lets: Vec<Let>,
    checks: Vec<Check>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Pattern {
    id: Option<String>,
    lets: Vec<Let>,
    rules: Vec<Rule>,
}

/// A Schematron schema.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Schema {
    pub title: Option<String>,
    /// The `queryBinding` the schema declares (`xslt2`, `xslt3`, …): informational, the language is XPath 3.1.
    pub query_binding: Option<String>,
    namespaces: Vec<(String, String)>,
    lets: Vec<Let>,
    /// Phases: id and the ids of their active patterns.
    phases: Vec<(String, Vec<String>)>,
    default_phase: Option<String>,
    patterns: Vec<Pattern>,
    diagnostics: HashMap<String, String>,
}

/// The abstract rules as written (before they are used by `extends`).
type Rules = HashMap<String, Rule>;
/// The abstract patterns as written (before they are used by `is-a`), by id.
type AbstractPatterns = HashMap<String, Pattern>;

struct Reader<'a> {
    text: &'a str,
    tree: Dom,
}

fn attr<'a>(el: &'a El, name: &str) -> Option<&'a str> {
    el.attrs.iter().find(|a| a.namespace.is_none() && a.local == name).map(|a| a.value.as_str())
}

fn is_sch(el: &El, local: &str) -> bool {
    el.local == local && el.namespace.as_deref() == Some(SCHEMATRON_NS)
}

/// The Schematron namespace.
pub const SCHEMATRON_NS: &str = "http://purl.oclc.org/dsdl/schematron";

impl Reader<'_> {
    fn el(&self, index: usize) -> Option<&El> {
        self.tree.elements.get(index)
    }

    fn children(&self, el: &El) -> Vec<usize> {
        el.children.clone()
    }

    /// The message of a check: its text, `name` and `value-of`, in order.
    fn message(&self, el: &El) -> Vec<Part> {
        let mut parts: Vec<Part> = Vec::new();
        let source = self.text;
        // The content lies between the end of the start tag and the start of the end tag.
        let from = el.attrs.iter().map(|a| a.value_range.end).chain(std::iter::once(el.name_range.end)).max().unwrap_or(el.name_range.end);
        let content_start = source.get(from..el.range.end).and_then(|rest| rest.find('>')).map_or(el.range.end, |gt| from + gt + 1);
        let self_closed = source.get(content_start.saturating_sub(2)..content_start) == Some("/>");
        let content_end = if self_closed {
            content_start
        } else {
            source.get(content_start..el.range.end).and_then(|c| c.rfind("</")).map_or(el.range.end, |lt| content_start + lt)
        };
        let mut at = content_start;
        let push_text = |parts: &mut Vec<Part>, raw: &str| {
            let t = decode_references(raw);
            if !t.is_empty() {
                parts.push(Part::Text(t));
            }
        };
        for index in &el.children {
            let Some(child) = self.el(*index) else { continue };
            if child.range.start < at || child.range.end > content_end {
                continue;
            }
            push_text(&mut parts, source.get(at..child.range.start).unwrap_or_default());
            if is_sch(child, "name") {
                parts.push(Part::Name(attr(child, "path").map(str::to_owned)));
            } else if is_sch(child, "value-of") {
                if let Some(select) = attr(child, "select") {
                    parts.push(Part::ValueOf(select.to_owned()));
                }
            } else {
                // `emph`, `span`, `dir` and the like carry text; their text is the message's.
                parts.push(Part::Text(decode_references(self.deep_text(child).as_str())));
            }
            at = child.range.end;
        }
        push_text(&mut parts, source.get(at..content_end).unwrap_or_default());
        // White space in a message is one space (a run keeps its place, so the spaces around `name` and `value-of` stay).
        for part in &mut parts {
            if let Part::Text(t) = part {
                *t = collapse_spaces(t);
            }
        }
        parts
    }

    fn deep_text(&self, el: &El) -> String {
        self.tree.all_text.get(el.deep.clone()).unwrap_or_default().to_owned()
    }
}

impl Schema {
    /// Read a Schematron schema. `include` gives the text of an `include`d document by its `href` (none: it is not found).
    pub fn parse(text: &str, include: &dyn Fn(&str) -> Option<String>) -> Result<Schema, SchemaError> {
        Schema::parse_at(text, include, 0)
    }

    fn parse_at(text: &str, include: &dyn Fn(&str) -> Option<String>, depth: usize) -> Result<Schema, SchemaError> {
        let parsed = parse(text);
        if let Some(error) = parsed.diagnostics().iter().find(|d| d.severity() == xmlcraft_syntax::Severity::Error) {
            return fail("the schema is not well-formed XML", Some(error.span.clone()));
        }
        let tree = dom::build(&parsed.syntax());
        let Some(root) = tree.elements.first().cloned() else { return fail("the schema has no root element", None) };
        if !is_sch(&root, "schema") {
            return fail(
                "this is not a Schematron schema (the root element is not `schema` in the Schematron namespace)",
                Some(root.name_range.clone()),
            );
        }
        let reader = Reader { text, tree };
        let mut schema = Schema {
            query_binding: attr(&root, "queryBinding").map(str::to_owned),
            default_phase: attr(&root, "defaultPhase").map(str::to_owned),
            ..Schema::default()
        };
        let mut abstract_rules: Rules = HashMap::new();
        let mut abstract_patterns: AbstractPatterns = HashMap::new();
        let mut count = 0usize;
        schema.read_children(&reader, 0, include, depth, &mut abstract_rules, &mut abstract_patterns, &mut count)?;
        Ok(schema)
    }

    #[allow(clippy::too_many_arguments)]
    fn read_children(
        &mut self,
        reader: &Reader<'_>,
        index: usize,
        include: &dyn Fn(&str) -> Option<String>,
        depth: usize,
        abstract_rules: &mut Rules,
        abstract_patterns: &mut AbstractPatterns,
        count: &mut usize,
    ) -> Result<(), SchemaError> {
        let Some(parent) = reader.el(index).cloned() else { return Ok(()) };
        for child_index in reader.children(&parent) {
            let Some(child) = reader.el(child_index).cloned() else { continue };
            *count += 1;
            if *count > MAX_ITEMS {
                return fail("the schema is too large", Some(child.name_range.clone()));
            }
            if child.namespace.as_deref() != Some(SCHEMATRON_NS) {
                continue; // other vocabularies (documentation, annotations) are not Schematron's
            }
            match child.local.as_str() {
                "title" => self.title = Some(reader.deep_text(&child).split_whitespace().collect::<Vec<_>>().join(" ")),
                "ns" => {
                    if let (Some(prefix), Some(uri)) = (attr(&child, "prefix"), attr(&child, "uri")) {
                        self.namespaces.push((prefix.to_owned(), uri.to_owned()));
                    }
                }
                "let" => {
                    if let (Some(name), Some(value)) = (attr(&child, "name"), attr(&child, "value")) {
                        self.lets.push(Let { name: name.to_owned(), value: value.to_owned() });
                    }
                }
                "phase" => {
                    let id = attr(&child, "id").unwrap_or_default().to_owned();
                    let active: Vec<String> = reader
                        .children(&child)
                        .into_iter()
                        .filter_map(|i| reader.el(i))
                        .filter(|e| is_sch(e, "active"))
                        .filter_map(|e| attr(e, "pattern").map(str::to_owned))
                        .collect();
                    self.phases.push((id, active));
                }
                "diagnostics" => {
                    for i in reader.children(&child) {
                        let Some(d) = reader.el(i) else { continue };
                        if is_sch(d, "diagnostic")
                            && let Some(id) = attr(d, "id")
                        {
                            self.diagnostics.insert(id.to_owned(), reader.deep_text(d).split_whitespace().collect::<Vec<_>>().join(" "));
                        }
                    }
                }
                "include" => {
                    let Some(href) = attr(&child, "href") else { continue };
                    if depth >= MAX_INCLUDE_DEPTH {
                        return fail("includes are nested too deeply", Some(child.name_range.clone()));
                    }
                    let Some(included) = include(href) else {
                        return fail(format!("the included file {href} cannot be read"), Some(child.name_range.clone()));
                    };
                    // The included document's root replaces the `include`: a `pattern`, a `schema` (its children are taken), …
                    let inner = Schema::parse_included(&included, include, depth + 1, href)?;
                    self.merge(inner);
                }
                "pattern" => {
                    let pattern = self.read_pattern(reader, &child, include, depth, abstract_rules, count)?;
                    if attr(&child, "abstract") == Some("true") {
                        if let Some(id) = attr(&child, "id") {
                            abstract_patterns.insert(id.to_owned(), pattern.clone());
                        }
                    } else if let Some(instance) = attr(&child, "is-a") {
                        let params: Vec<(String, String)> = reader
                            .children(&child)
                            .into_iter()
                            .filter_map(|i| reader.el(i))
                            .filter(|e| is_sch(e, "param"))
                            .filter_map(|e| Some((attr(e, "name")?.to_owned(), attr(e, "value")?.to_owned())))
                            .collect();
                        let Some(Pattern { lets, rules, .. }) = abstract_patterns.get(instance).cloned() else {
                            return fail(format!("the abstract pattern {instance} is not defined before it is used"), Some(child.name_range.clone()));
                        };
                        let substitute = |text: &str| substitute_params(text, &params);
                        let rules = rules
                            .into_iter()
                            .map(|r| Rule {
                                id: r.id,
                                context: substitute(&r.context),
                                lets: r.lets.into_iter().map(|l| Let { name: l.name, value: substitute(&l.value) }).collect(),
                                checks: r
                                    .checks
                                    .into_iter()
                                    .map(|c| Check {
                                        test: substitute(&c.test),
                                        message: c
                                            .message
                                            .into_iter()
                                            .map(|p| match p {
                                                Part::Text(t) => Part::Text(substitute(&t)),
                                                Part::ValueOf(s) => Part::ValueOf(substitute(&s)),
                                                Part::Name(Some(s)) => Part::Name(Some(substitute(&s))),
                                                other => other,
                                            })
                                            .collect(),
                                        ..c
                                    })
                                    .collect(),
                            })
                            .collect();
                        self.patterns.push(Pattern {
                            id: pattern.id,
                            lets: lets.into_iter().map(|l| Let { name: l.name, value: substitute(&l.value) }).collect(),
                            rules,
                        });
                    } else {
                        self.patterns.push(pattern);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn parse_included(text: &str, include: &dyn Fn(&str) -> Option<String>, depth: usize, href: &str) -> Result<Schema, SchemaError> {
        let parsed = parse(text);
        if parsed.diagnostics().iter().any(|d| d.severity() == xmlcraft_syntax::Severity::Error) {
            return fail(format!("the included file {href} is not well-formed XML"), None);
        }
        let tree = dom::build(&parsed.syntax());
        let Some(root) = tree.elements.first().cloned() else { return fail(format!("the included file {href} is empty"), None) };
        if is_sch(&root, "schema") {
            return Schema::parse_at(text, include, depth);
        }
        // A pattern (or a bare group of rules) is wrapped in a schema of its own.
        let wrapped = format!("<schema xmlns=\"{SCHEMATRON_NS}\">{}</schema>", strip_declaration(text));
        Schema::parse_at(&wrapped, include, depth)
    }

    fn merge(&mut self, other: Schema) {
        self.namespaces.extend(other.namespaces);
        self.lets.extend(other.lets);
        self.phases.extend(other.phases);
        self.patterns.extend(other.patterns);
        self.diagnostics.extend(other.diagnostics);
    }

    #[allow(clippy::too_many_arguments)]
    fn read_pattern(
        &mut self,
        reader: &Reader<'_>,
        el: &El,
        include: &dyn Fn(&str) -> Option<String>,
        depth: usize,
        abstract_rules: &mut Rules,
        count: &mut usize,
    ) -> Result<Pattern, SchemaError> {
        let mut pattern = Pattern { id: attr(el, "id").map(str::to_owned), lets: Vec::new(), rules: Vec::new() };
        for child_index in reader.children(el) {
            let Some(child) = reader.el(child_index).cloned() else { continue };
            *count += 1;
            if *count > MAX_ITEMS {
                return fail("the schema is too large", Some(child.name_range.clone()));
            }
            if child.namespace.as_deref() != Some(SCHEMATRON_NS) {
                continue;
            }
            match child.local.as_str() {
                "let" => {
                    if let (Some(name), Some(value)) = (attr(&child, "name"), attr(&child, "value")) {
                        pattern.lets.push(Let { name: name.to_owned(), value: value.to_owned() });
                    }
                }
                "include" => {
                    let Some(href) = attr(&child, "href") else { continue };
                    if depth >= MAX_INCLUDE_DEPTH {
                        return fail("includes are nested too deeply", Some(child.name_range.clone()));
                    }
                    let Some(included) = include(href) else {
                        return fail(format!("the included file {href} cannot be read"), Some(child.name_range.clone()));
                    };
                    let inner = Schema::parse_included(&included, include, depth + 1, href)?;
                    for p in inner.patterns {
                        pattern.rules.extend(p.rules);
                    }
                }
                "rule" => {
                    let rule = self.read_rule(reader, &child, abstract_rules, count)?;
                    if attr(&child, "abstract") == Some("true") {
                        if let Some(id) = attr(&child, "id") {
                            abstract_rules.insert(id.to_owned(), rule);
                        }
                    } else {
                        pattern.rules.push(rule);
                    }
                }
                _ => {}
            }
        }
        Ok(pattern)
    }

    fn read_rule(&mut self, reader: &Reader<'_>, el: &El, abstract_rules: &Rules, count: &mut usize) -> Result<Rule, SchemaError> {
        let mut rule = Rule {
            id: attr(el, "id").map(str::to_owned),
            context: attr(el, "context").unwrap_or_default().to_owned(),
            lets: Vec::new(),
            checks: Vec::new(),
        };
        if attr(el, "abstract") != Some("true") && rule.context.trim().is_empty() {
            return fail("a rule has no `context`", Some(el.name_range.clone()));
        }
        for child_index in reader.children(el) {
            let Some(child) = reader.el(child_index).cloned() else { continue };
            *count += 1;
            if *count > MAX_ITEMS {
                return fail("the schema is too large", Some(child.name_range.clone()));
            }
            if child.namespace.as_deref() != Some(SCHEMATRON_NS) {
                continue;
            }
            match child.local.as_str() {
                "let" => {
                    if let (Some(name), Some(value)) = (attr(&child, "name"), attr(&child, "value")) {
                        rule.lets.push(Let { name: name.to_owned(), value: value.to_owned() });
                    }
                }
                "extends" => {
                    if let Some(base) = attr(&child, "rule") {
                        let Some(base) = abstract_rules.get(base) else {
                            return fail(format!("the abstract rule {base} is not defined before it is used"), Some(child.name_range.clone()));
                        };
                        rule.lets.extend(base.lets.clone());
                        rule.checks.extend(base.checks.clone());
                    }
                }
                kind @ ("assert" | "report") => {
                    let Some(test) = attr(&child, "test") else {
                        return fail(format!("an {kind} has no `test`"), Some(child.name_range.clone()));
                    };
                    rule.checks.push(Check {
                        kind: if kind == "assert" { CheckKind::Assert } else { CheckKind::Report },
                        test: test.to_owned(),
                        id: attr(&child, "id").map(str::to_owned),
                        flag: attr(&child, "flag").map(str::to_owned),
                        role: attr(&child, "role").map(str::to_owned),
                        message: reader.message(&child),
                        diagnostics: attr(&child, "diagnostics").map(|d| d.split_whitespace().map(str::to_owned).collect()).unwrap_or_default(),
                    });
                }
                _ => {}
            }
        }
        Ok(rule)
    }

    /// The ids of the phases the schema defines.
    pub fn phases(&self) -> Vec<&str> {
        self.phases.iter().map(|(id, _)| id.as_str()).collect()
    }

    /// How many rules and checks the schema has.
    pub fn size(&self) -> (usize, usize) {
        let rules: usize = self.patterns.iter().map(|p| p.rules.len()).sum();
        let checks: usize = self.patterns.iter().flat_map(|p| &p.rules).map(|r| r.checks.len()).sum();
        (rules, checks)
    }
}

/// Runs of white space as one space (the ends too: a leading or trailing run is one space).
fn collapse_spaces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

/// `$name` (whole names) replaced by the values of the parameters of an abstract pattern.
fn substitute_params(text: &str, params: &[(String, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(rest.get(..at).unwrap_or_default());
        let tail = rest.get(at + 1..).unwrap_or_default();
        let name_len = tail.find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))).unwrap_or(tail.len());
        let name = tail.get(..name_len).unwrap_or_default();
        // Parameters may be written with or without the dollar sign.
        let found = params.iter().find(|(n, _)| n.trim_start_matches('$') == name);
        match found {
            Some((_, value)) if !name.is_empty() => out.push_str(value),
            _ => {
                out.push('$');
                out.push_str(name);
            }
        }
        rest = tail.get(name_len..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

/// The text of a document without its XML declaration (so that it can go inside another element).
fn strip_declaration(text: &str) -> String {
    let t = text.trim_start();
    match t.strip_prefix("<?xml").and_then(|rest| rest.find("?>").map(|e| rest.get(e + 2..).unwrap_or_default())) {
        Some(rest) => rest.to_owned(),
        None => t.to_owned(),
    }
}

/// One finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub kind: CheckKind,
    pub severity: Severity,
    /// The message of the check, with its `name` and `value-of` filled in.
    pub message: String,
    /// The `id` of the check, when it has one.
    pub id: Option<String>,
    /// The `flag` and the `role` of the check.
    pub flag: Option<String>,
    pub role: Option<String>,
    /// The test that fired, and the context of its rule.
    pub test: String,
    pub context: String,
    /// The node that fired it: its path as `fn:path` writes it, and its place in the text.
    pub path: Option<String>,
    pub span: Option<Range<usize>>,
    /// The id of the pattern.
    pub pattern: Option<String>,
    /// The texts of the diagnostics the check refers to.
    pub diagnostics: Vec<String>,
}

/// What came of validating a document.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Report {
    pub findings: Vec<Finding>,
    /// How many findings there are (more than `findings.len()` when the list was cut at [`MAX_FINDINGS`]).
    pub count: usize,
    /// How many rules fired on at least one node, and how many tests were evaluated.
    pub fired_rules: usize,
    pub checks: usize,
    /// What could not be evaluated (a context or test that is no valid XPath, an expression that failed), in words.
    pub problems: Vec<String>,
}

impl Report {
    pub fn error_count(&self) -> usize {
        self.findings.iter().filter(|f| f.severity == Severity::Error).count()
    }
}

fn severity_of(kind: CheckKind, flag: Option<&str>, role: Option<&str>) -> Severity {
    let word = flag.or(role).map(str::to_ascii_lowercase);
    match word.as_deref() {
        Some(w) if w.contains("warn") => Severity::Warning,
        Some(w) if w.contains("info") => Severity::Info,
        Some(_) => Severity::Error,
        None => match kind {
            CheckKind::Assert => Severity::Error,
            CheckKind::Report => Severity::Info,
        },
    }
}

fn options_for(schema: &Schema) -> Options {
    Options { namespaces: schema.namespaces.clone(), max_items: MAX_NODES, max_text: 4096, max_total_text: 64 * 1024 * 1024 }
}

/// `let $a := (…), $b := (…) return ` for the lets in scope (document-level ones with the document as their context).
fn lets_prefix(groups: &[(&[Let], bool)]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (lets, from_root) in groups {
        for l in *lets {
            let name = l.name.trim_start_matches('$');
            if *from_root {
                parts.push(format!("${name} := /!({})", l.value));
            } else {
                parts.push(format!("${name} := ({})", l.value));
            }
        }
    }
    if parts.is_empty() { String::new() } else { format!("let {} return ", parts.join(", ")) }
}

impl Schema {
    /// Validate `xml` with the rules of `phase` (`None` for the schema's default phase, else all patterns; `#ALL` is all).
    pub fn validate(&self, xml: &str, phase: Option<&str>) -> Result<Report, SchemaError> {
        let mut prepared = Prepared::new(xml).map_err(|e| SchemaError { message: e.to_string(), span: None })?;
        let options = options_for(self);
        let wanted = phase.or(self.default_phase.as_deref()).filter(|p| *p != "#ALL" && *p != "#DEFAULT");
        let active: Option<HashSet<&str>> = match wanted {
            None => None,
            Some(id) => match self.phases.iter().find(|(p, _)| p == id) {
                Some((_, patterns)) => Some(patterns.iter().map(String::as_str).collect()),
                None => return fail(format!("the schema has no phase {id}"), None),
            },
        };
        let mut report = Report::default();
        for pattern in &self.patterns {
            if let Some(active) = &active
                && !pattern.id.as_deref().is_some_and(|id| active.contains(id))
            {
                continue;
            }
            // A node is matched by the first rule of the pattern whose context it is in.
            let mut matched: HashSet<String> = HashSet::new();
            for rule in &pattern.rules {
                self.fire_rule(&mut prepared, &options, pattern, rule, &mut matched, &mut report);
            }
        }
        Ok(report)
    }

    fn fire_rule(
        &self,
        prepared: &mut Prepared,
        options: &Options,
        pattern: &Pattern,
        rule: &Rule,
        matched: &mut HashSet<String>,
        report: &mut Report,
    ) {
        // A rule's context is a match pattern, as in XSLT: it matches the nodes that some ancestor (or the root) reaches with it, wherever they are.
        let context = format!("(/descendant-or-self::node()/({}))", rule.context);
        let nodes = match prepared.evaluate(&context, options) {
            Ok(done) => done,
            Err(e) => {
                report.problems.push(format!("the context `{}` cannot be evaluated: {e}", rule.context));
                return;
            }
        };
        // (A context that gives something that is not a node matches nothing.)
        let fresh: Vec<usize> = nodes
            .items
            .iter()
            .enumerate()
            .filter(|(_, n)| n.path.is_some() && n.path.as_ref().is_some_and(|p| !matched.contains(p)))
            .map(|(i, _)| i)
            .collect();
        if fresh.is_empty() {
            return;
        }
        for i in &fresh {
            if let Some(path) = nodes.items.get(*i).and_then(|n| n.path.clone()) {
                matched.insert(path);
            }
        }
        report.fired_rules += 1;
        let prefix = lets_prefix(&[(&self.lets, true), (&pattern.lets, true), (&rule.lets, false)]);
        for check in &rule.checks {
            report.checks += 1;
            let expression = format!("{context}!({prefix}boolean({}))", check.test);
            let results = match prepared.evaluate(&expression, options) {
                Ok(done) => done,
                Err(e) => {
                    report.problems.push(format!("the test `{}` of the rule for `{}` cannot be evaluated: {e}", check.test, rule.context));
                    continue;
                }
            };
            for i in &fresh {
                let Some(outcome) = results.items.get(*i) else { continue };
                let truth = outcome.text.trim() == "true()";
                let fires = match check.kind {
                    CheckKind::Assert => !truth,
                    CheckKind::Report => truth,
                };
                if !fires {
                    continue;
                }
                report.count += 1;
                if report.findings.len() >= MAX_FINDINGS {
                    continue;
                }
                let node = nodes.items.get(*i);
                let message =
                    self.message_for(prepared, options, &context, &prefix, *i, check, node.map(|n| n.path.clone()).unwrap_or_default(), report);
                report.findings.push(Finding {
                    kind: check.kind,
                    severity: severity_of(check.kind, check.flag.as_deref(), check.role.as_deref()),
                    message,
                    id: check.id.clone(),
                    flag: check.flag.clone(),
                    role: check.role.clone(),
                    test: check.test.clone(),
                    context: rule.context.clone(),
                    path: node.and_then(|n| n.path.clone()),
                    span: node.and_then(|n| n.span.clone()),
                    pattern: pattern.id.clone(),
                    diagnostics: check.diagnostics.iter().filter_map(|d| self.diagnostics.get(d).cloned()).collect(),
                });
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn message_for(
        &self,
        prepared: &mut Prepared,
        options: &Options,
        context: &str,
        prefix: &str,
        index: usize,
        check: &Check,
        _path: Option<String>,
        report: &mut Report,
    ) -> String {
        let mut out = String::new();
        let at = index + 1;
        for part in &check.message {
            match part {
                Part::Text(t) => out.push_str(t),
                Part::Name(path) => {
                    let inner = path.as_deref().map_or("name(.)".to_owned(), |p| format!("name({p})"));
                    out.push_str(&self.string_of(prepared, options, context, prefix, at, &inner, report));
                }
                Part::ValueOf(select) => {
                    let inner = format!("string-join(({select}) ! string(.), ' ')");
                    out.push_str(&self.string_of(prepared, options, context, prefix, at, &inner, report));
                }
            }
        }
        collapse_spaces(&out).trim().to_owned()
    }

    #[allow(clippy::too_many_arguments)]
    fn string_of(
        &self,
        prepared: &mut Prepared,
        options: &Options,
        context: &str,
        prefix: &str,
        at: usize,
        inner: &str,
        report: &mut Report,
    ) -> String {
        let expression = format!("{context}[position() = {at}]!({prefix}{inner})");
        match prepared.evaluate(&expression, options) {
            Ok(done) => done.items.first().map(|i| unquote(&i.text)).unwrap_or_default(),
            Err(e) => {
                report.problems.push(format!("a message expression `{inner}` cannot be evaluated: {e}"));
                String::new()
            }
        }
    }
}

/// An XPath string representation (`"a"`) as the string.
fn unquote(representation: &str) -> String {
    let t = representation.trim();
    let quote = t.chars().next().filter(|c| matches!(c, '"' | '\''));
    match quote {
        Some(q) if t.len() >= 2 && t.ends_with(q) => {
            let inner = t.get(1..t.len() - 1).unwrap_or_default();
            inner.replace(&format!("{q}{q}"), &q.to_string())
        }
        _ => t.to_owned(),
    }
}
