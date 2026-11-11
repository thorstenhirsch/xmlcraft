#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! ISO Schematron: rules (`.sch`) read and checked on a document with XPath 3.1 (`xmlcraft-query`).
//!
//! - **Layer:** L4, no UI and no I/O (an `include` is read through a function the caller gives), so it compiles for the web.
//! - [`Schema::parse`] reads a Schematron schema: `ns`, `let` (with `value` or with text), `phase`, `pattern`, `rule` (also abstract
//!   rules with `extends`, and abstract patterns with `is-a` and `param`, also across `include`d files), `include` (an `href` is taken
//!   from the file that names it), `assert`, `report` with `id`, `flag`, `role`, `diagnostics`, and messages with `name` and `value-of`.
//!   The query language is XPath 3.1 whatever the `queryBinding` says (XSLT 1 and 2 expressions are subsets). For the XSLT query
//!   bindings, the XSLT functions of the schema (`xsl:function` at the top level) are turned into XPath inline functions (see `xslt.rs`),
//!   and `current()` in a rule is the node the rule fired on.
//! - The variables of a pattern (`let` in a `pattern`) are visible to every pattern, as the XSLT implementations make them (global
//!   variables); those of the pattern itself come first.
//! - [`Schema::validate`] fires the rules on a document: a node is matched by the first rule of a pattern whose context it is in, an
//!   `assert` whose test is false and a `report` whose test is true are findings, each at the node that fired it (its path and its place
//!   in the text).
//! - Every check has an outcome ([`CheckState`]): not fired, passed, failed, or **not evaluated** with the reason (a context or a test
//!   that is no XPath the engine compiles, an expression that fails, a function whose body XPath cannot say, the budget ran out). A check
//!   that was not evaluated is never counted as passed or as failed. Once the context of a rule cannot be evaluated, which nodes the later
//!   rules of its pattern get is not known, so their checks are not evaluated either.

mod xpath;
mod xslt;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ops::Range;

use xmlcraft_query::{Options, Phase as QueryPhase, Prepared};
use xmlcraft_syntax::{decode_references, parse};
use xmlcraft_xsd::dom::{self, Dom, El};

use crate::xpath::{CURRENT_VAR, Callee, FUNCTIONS_VAR, rewrite_calls, string_literal, variables};
use crate::xslt::{Function, XSLT_NS};

/// How deep `include`s and abstract patterns may go.
const MAX_INCLUDE_DEPTH: usize = 8;
/// The most rules, checks and expressions a schema may hold (a schema larger than this is not read).
const MAX_ITEMS: usize = 200_000;
/// The most nodes one rule may fire on.
const MAX_NODES: usize = 100_000;
/// The most findings a report holds by default (the rest are counted).
pub const MAX_FINDINGS: usize = 10_000;
/// The most nodes of one rule that are checked one by one when a check fails on the whole of them.
const MAX_ONE_BY_ONE: usize = 1_000;

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
    /// The prefixes the schema's root element declares (`xmlns:u="utils"`): where a function call's prefix is looked up when no `ns`
    /// declares it.
    declared: Vec<(String, String)>,
    lets: Vec<Let>,
    /// Phases: id and the ids of their active patterns.
    phases: Vec<(String, Vec<String>)>,
    default_phase: Option<String>,
    patterns: Vec<Pattern>,
    diagnostics: HashMap<String, String>,
    functions: Vec<Function>,
}

/// The abstract rules and patterns as written (before `extends` and `is-a` use them). They are shared by all the files of a schema: an
/// abstract pattern is often in one included file and its instance in another.
#[derive(Default)]
struct Shared {
    abstract_rules: HashMap<String, Rule>,
    abstract_patterns: HashMap<String, Pattern>,
    count: usize,
}

struct Reader<'a> {
    text: &'a str,
    tree: Dom,
    /// The `href` of this file as the root schema names it (empty for the root schema): what its own `include`s are relative to.
    base: String,
}

fn attr<'a>(el: &'a El, name: &str) -> Option<&'a str> {
    el.attrs.iter().find(|a| a.namespace.is_none() && a.local == name).map(|a| a.value.as_str())
}

fn is_sch(el: &El, local: &str) -> bool {
    el.local == local && el.namespace.as_deref() == Some(SCHEMATRON_NS)
}

/// The Schematron namespace.
pub const SCHEMATRON_NS: &str = "http://purl.oclc.org/dsdl/schematron";

/// `href` taken from the file at `base` (both as the root schema names them): a relative reference is joined to the folder of `base`,
/// with `.` and `..` steps worked out. A reference with a scheme or an absolute path is itself.
fn relative_to(base: &str, href: &str) -> String {
    let has_scheme = href.split_once(':').is_some_and(|(scheme, _)| !scheme.is_empty() && !scheme.contains('/'));
    if has_scheme || href.starts_with('/') || href.starts_with('\\') {
        return href.to_owned();
    }
    let folder = base.rfind('/').map_or("", |at| base.get(..at + 1).unwrap_or_default());
    let joined = format!("{folder}{href}");
    let mut steps: Vec<&str> = Vec::new();
    for step in joined.split('/') {
        match step {
            "." => {}
            ".." if steps.last().is_some_and(|s| *s != "..") => {
                steps.pop();
            }
            _ => steps.push(step),
        }
    }
    steps.join("/")
}

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

    /// A `let`: its `value`, or (ISO Schematron 2016) its text as a string.
    fn read_let(&self, el: &El) -> Option<Let> {
        let name = attr(el, "name")?.trim().to_owned();
        let value = match attr(el, "value") {
            Some(v) => v.to_owned(),
            None => string_literal(&self.deep_text(el)),
        };
        Some(Let { name, value })
    }
}

