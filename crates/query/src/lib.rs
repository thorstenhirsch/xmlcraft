#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

//! XPath 3.1 over XML documents for XmlCraft, on the `xee` engine.
//!
//! - **Layer:** L4, no UI and no I/O, so it compiles for the web.
//! - [`evaluate`] runs an expression with the document as the context and returns the items of the result: for a node its kind, name,
//!   path (as `fn:path` writes it) and its place in the text; for a value its type and its XPath representation.
//! - The engine is XPath 3.1's. XPath 1.0, 2.0 and 3.0 are subsets of it; an expression of an older version runs under 3.1's rules
//!   (1.0's compatibility mode, where the results differ, is not implemented).
//! - Errors carry their W3C code (`XPST0003`, `FOAR0001`), say whether they came from compiling or running, and the place in the
//!   expression.

mod source_map;

use std::ops::Range;

use xee_xpath::context::StaticContextBuilder;
use xee_xpath::{Documents, Item, Queries, Query};
use xot::{Value as XotValue, Xot};

pub use source_map::SourceMap;

/// What to evaluate with.
#[derive(Clone, Debug)]
pub struct Options {
    /// Prefixes the expression may use besides the standard ones (`xs`, `fn`, `map`, `array`, `math`, `xml`): `(prefix, namespace)`.
    pub namespaces: Vec<(String, String)>,
    /// The most items returned (the rest are counted).
    pub max_items: usize,
    /// The most characters of the text of one item.
    pub max_text: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self { namespaces: Vec::new(), max_items: 10_000, max_text: 100_000 }
    }
}

/// Where an error came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The document was not read.
    Document,
    /// The expression did not compile (syntax, unknown function or prefix, type error found early).
    Compile,
    /// The expression failed while running.
    Run,
}

/// Why an evaluation failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryError {
    pub phase: Phase,
    /// The W3C error code, such as `XPST0003`; empty for a document that cannot be read.
    pub code: String,
    pub message: String,
    /// Where in the expression (a byte range), when the engine says.
    pub span: Option<Range<usize>>,
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.code.is_empty() { f.write_str(&self.message) } else { write!(f, "{}: {}", self.code, self.message) }
    }
}

impl std::error::Error for QueryError {}

/// One item of a result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultItem {
    /// `element`, `attribute`, `text`, `comment`, `processing-instruction`, `document`, `namespace`; for a value its type (`xs:string`,
    /// `xs:integer`, …); `map`, `array` or `function`.
    pub kind: String,
    /// For a node its name (`item`, `Q{urn:x}item` when it has a namespace); empty for the others.
    pub name: String,
    /// For a node its path as `fn:path` writes it (`/r[1]/i[2]/@n`, `/Q{urn:x}r[1]`).
    pub path: Option<String>,
    /// For a node, where it is in the text (when the text is well-formed enough to know).
    pub span: Option<Range<usize>>,
    /// The value: for a node its XML (an attribute `n="1"`, text its characters), for a value its XPath representation.
    pub text: String,
    /// `text` was cut at `Options::max_text` characters.
    pub cut: bool,
}

/// The result of an evaluation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evaluated {
    pub items: Vec<ResultItem>,
    /// How many items the result has (more than `items.len()` when it was cut at `Options::max_items`).
    pub count: usize,
}

fn span_of(span: Option<xee_xpath::error::SourceSpan>) -> Option<Range<usize>> {
    span.map(|s| s.range())
}

/// Evaluate `expression` with the document `xml` as the context item.
pub fn evaluate(xml: &str, expression: &str, options: &Options) -> Result<Evaluated, QueryError> {
    let mut documents = Documents::new();
    // The engine does not read a DOCTYPE: it is blanked, byte for byte, so that nothing else moves.
    let readable = without_doctype(xml);
    let document = documents.add_string_without_uri(&readable).map_err(|e| QueryError {
        phase: Phase::Document,
        code: String::new(),
        message: format!("the document cannot be read: {e}"),
        span: None,
    })?;
    let mut builder = StaticContextBuilder::default();
    for (prefix, uri) in &options.namespaces {
        builder.add_namespace(prefix, uri);
    }
    let queries = Queries::new(builder);
    let query = queries.sequence(expression).map_err(|e| QueryError {
        phase: Phase::Compile,
        code: e.error.to_string(),
        message: e.error.to_string(),
        span: span_of(e.span),
    })?;
    let sequence = query.execute(&mut documents, document).map_err(|e| QueryError {
        phase: Phase::Run,
        code: e.error.to_string(),
        message: e.error.to_string(),
        span: span_of(e.span),
    })?;
    let map = SourceMap::of(xml);
    let xot = documents.xot();
    let mut items = Vec::new();
    let mut count = 0;
    for item in sequence.iter() {
        count += 1;
        if items.len() < options.max_items {
            items.push(describe(&item, xot, &map, options, xml));
        }
    }
    Ok(Evaluated { items, count })
}

fn describe(item: &Item, xot: &Xot, map: &SourceMap, options: &Options, source: &str) -> ResultItem {
    let (kind, name, path, span, text) = match item {
        Item::Node(node) => {
            let (kind, name) = match xot.value(*node) {
                XotValue::Document => ("document", String::new()),
                XotValue::Element(e) => ("element", qualified(xot, e.name())),
                XotValue::Attribute(a) => ("attribute", qualified(xot, a.name())),
                XotValue::Text(_) => ("text", String::new()),
                XotValue::Comment(_) => ("comment", String::new()),
                XotValue::ProcessingInstruction(p) => ("processing-instruction", xot.local_name_str(p.target()).to_owned()),
                XotValue::Namespace(_) => ("namespace", String::new()),
            };
            let span = map.place(&way_of(xot, *node));
            // A node is shown as it is written in the text when its place is known (a serialization would add the namespace
            // declarations in scope, resolve references, and change the quotes).
            let written =
                span.as_ref().filter(|_| !matches!(xot.value(*node), XotValue::Text(_))).and_then(|s| source.get(s.clone())).map(str::to_owned);
            let text = written.unwrap_or_else(|| match xot.value(*node) {
                XotValue::Attribute(a) => format!("{}=\"{}\"", qualified(xot, a.name()), a.value()),
                XotValue::Text(_) | XotValue::Namespace(_) => xot.string_value(*node),
                _ => xot.to_string(*node).unwrap_or_else(|_| xot.string_value(*node)),
            });
            (kind.to_owned(), name, Some(path_of(xot, *node)), span, text)
        }
        Item::Atomic(a) => (atomic_type(a).to_owned(), String::new(), None, None, a.xpath_representation()),
        Item::Function(f) => {
            let kind = match f {
                xee_xpath::function::Function::Map(_) => "map",
                xee_xpath::function::Function::Array(_) => "array",
                _ => "function",
            };
            (kind.to_owned(), String::new(), None, None, format!("{f:?}"))
        }
    };
    let (text, cut) = cut(text, options.max_text);
    ResultItem { kind, name, path, span, text, cut }
}

/// `xml` with its DOCTYPE declaration (and the internal subset) replaced by spaces of the same length.
fn without_doctype(xml: &str) -> String {
    let parsed = xmlcraft_syntax::parse(xml);
    let root = parsed.syntax();
    let Some(doctype) = root.children().find(|n| n.kind() == xmlcraft_syntax::SyntaxKind::Doctype) else { return xml.to_owned() };
    let (start, end) = (usize::from(doctype.text_range().start()), usize::from(doctype.text_range().end()));
    match (xml.get(..start), xml.get(start..end), xml.get(end..)) {
        (Some(before), Some(declaration), Some(after)) => {
            // (Line breaks stay, so lines are where they were.)
            let blank: String = declaration.chars().map(|c| if matches!(c, '\n' | '\r') { c } else { ' ' }).collect();
            format!("{before}{blank}{after}")
        }
        _ => xml.to_owned(),
    }
}