impl Schema {
    /// Read a Schematron schema. `include` gives the text of an `include`d document by its `href` (none: it is not found); an `href` in
    /// an included document is taken from that document's own `href` (`ubl/rules.sch` including `../common.sch` asks for `common.sch`).
    pub fn parse(text: &str, include: &dyn Fn(&str) -> Option<String>) -> Result<Schema, SchemaError> {
        Schema::parse_located(text, "", include)
    }

    /// [`Schema::parse`] for a schema that is at `location` (a relative path, as `include` knows them): its `include`s are asked for
    /// relative to it (`en16931/ubl/main.sch` including `abstract/model.sch` asks for `en16931/ubl/abstract/model.sch`).
    pub fn parse_located(text: &str, location: &str, include: &dyn Fn(&str) -> Option<String>) -> Result<Schema, SchemaError> {
        let mut shared = Shared::default();
        Schema::parse_at(text, location, include, 0, &mut shared)
    }

    fn parse_at(text: &str, base: &str, include: &dyn Fn(&str) -> Option<String>, depth: usize, shared: &mut Shared) -> Result<Schema, SchemaError> {
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
        let declared = root.declarations.iter().filter_map(|(p, uri)| Some((p.clone()?, uri.clone()))).collect();
        let reader = Reader { text, tree, base: base.to_owned() };
        let mut schema = Schema {
            query_binding: attr(&root, "queryBinding").map(str::to_owned),
            default_phase: attr(&root, "defaultPhase").map(str::to_owned),
            declared,
            ..Schema::default()
        };
        schema.read_children(&reader, 0, include, depth, shared)?;
        Ok(schema)
    }

    fn count(shared: &mut Shared, el: &El) -> Result<(), SchemaError> {
        shared.count += 1;
        if shared.count > MAX_ITEMS {
            return fail("the schema is too large", Some(el.name_range.clone()));
        }
        Ok(())
    }