fn cut(text: String, max: usize) -> (String, bool) {
    if text.chars().count() <= max {
        return (text, false);
    }
    (text.chars().take(max).collect(), true)
}

fn qualified(xot: &Xot, name: xot::NameId) -> String {
    let (local, namespace) = xot.name_ns_str(name);
    if namespace.is_empty() { local.to_owned() } else { format!("Q{{{namespace}}}{local}") }
}

/// What kind of step a node is among its siblings: its kind and its name (the target of a processing instruction).
fn sort_of(xot: &Xot, node: xot::Node) -> (u8, String) {
    match xot.value(node) {
        XotValue::Element(e) => (1, qualified(xot, e.name())),
        XotValue::Text(_) => (2, String::new()),
        XotValue::Comment(_) => (3, String::new()),
        XotValue::ProcessingInstruction(p) => (4, xot.local_name_str(p.target()).to_owned()),
        _ => (0, String::new()),
    }
}

/// The path of a node as `fn:path` writes it.
fn path_of(xot: &Xot, node: xot::Node) -> String {
    let mut steps: Vec<String> = Vec::new();
    let mut current = Some(node);
    while let Some(n) = current {
        let parent = xot.parent(n);
        let step = match xot.value(n) {
            XotValue::Document => break,
            XotValue::Attribute(a) => format!("@{}", qualified(xot, a.name())),
            XotValue::Namespace(_) => "namespace::*".to_owned(),
            _ => {
                let sort = sort_of(xot, n);
                let position = parent.map_or(1, |p| xot.children(p).take_while(|c| *c != n).filter(|c| sort_of(xot, *c) == sort).count() + 1);
                let label = match sort.0 {
                    1 => sort.1,
                    2 => "text()".to_owned(),
                    3 => "comment()".to_owned(),
                    _ => format!("processing-instruction({})", sort.1),
                };
                format!("{label}[{position}]")
            }
        };
        steps.push(step);
        current = parent;
    }
    steps.reverse();
    format!("/{}", steps.join("/"))
}

/// The way down to a node as [`SourceMap`] names it.
fn way_of(xot: &Xot, node: xot::Node) -> String {
    let mut steps: Vec<String> = Vec::new();
    let mut current = Some(node);
    while let Some(n) = current {
        let Some(parent) = xot.parent(n) else { break };
        let step = if let XotValue::Attribute(_) = xot.value(n) {
            format!("@{}", xot.attribute_nodes(parent).position(|a| a == n).unwrap_or(0))
        } else {
            xot.children(parent).take_while(|c| *c != n).count().to_string()
        };
        steps.push(step);
        current = Some(parent);
    }
    steps.reverse();
    steps.join("/")
}

fn atomic_type(a: &xee_xpath::Atomic) -> &'static str {
    use xee_xpath::Atomic as A;
    match a {
        A::Untyped(_) => "xs:untypedAtomic",
        A::String(..) => "xs:string",
        A::Float(_) => "xs:float",
        A::Double(_) => "xs:double",
        A::Decimal(_) => "xs:decimal",
        A::Integer(..) => "xs:integer",
        A::Duration(_) => "xs:duration",
        A::YearMonthDuration(_) => "xs:yearMonthDuration",
        A::DayTimeDuration(_) => "xs:dayTimeDuration",
        A::DateTime(_) => "xs:dateTime",
        A::DateTimeStamp(_) => "xs:dateTimeStamp",
        A::Time(_) => "xs:time",
        A::Date(_) => "xs:date",
        A::GYearMonth(_) => "xs:gYearMonth",
        A::GYear(_) => "xs:gYear",
        A::GMonthDay(_) => "xs:gMonthDay",
        A::GMonth(_) => "xs:gMonth",
        A::GDay(_) => "xs:gDay",
        A::Boolean(_) => "xs:boolean",
        A::Binary(..) => "xs:base64Binary",
        A::QName(_) => "xs:QName",
    }
}