    fn read_children(
        &mut self,
        reader: &Reader<'_>,
        index: usize,
        include: &dyn Fn(&str) -> Option<String>,
        depth: usize,
        shared: &mut Shared,
    ) -> Result<(), SchemaError> {
        let Some(parent) = reader.el(index).cloned() else { return Ok(()) };
        for child_index in reader.children(&parent) {
            let Some(child) = reader.el(child_index).cloned() else { continue };
            Schema::count(shared, &child)?;
            if child.namespace.as_deref() == Some(XSLT_NS) && child.local == "function" {
                if let Some(function) = xslt::read(&reader.tree, child_index, &self.namespaces) {
                    self.functions.push(function);
                }
                continue;
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
                    if let Some(l) = reader.read_let(&child) {
                        self.lets.push(l);
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
                    // The included document's root replaces the `include`: a `pattern`, a `schema` (its children are taken), …
                    let inner = Schema::read_include(reader, &child, href, include, depth, shared)?;
                    self.merge(inner);
                }
                "pattern" => {
                    let pattern = self.read_pattern(reader, &child, include, depth, shared)?;
                    if attr(&child, "abstract") == Some("true") {
                        if let Some(id) = attr(&child, "id") {
                            shared.abstract_patterns.insert(id.to_owned(), pattern);
                        }
                    } else if let Some(instance) = attr(&child, "is-a") {
                        let params: Vec<(String, String)> = reader
                            .children(&child)
                            .into_iter()
                            .filter_map(|i| reader.el(i))
                            .filter(|e| is_sch(e, "param"))
                            .filter_map(|e| Some((attr(e, "name")?.to_owned(), attr(e, "value")?.to_owned())))
                            .collect();
                        let Some(Pattern { lets, rules, .. }) = shared.abstract_patterns.get(instance).cloned() else {
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
                        let mut own_lets: Vec<Let> = lets.into_iter().map(|l| Let { name: l.name, value: substitute(&l.value) }).collect();
                        own_lets.extend(pattern.lets);
                        let mut own_rules: Vec<Rule> = rules;
                        own_rules.extend(pattern.rules);
                        self.patterns.push(Pattern { id: pattern.id, lets: own_lets, rules: own_rules });
                    } else {
                        self.patterns.push(pattern);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn read_include(
        reader: &Reader<'_>,
        el: &El,
        href: &str,
        include: &dyn Fn(&str) -> Option<String>,
        depth: usize,
        shared: &mut Shared,
    ) -> Result<Schema, SchemaError> {
        if depth >= MAX_INCLUDE_DEPTH {
            return fail("includes are nested too deeply", Some(el.name_range.clone()));
        }
        let wanted = relative_to(&reader.base, href);
        let Some(included) = include(&wanted) else {
            return fail(format!("the included file {wanted} cannot be read"), Some(el.name_range.clone()));
        };
        Schema::parse_included(&included, &wanted, include, depth + 1, shared)
    }

    fn parse_included(
        text: &str,
        href: &str,
        include: &dyn Fn(&str) -> Option<String>,
        depth: usize,
        shared: &mut Shared,
    ) -> Result<Schema, SchemaError> {
        let parsed = parse(text);
        if parsed.diagnostics().iter().any(|d| d.severity() == xmlcraft_syntax::Severity::Error) {
            return fail(format!("the included file {href} is not well-formed XML"), None);
        }
        let tree = dom::build(&parsed.syntax());
        let Some(root) = tree.elements.first().cloned() else { return fail(format!("the included file {href} is empty"), None) };
        if is_sch(&root, "schema") {
            return Schema::parse_at(text, href, include, depth, shared);
        }
        // A pattern (or a bare group of rules) is wrapped in a schema of its own.
        let wrapped = format!("<schema xmlns=\"{SCHEMATRON_NS}\">{}</schema>", strip_declaration(text));
        Schema::parse_at(&wrapped, href, include, depth, shared)
    }

    fn merge(&mut self, other: Schema) {
        self.namespaces.extend(other.namespaces);
        for declared in other.declared {
            if !self.declared.iter().any(|(p, _)| *p == declared.0) {
                self.declared.push(declared);
            }
        }
        self.lets.extend(other.lets);
        self.phases.extend(other.phases);
        self.patterns.extend(other.patterns);
        self.diagnostics.extend(other.diagnostics);
        self.functions.extend(other.functions);
    }

    fn read_pattern(
        &mut self,
        reader: &Reader<'_>,
        el: &El,
        include: &dyn Fn(&str) -> Option<String>,
        depth: usize,
        shared: &mut Shared,
    ) -> Result<Pattern, SchemaError> {
        let mut pattern = Pattern { id: attr(el, "id").map(str::to_owned), lets: Vec::new(), rules: Vec::new() };
        for child_index in reader.children(el) {
            let Some(child) = reader.el(child_index).cloned() else { continue };
            Schema::count(shared, &child)?;
            if child.namespace.as_deref() != Some(SCHEMATRON_NS) {
                continue;
            }
            match child.local.as_str() {
                "let" => {
                    if let Some(l) = reader.read_let(&child) {
                        pattern.lets.push(l);
                    }
                }
                "include" => {
                    let Some(href) = attr(&child, "href") else { continue };
                    let inner = Schema::read_include(reader, &child, href, include, depth, shared)?;
                    for p in inner.patterns {
                        pattern.lets.extend(p.lets);
                        pattern.rules.extend(p.rules);
                    }
                    self.functions.extend(inner.functions);
                    self.namespaces.extend(inner.namespaces);
                }
                "rule" => {
                    let rule = self.read_rule(reader, &child, shared)?;
                    if attr(&child, "abstract") == Some("true") {
                        if let Some(id) = attr(&child, "id") {
                            shared.abstract_rules.insert(id.to_owned(), rule);
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

    fn read_rule(&mut self, reader: &Reader<'_>, el: &El, shared: &mut Shared) -> Result<Rule, SchemaError> {
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
            Schema::count(shared, &child)?;
            if child.namespace.as_deref() != Some(SCHEMATRON_NS) {
                continue;
            }
            match child.local.as_str() {
                "let" => {
                    if let Some(l) = reader.read_let(&child) {
                        rule.lets.push(l);
                    }
                }
                "extends" => {
                    if let Some(base) = attr(&child, "rule") {
                        let Some(base) = shared.abstract_rules.get(base) else {
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

    /// The XSLT functions of the schema (as written, `u:gln`) and, for each one XPath cannot say, why.
    pub fn functions(&self) -> Vec<(&str, Option<&str>)> {
        self.functions.iter().map(|f| (f.name.as_str(), f.body.as_ref().err().map(String::as_str))).collect()
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
        // Parameters may be written with or without the dollar sign, and with spaces around the name (`name="Seller "` is in the CEN
        // bindings).
        let found = params.iter().find(|(n, _)| n.trim().trim_start_matches('$') == name);
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

/// What came of one check (an `assert` or a `report` of a rule of an active pattern).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckState {
    /// The context of its rule matched no node (or only nodes an earlier rule of the pattern took).
    NotFired,
    /// It was evaluated on every node its rule fired on, and gave no finding.
    Passed,
    /// It gave findings (this many).
    Failed(usize),
    /// It could not be evaluated, and why. It is neither passed nor failed.
    NotEvaluated(String),
}

/// One check and what came of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckOutcome {
    pub kind: CheckKind,
    pub id: Option<String>,
    pub flag: Option<String>,
    pub test: String,
    pub context: String,
    pub pattern: Option<String>,
    pub state: CheckState,
}

/// How many checks came out which way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Totals {
    /// The checks of the active patterns.
    pub checks: usize,
    pub not_fired: usize,
    pub passed: usize,
    pub failed: usize,
    pub not_evaluated: usize,
}

/// What came of validating a document.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Report {
    pub findings: Vec<Finding>,
    /// How many findings there are (more than `findings.len()` when the list was cut at the limit).
    pub count: usize,
    /// How many rules fired on at least one node, and how many checks those rules have.
    pub fired_rules: usize,
    pub checks: usize,
    /// What could not be evaluated (a context or test that is no valid XPath, an expression that failed), in words.
    pub problems: Vec<String>,
    /// Every check of the active patterns, in order, with what came of it.
    pub outcomes: Vec<CheckOutcome>,
}

impl Report {
    pub fn error_count(&self) -> usize {
        self.findings.iter().filter(|f| f.severity == Severity::Error).count()
    }

    pub fn totals(&self) -> Totals {
        let mut t = Totals { checks: self.outcomes.len(), ..Totals::default() };
        for o in &self.outcomes {
            match o.state {
                CheckState::NotFired => t.not_fired += 1,
                CheckState::Passed => t.passed += 1,
                CheckState::Failed(_) => t.failed += 1,
                CheckState::NotEvaluated(_) => t.not_evaluated += 1,
            }
        }
        t
    }
}

/// The bounds of a validation.
pub struct Limits<'a> {
    /// The most findings listed (the rest are counted).
    pub max_findings: usize,
    /// Asked before each expression is evaluated: `true` stops the validation, and the checks not yet evaluated are reported as not
    /// evaluated (a time budget, which the caller keeps: there is no clock on the web).
    pub stop: Option<&'a dyn Fn() -> bool>,
}

impl Default for Limits<'_> {
    fn default() -> Self {
        Self { max_findings: MAX_FINDINGS, stop: None }
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
    let mut namespaces = schema.namespaces.clone();
    // A prefix only the root element declares is known too (rules written for XSLT use them).
    for (prefix, uri) in &schema.declared {
        if !namespaces.iter().any(|(p, _)| p == prefix) {
            namespaces.push((prefix.clone(), uri.clone()));
        }
    }
    Options { namespaces, max_items: MAX_NODES, max_text: 4096, max_total_text: 64 * 1024 * 1024 }
}

/// An expression of the schema with its calls rewritten, and what it refers to.
#[derive(Clone, Debug)]
struct Piece {
    text: Result<String, String>,
    vars: BTreeSet<String>,
    calls: BTreeSet<String>,
}

/// What the expressions of one validation are built from: the variables of the schema and of its patterns, and its functions, each
/// rewritten once.
struct Program<'s> {
    schema: &'s Schema,
    /// The global variables in order: the schema's, then each pattern's (pattern index), with their pieces.
    globals: Vec<(Option<usize>, &'s str, Piece)>,
    /// The first global of each name, and per pattern the pattern's own.
    by_name: HashMap<&'s str, usize>,
    by_pattern: HashMap<(usize, &'s str), usize>,
    /// The functions by key: their inline function expression, what they refer to (their own parameters and variables left out).
    functions: BTreeMap<String, Piece>,
}

impl<'s> Program<'s> {
    fn new(schema: &'s Schema) -> Self {
        let mut program = Program { schema, globals: Vec::new(), by_name: HashMap::new(), by_pattern: HashMap::new(), functions: BTreeMap::new() };
        let mut all: Vec<(Option<usize>, &Let)> = schema.lets.iter().map(|l| (None, l)).collect();
        for (i, p) in schema.patterns.iter().enumerate() {
            all.extend(p.lets.iter().map(|l| (Some(i), l)));
        }
        for (pattern, l) in all {
            let index = program.globals.len();
            let name = l.name.trim_start_matches('$');
            program.by_name.entry(name).or_insert(index);
            if let Some(p) = pattern {
                program.by_pattern.entry((p, name)).or_insert(index);
            }
            let piece = program.piece(&l.value, false);
            program.globals.push((pattern, name, piece));
        }
        for f in &schema.functions {
            let piece = match &f.body {
                Err(why) => Piece { text: Err(why.clone()), vars: BTreeSet::new(), calls: BTreeSet::new() },
                Ok(body) => {
                    let mut piece = program.piece(body, false);
                    piece.text = piece.text.map(|b| xslt::inline(f, &b));
                    for (param, _) in &f.params {
                        piece.vars.remove(param.as_str());
                    }
                    piece.vars.remove(FUNCTIONS_VAR);
                    piece
                }
            };
            // The first of two functions with one key is the one (XSLT would refuse both).
            program.functions.entry(f.key.clone()).or_insert(piece);
        }
        program
    }

    /// The namespace a prefix of a function call stands for.
    fn namespace_of(&self, prefix: &str) -> Option<&str> {
        self.schema
            .namespaces
            .iter()
            .find(|(p, _)| p == prefix)
            .or_else(|| self.schema.declared.iter().find(|(p, _)| p == prefix))
            .map(|(_, uri)| uri.as_str())
    }

    fn piece(&self, text: &str, current: bool) -> Piece {
        let mut calls = BTreeSet::new();
        let resolve = |name: &str, arity: usize| -> Callee {
            match (name, arity) {
                ("current", _) => return Callee::Current,
                ("normalize-space" | "fn:normalize-space" | "string-length" | "fn:string-length", 0) => return Callee::OfContextString,
                _ => {}
            }
            let Some((prefix, local)) = name.split_once(':') else { return Callee::Other };
            let Some(namespace) = self.namespace_of(prefix) else { return Callee::Other };
            let key = xslt::key(namespace, local, arity);
            if self.schema.functions.iter().any(|f| f.key == key) { Callee::Function(key) } else { Callee::Other }
        };
        let rewritten = rewrite_calls(text, &resolve, current, &mut calls);
        Piece { vars: variables(text), text: rewritten, calls }
    }

    fn global(&self, name: &str, pattern: Option<usize>) -> Option<usize> {
        pattern.and_then(|p| self.by_pattern.get(&(p, name)).copied()).or_else(|| self.by_name.get(name).copied())
    }

    /// `let … return ` binding what `pieces` refer to (the global variables and the functions they need, in an order where each comes
    /// after what it uses), or why that cannot be. `local` are the names the caller binds itself (a rule's variables).
    fn prefix(&self, pieces: &[&Piece], pattern: Option<usize>, local: &BTreeSet<&str>) -> Result<String, String> {
        // Which globals and functions are needed, transitively.
        let mut globals: BTreeSet<usize> = BTreeSet::new();
        let mut functions: BTreeSet<String> = BTreeSet::new();
        let mut vars: Vec<String> = Vec::new();
        let mut calls: Vec<String> = Vec::new();
        for p in pieces {
            if let Err(why) = &p.text {
                return Err(why.clone());
            }
            vars.extend(p.vars.iter().filter(|v| !local.contains(v.as_str())).cloned());
            calls.extend(p.calls.iter().cloned());
        }
        while !vars.is_empty() || !calls.is_empty() {
            if let Some(v) = vars.pop()
                && let Some(g) = self.global(&v, pattern)
                && globals.insert(g)
                && let Some((_, _, piece)) = self.globals.get(g)
            {
                vars.extend(piece.vars.iter().cloned());
                calls.extend(piece.calls.iter().cloned());
            }
            if let Some(c) = calls.pop()
                && !functions.contains(&c)
            {
                let Some(piece) = self.functions.get(&c) else { continue };
                if let Err(why) = &piece.text {
                    return Err(why.clone());
                }
                vars.extend(piece.vars.iter().cloned());
                calls.extend(piece.calls.iter().cloned());
                functions.insert(c);
            }
        }
        for g in &globals {
            if let Some((_, name, piece)) = self.globals.get(*g)
                && let Err(why) = &piece.text
            {
                return Err(format!("the variable {name}: {why}"));
            }
        }
        // The order: each global after the globals it names and after the functions when it calls one; the functions (one map) after
        // the globals any of them names.
        let fns_deps: BTreeSet<usize> = functions
            .iter()
            .filter_map(|k| self.functions.get(k))
            .flat_map(|p| p.vars.iter())
            .filter_map(|v| self.global(v, pattern))
            .filter(|g| globals.contains(g))
            .collect();
        let mut order: Vec<Option<usize>> = Vec::new(); // `None` is the map of the functions
        let mut state: HashMap<Option<usize>, bool> = HashMap::new(); // false: being visited, true: done
        let mut stack: Vec<(Option<usize>, bool)> = Vec::new();
        let mut roots: Vec<Option<usize>> = globals.iter().map(|g| Some(*g)).collect();
        if !functions.is_empty() {
            roots.push(None);
        }
        for root in roots {
            stack.push((root, false));
            while let Some((node, expanded)) = stack.pop() {
                if expanded {
                    state.insert(node, true);
                    order.push(node);
                    continue;
                }
                match state.get(&node) {
                    Some(true) => continue,
                    Some(false) => return Err("variables of the schema depend on each other in a circle".to_owned()),
                    None => {}
                }
                state.insert(node, false);
                stack.push((node, true));
                let deps: Vec<Option<usize>> = match node {
                    None => fns_deps.iter().map(|g| Some(*g)).collect(),
                    Some(g) => {
                        let Some((_, _, piece)) = self.globals.get(g) else { continue };
                        let mut deps: Vec<Option<usize>> =
                            piece.vars.iter().filter_map(|v| self.global(v, pattern)).filter(|d| *d != g).map(Some).collect();
                        if !piece.calls.is_empty() {
                            deps.push(None);
                        }
                        deps
                    }
                };
                for d in deps {
                    match state.get(&d) {
                        Some(true) => {}
                        Some(false) => return Err("variables of the schema depend on each other in a circle".to_owned()),
                        None => stack.push((d, false)),
                    }
                }
            }
        }
        let mut parts: Vec<String> = Vec::new();
        for node in order {
            match node {
                None => {
                    let entries: Vec<String> = functions
                        .iter()
                        .filter_map(|k| Some(format!("{}: {}", string_literal(k), self.functions.get(k)?.text.as_ref().ok()?)))
                        .collect();
                    parts.push(format!("${FUNCTIONS_VAR} := map{{ {} }}", entries.join(", ")));
                }
                Some(g) => {
                    if let Some((_, name, Piece { text: Ok(value), .. })) = self.globals.get(g) {
                        parts.push(format!("${name} := ({value})"));
                    }
                }
            }
        }
        Ok(if parts.is_empty() { String::new() } else { format!("let {} return ", parts.join(", ")) })
    }
}

/// How one rule is evaluated: its context and the variables of the rule, ready to be put around a test.
struct RuleScope<'p> {
    /// `(/descendant-or-self::node()/(context))`, or why the context cannot be read.
    context: Result<String, String>,
    context_piece: Piece,
    /// `, $a := (…), $b := (…)` for the rule's variables.
    lets: String,
    pieces: Vec<Piece>,
    local: BTreeSet<&'p str>,
    /// A variable of the rule that cannot be read.
    let_error: Option<String>,
}

impl Program<'_> {
    fn scope<'r>(&self, rule: &'r Rule) -> RuleScope<'r> {
        let context_piece = self.piece(&rule.context, false);
        let mut local: BTreeSet<&str> = BTreeSet::new();
        let mut lets = String::new();
        let mut pieces: Vec<Piece> = Vec::new();
        let mut let_error: Option<String> = None;
        for l in &rule.lets {
            let piece = self.piece(&l.value, true);
            match &piece.text {
                Ok(value) => lets.push_str(&format!(", ${} := ({value})", l.name.trim_start_matches('$'))),
                Err(why) => let_error = Some(format!("the variable {}: {why}", l.name)),
            }
            pieces.push(piece);
            local.insert(l.name.trim_start_matches('$'));
        }
        // A rule's context is a match pattern, as in XSLT: it matches the nodes that some node of the document (or the root) reaches with
        // it. An absolute pattern reaches the same nodes from anywhere, so it is evaluated once (the wrapper would make it quadratic).
        let context = match &context_piece.text {
            Ok(t) if xpath::is_absolute_pattern(t) => Ok(format!("({t})")),
            Ok(t) => Ok(format!("(/descendant-or-self::node()/({t}))")),
            Err(why) => Err(format!("the context cannot be read: {why}")),
        };
        RuleScope { context, context_piece, lets, pieces, local, let_error }
    }

    /// The expression that evaluates `check` on the nodes of its rule's context (`selector` picks some of them, `[position() = 3]`):
    /// one boolean per node.
    fn check_expression(&self, index: usize, scope: &RuleScope<'_>, check: &Check, selector: &str) -> Result<String, String> {
        let context = scope.context.as_ref().map_err(Clone::clone)?;
        if let Some(why) = &scope.let_error {
            return Err(why.clone());
        }
        let test = self.piece(&check.test, true);
        let test_text = test.text.as_ref().map_err(Clone::clone)?;
        let mut all: Vec<&Piece> = vec![&scope.context_piece, &test];
        all.extend(scope.pieces.iter());
        let prefix = self.prefix(&all, Some(index), &scope.local)?;
        Ok(format!("{prefix}{context}{selector}!(let ${CURRENT_VAR} := .{} return boolean({test_text}))", scope.lets))
    }
}

/// Whether one check compiles (before any document is seen).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compiled {
    pub id: Option<String>,
    pub test: String,
    pub context: String,
    pub pattern: Option<String>,
    /// `Err` with the reason when the engine cannot compile it (or it cannot be turned into XPath).
    pub outcome: Result<(), String>,
}

impl Schema {
    /// Compile every check of the active patterns of `phase` (as [`Schema::validate`] chooses them) with its context, its variables and
    /// the functions it calls, without a document: which checks the engine can evaluate at all.
    pub fn compile_checks(&self, phase: Option<&str>) -> Result<Vec<Compiled>, SchemaError> {
        let active = self.active(phase)?;
        let options = options_for(self);
        let program = Program::new(self);
        let mut out = Vec::new();
        for (index, pattern) in self.patterns.iter().enumerate() {
            if !Schema::is_active(&active, pattern) {
                continue;
            }
            for rule in &pattern.rules {
                let scope = program.scope(rule);
                for check in &rule.checks {
                    let outcome = program
                        .check_expression(index, &scope, check, "")
                        .and_then(|e| xmlcraft_query::compile(&e, &options).map_err(|e| e.to_string()));
                    out.push(Compiled {
                        id: check.id.clone(),
                        test: check.test.clone(),
                        context: rule.context.clone(),
                        pattern: pattern.id.clone(),
                        outcome,
                    });
                }
            }
        }
        Ok(out)
    }

    /// The patterns of `phase`: `None` when all are active.
    fn active(&self, phase: Option<&str>) -> Result<Option<HashSet<&str>>, SchemaError> {
        let wanted = phase.or(self.default_phase.as_deref()).filter(|p| *p != "#ALL" && *p != "#DEFAULT");
        match wanted {
            None => Ok(None),
            Some(id) => match self.phases.iter().find(|(p, _)| p == id) {
                Some((_, patterns)) => Ok(Some(patterns.iter().map(String::as_str).collect())),
                None => fail(format!("the schema has no phase {id}"), None),
            },
        }
    }

    fn is_active(active: &Option<HashSet<&str>>, pattern: &Pattern) -> bool {
        active.as_ref().is_none_or(|a| pattern.id.as_deref().is_some_and(|id| a.contains(id)))
    }
}

impl Schema {
    /// Validate `xml` with the rules of `phase` (`None` for the schema's default phase, else all patterns; `#ALL` is all).
    pub fn validate(&self, xml: &str, phase: Option<&str>) -> Result<Report, SchemaError> {
        self.validate_with(xml, phase, &Limits::default())
    }

    /// [`Schema::validate`] within `limits`.
    pub fn validate_with(&self, xml: &str, phase: Option<&str>, limits: &Limits<'_>) -> Result<Report, SchemaError> {
        let active = self.active(phase)?;
        let mut prepared = Prepared::new(xml).map_err(|e| SchemaError { message: e.to_string(), span: None })?;
        let options = options_for(self);
        let program = Program::new(self);
        let mut run = Run { prepared: &mut prepared, options: &options, program: &program, limits, report: Report::default(), stopped: false };
        for (index, pattern) in self.patterns.iter().enumerate() {
            if !Schema::is_active(&active, pattern) {
                continue;
            }
            // A node is matched by the first rule of the pattern whose context it is in.
            let mut matched: HashSet<String> = HashSet::new();
            let mut broken: Option<String> = None;
            for rule in &pattern.rules {
                match &broken {
                    Some(why) => run.not_evaluated(pattern, rule, why),
                    None => {
                        if run.fire_rule(index, pattern, rule, &mut matched).is_err() {
                            broken = Some(format!(
                                "an earlier rule of the pattern (context `{}`) could not be evaluated, so which nodes this rule gets is not known",
                                rule.context.trim()
                            ));
                        }
                    }
                }
            }
        }
        Ok(run.report)
    }
}

struct Run<'r, 's> {
    prepared: &'r mut Prepared,
    options: &'r Options,
    program: &'r Program<'s>,
    limits: &'r Limits<'r>,
    report: Report,
    stopped: bool,
}

impl Run<'_, '_> {
    fn outcome(&mut self, pattern: &Pattern, rule: &Rule, check: &Check, state: CheckState) {
        if let CheckState::NotEvaluated(why) = &state {
            let id = check.id.as_deref().map(|i| format!(" ({i})")).unwrap_or_default();
            self.report.problems.push(format!(
                "the test `{}`{id} of the rule for `{}` is not evaluated: {why}",
                check.test.trim(),
                rule.context.trim()
            ));
        }
        self.report.outcomes.push(CheckOutcome {
            kind: check.kind,
            id: check.id.clone(),
            flag: check.flag.clone(),
            test: check.test.clone(),
            context: rule.context.clone(),
            pattern: pattern.id.clone(),
            state,
        });
    }

    fn not_evaluated(&mut self, pattern: &Pattern, rule: &Rule, why: &str) {
        for check in &rule.checks {
            self.outcome(pattern, rule, check, CheckState::NotEvaluated(why.to_owned()));
        }
    }

    fn should_stop(&mut self) -> bool {
        if !self.stopped && self.limits.stop.is_some_and(|stop| stop()) {
            self.stopped = true;
        }
        self.stopped
    }

    fn evaluate(&mut self, expression: &str) -> Result<xmlcraft_query::Evaluated, xmlcraft_query::QueryError> {
        self.prepared.evaluate(expression, self.options)
    }

    /// Fire `rule` (of the pattern at `index`). An error is a context that could not be evaluated (its checks are reported).
    fn fire_rule(&mut self, index: usize, pattern: &Pattern, rule: &Rule, matched: &mut HashSet<String>) -> Result<(), String> {
        const STOPPED: &str = "the validation stopped (the time budget ran out) before this rule";
        if self.should_stop() {
            self.not_evaluated(pattern, rule, STOPPED);
            return Ok(());
        }
        let program = self.program;
        let scope = program.scope(rule);
        let context = match &scope.context {
            Ok(t) => t.clone(),
            Err(why) => {
                self.not_evaluated(pattern, rule, why);
                return Err(why.clone());
            }
        };
        let context_prefix = match program.prefix(&[&scope.context_piece], Some(index), &BTreeSet::new()) {
            Ok(p) => p,
            Err(why) => {
                let why = format!("the context cannot be evaluated: {why}");
                self.not_evaluated(pattern, rule, &why);
                return Err(why);
            }
        };
        let nodes = match self.evaluate(&format!("{context_prefix}{context}")) {
            Ok(done) => done,
            Err(e) => {
                let why = format!("the context `{}` cannot be evaluated: {e}", rule.context.trim());
                self.not_evaluated(pattern, rule, &why);
                return Err(why);
            }
        };
        if nodes.count > nodes.items.len() {
            self.report
                .problems
                .push(format!("the rule for `{}` fired on more than {MAX_NODES} nodes; only the first are checked", rule.context.trim()));
        }
        // (A context that gives something that is not a node matches nothing.)
        let fresh: Vec<usize> =
            nodes.items.iter().enumerate().filter(|(_, n)| n.path.as_ref().is_some_and(|p| !matched.contains(p))).map(|(i, _)| i).collect();
        if fresh.is_empty() {
            for check in &rule.checks {
                self.outcome(pattern, rule, check, CheckState::NotFired);
            }
            return Ok(());
        }
        for i in &fresh {
            if let Some(path) = nodes.items.get(*i).and_then(|n| n.path.clone()) {
                matched.insert(path);
            }
        }
        self.report.fired_rules += 1;
        for check in &rule.checks {
            self.report.checks += 1;
            if self.should_stop() {
                self.outcome(pattern, rule, check, CheckState::NotEvaluated(STOPPED.to_owned()));
                continue;
            }
            let state = self.run_check(index, pattern, rule, check, &scope, &nodes, &fresh);
            self.outcome(pattern, rule, check, state);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn run_check(
        &mut self,
        index: usize,
        pattern: &Pattern,
        rule: &Rule,
        check: &Check,
        scope: &RuleScope<'_>,
        nodes: &xmlcraft_query::Evaluated,
        fresh: &[usize],
    ) -> CheckState {
        let program = self.program;
        let body = |selector: &str| program.check_expression(index, scope, check, selector);
        let whole = match body("") {
            Ok(e) => e,
            Err(why) => return CheckState::NotEvaluated(why),
        };
        // Each node's outcome: true or false, or why it is not known.
        let mut truths: HashMap<usize, Result<bool, String>> = HashMap::new();
        match self.evaluate(&whole) {
            Ok(results) => {
                for i in fresh {
                    if let Some(outcome) = results.items.get(*i) {
                        truths.insert(*i, Ok(outcome.text.trim() == "true()"));
                    }
                }
            }
            Err(e) if e.phase == QueryPhase::Compile => return CheckState::NotEvaluated(e.to_string()),
            Err(_) => {
                // It fails on some node: each node is tried on its own, so that the others still have their say.
                for (n, i) in fresh.iter().enumerate() {
                    if n >= MAX_ONE_BY_ONE || self.should_stop() {
                        truths.insert(*i, Err("not tried: too many nodes failed or the time budget ran out".to_owned()));
                        continue;
                    }
                    let one = match body(&format!("[position() = {}]", i + 1)).map(|e| self.evaluate(&e).map_err(|e| e.to_string())) {
                        Ok(Ok(r)) => r.items.first().map(|o| o.text.trim() == "true()").ok_or_else(|| "no value".to_owned()),
                        Ok(Err(why)) | Err(why) => Err(why),
                    };
                    truths.insert(*i, one);
                }
            }
        }
        let mut failed = 0usize;
        let mut unknown: Option<String> = None;
        for i in fresh {
            match truths.get(i) {
                Some(Ok(truth)) => {
                    let fires = match check.kind {
                        CheckKind::Assert => !truth,
                        CheckKind::Report => *truth,
                    };
                    if fires {
                        failed += 1;
                        self.finding(index, pattern, rule, check, scope, nodes, *i);
                    }
                }
                Some(Err(why)) => {
                    unknown.get_or_insert_with(|| why.clone());
                }
                None => {
                    unknown.get_or_insert_with(|| "the result has no value for the node".to_owned());
                }
            }
        }
        match (failed, unknown) {
            (0, Some(why)) => CheckState::NotEvaluated(why),
            (0, None) => CheckState::Passed,
            (n, Some(why)) => {
                self.report
                    .problems
                    .push(format!("the test `{}` failed on some nodes and could not be evaluated on others: {why}", check.test.trim()));
                CheckState::Failed(n)
            }
            (n, None) => CheckState::Failed(n),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn finding(
        &mut self,
        index: usize,
        pattern: &Pattern,
        rule: &Rule,
        check: &Check,
        scope: &RuleScope<'_>,
        nodes: &xmlcraft_query::Evaluated,
        at: usize,
    ) {
        self.report.count += 1;
        if self.report.findings.len() >= self.limits.max_findings {
            return;
        }
        let node = nodes.items.get(at);
        let message = self.message_for(index, scope, at + 1, check);
        self.report.findings.push(Finding {
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
            diagnostics: check.diagnostics.iter().filter_map(|d| self.program.schema.diagnostics.get(d).cloned()).collect(),
        });
    }

    fn message_for(&mut self, index: usize, scope: &RuleScope<'_>, at: usize, check: &Check) -> String {
        let mut out = String::new();
        for part in &check.message {
            match part {
                Part::Text(t) => out.push_str(t),
                Part::Name(path) => {
                    let inner = path.as_deref().map_or("name(.)".to_owned(), |p| format!("name({p})"));
                    out.push_str(&self.string_of(index, scope, at, &inner));
                }
                Part::ValueOf(select) => {
                    let inner = format!("string-join(({select}) ! string(.), ' ')");
                    out.push_str(&self.string_of(index, scope, at, &inner));
                }
            }
        }
        collapse_spaces(&out).trim().to_owned()
    }

    /// The string of `inner` at the `at`-th node of the rule's context (1-based), with the variables and functions it needs.
    fn string_of(&mut self, index: usize, scope: &RuleScope<'_>, at: usize, inner: &str) -> String {
        let piece = self.program.piece(inner, true);
        let Ok(context) = &scope.context else { return String::new() };
        let mut all: Vec<&Piece> = vec![&piece, &scope.context_piece];
        all.extend(scope.pieces.iter());
        let (text, prefix) = match (piece.text.clone(), self.program.prefix(&all, Some(index), &scope.local)) {
            (Ok(text), Ok(prefix)) => (text, prefix),
            (Err(why), _) | (_, Err(why)) => {
                self.report.problems.push(format!("a message expression `{inner}` cannot be evaluated: {why}"));
                return String::new();
            }
        };
        let expression = format!("{prefix}{context}[position() = {at}]!(let ${CURRENT_VAR} := .{} return {text})", scope.lets);
        match self.evaluate(&expression) {
            Ok(done) => done.items.first().map(|i| unquote(&i.text)).unwrap_or_default(),
            Err(e) => {
                self.report.problems.push(format!("a message expression `{inner}` cannot be evaluated: {e}"));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_href_is_taken_from_the_file_that_names_it() {
        assert_eq!(relative_to("", "rules.sch"), "rules.sch");
        assert_eq!(relative_to("ubl/main.sch", "../common.sch"), "common.sch");
        assert_eq!(relative_to("a/b/main.sch", "./c/d.sch"), "a/b/c/d.sch");
        assert_eq!(relative_to("main.sch", "../up.sch"), "../up.sch", "a step above the root stays");
        assert_eq!(relative_to("a/main.sch", "/abs.sch"), "/abs.sch");
        assert_eq!(relative_to("a/main.sch", "https://example.org/x.sch"), "https://example.org/x.sch");
        assert_eq!(relative_to("a/main.sch", "c:x.sch"), "c:x.sch");
    }
}
