//! XSLT: a stylesheet applied to a document, on the `xee` engine's XSLT compiler.
//!
//! The engine implements part of XSLT 3.0 (templates and `apply-templates`, `for-each`, `value-of`, literal result elements, …) and
//! reports what it does not implement as an error, so a stylesheet that uses more than it has fails with that said and nothing is
//! half-run. The result is the sequence of items the transformation makes, written as text: nodes as XML, text and values as their
//! string.

use xee_interpreter::context::StaticContext;
use xee_interpreter::sequence::Item;
use xee_name::{FN_NAMESPACE, Namespaces};
use xot::Xot;

const XSLT_NAMESPACE: &str = "http://www.w3.org/1999/XSL/Transform";

/// The most parameters that are passed.
pub const MAX_PARAMS: usize = 256;

/// The value of a global parameter, with the type it has in the stylesheet.
#[derive(Clone, Debug, PartialEq)]
pub enum ParamValue {
    /// `xs:string`.
    String(String),
    /// A number: `xs:integer` when the text is a whole number, else `xs:double`.
    Number(String),
    /// `xs:boolean`.
    Boolean(bool),
}

/// A value for the global `xsl:param` called `name`.
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    pub value: ParamValue,
}

/// A top-level `xsl:param` of a stylesheet and its `select` default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclaredParam {
    pub name: String,
    pub select: Option<String>,
}

fn is_xslt(xot: &Xot, node: xot::Node, local: &str) -> bool {
    xot.element(node).is_some_and(|e| xot.name_ns_str(e.name()) == (local, XSLT_NAMESPACE))
}

fn attribute(xot: &Xot, node: xot::Node, name: &str) -> Option<String> {
    let id = xot.name(name)?;
    xot.attributes(node).get(id).cloned()
}

/// The top-level `xsl:param`s of `xslt`, in order (none when it cannot be read).
pub fn declared_params(xslt: &str) -> Vec<DeclaredParam> {
    let mut xot = Xot::new();
    let Ok(root) = xot.parse(xslt) else { return Vec::new() };
    let Ok(top) = xot.document_element(root) else { return Vec::new() };
    xot.children(top)
        .filter(|c| is_xslt(&xot, *c, "param"))
        .filter_map(|c| Some(DeclaredParam { name: attribute(&xot, c, "name")?, select: attribute(&xot, c, "select") }))
        .take(MAX_PARAMS)
        .collect()
}

/// A literal the default of a parameter is written as (`'text'`, `12`, `true()`), as a value; none for an expression.
pub fn literal_default(select: &str) -> Option<ParamValue> {
    let t = select.trim();
    match t {
        "true()" => return Some(ParamValue::Boolean(true)),
        "false()" => return Some(ParamValue::Boolean(false)),
        _ => {}
    }
    for quote in ['\'', '"'] {
        if let Some(inner) = t.strip_prefix(quote).and_then(|r| r.strip_suffix(quote))
            && !inner.contains(quote)
        {
            return Some(ParamValue::String(inner.to_owned()));
        }
    }
    (t.parse::<f64>().is_ok() && t.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'-' | b'.' | b'e' | b'E')))
        .then(|| ParamValue::Number(t.to_owned()))
}

/// What a transformation made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transformed {
    /// The result, written out.
    pub text: String,
    /// How many items the result sequence had.
    pub items: usize,
}

/// Why a transformation did not run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransformError {
    pub message: String,
}

impl std::fmt::Display for TransformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TransformError {}

fn fail<T>(message: impl Into<String>) -> Result<T, TransformError> {
    Err(TransformError { message: message.into() })
}

/// The most text a transformation returns.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024 * 1024;

/// Apply the stylesheet `xslt` to the document `xml`.
pub fn transform(xml: &str, xslt: &str) -> Result<Transformed, TransformError> {
    transform_with(xml, xslt, &[])
}

/// [`transform`] with values for the global parameters of the stylesheet (see `with_parameters`: the values are written into the
/// expressions of a copy of the stylesheet, because the engine implements no `xsl:param`).
pub fn transform_with(xml: &str, xslt: &str, params: &[Param]) -> Result<Transformed, TransformError> {
    if params.len() > MAX_PARAMS {
        return fail(format!("at most {MAX_PARAMS} parameters are passed"));
    }
    // (The engine is early: it can panic on input it does not expect, so a panic is an error here, not a crash.)
    match std::panic::catch_unwind(|| run(xml, xslt, params)) {
        Ok(done) => done,
        Err(_) => fail("the XSLT engine stopped on this input (it does not implement everything of XSLT); nothing was produced"),
    }
}

/// The XPath attributes of an XSLT element in which `$name` is replaced; every other attribute is a value template (`{$name}` only).
const XPATH_ATTRIBUTES: &[&str] = &["select", "test", "use", "group-by", "group-adjacent", "count", "from", "value"];

/// `expression` with each reference to a parameter replaced by its value written as an XPath literal in parentheses. Text inside string
/// literals and comments is left alone, and so is a reference to a prefixed name.
fn substitute(expression: &str, literals: &[(String, String)]) -> String {
    let mut out = String::with_capacity(expression.len());
    let mut chars = expression.char_indices().peekable();
    let mut quote: Option<char> = None;
    while let Some((at, c)) = chars.next() {
        match quote {
            Some(q) => {
                out.push(c);
                if c == q {
                    quote = None;
                }
                continue;
            }
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                out.push(c);
                continue;
            }
            None => {}
        }
        if c != '$' {
            out.push(c);
            continue;
        }
        let rest = expression.get(at + 1..).unwrap_or_default();
        let len = rest.find(|ch: char| !(ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.'))).unwrap_or(rest.len());
        let name = rest.get(..len).unwrap_or_default();
        let prefixed = rest.get(len..).is_some_and(|after| after.starts_with(':') && !after.starts_with("::"));
        match literals.iter().find(|(n, _)| n == name) {
            Some((_, literal)) if !name.is_empty() && !prefixed => {
                out.push('(');
                out.push_str(literal);
                out.push(')');
                for _ in 0..name.chars().count() {
                    chars.next();
                }
            }
            _ => out.push('$'),
        }
    }
    out
}

/// A value template with the references inside its `{…}` replaced (`{{` and `}}` are braces of the text).
fn substitute_template(template: &str, literals: &[(String, String)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(rest.get(..open).unwrap_or_default());
        let after = rest.get(open..).unwrap_or_default();
        if after.starts_with("{{") {
            out.push_str("{{");
            rest = after.get(2..).unwrap_or_default();
            continue;
        }
        match after.find('}') {
            Some(close) => {
                out.push('{');
                out.push_str(&substitute(after.get(1..close).unwrap_or_default(), literals));
                out.push('}');
                rest = after.get(close + 1..).unwrap_or_default();
            }
            None => {
                out.push_str(after);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// A value as an XPath literal.
fn literal_of(value: &ParamValue) -> Result<String, TransformError> {
    Ok(match value {
        ParamValue::String(s) => format!("'{}'", s.replace('\'', "''")),
        ParamValue::Boolean(true) => "true()".to_owned(),
        ParamValue::Boolean(false) => "false()".to_owned(),
        ParamValue::Number(text) => {
            let t = text.trim();
            if t.parse::<i64>().is_ok() {
                t.to_owned()
            } else if t.parse::<f64>().is_ok_and(f64::is_finite) {
                format!("number('{t}')")
            } else {
                return fail(format!("{t:?} is not a number"));
            }
        }
    })
}

/// The stylesheet as it is run: without its top-level `xsl:param`s, and with each `$name` that refers to one replaced by its value.
///
/// The engine's compiler does not implement `xsl:param` or `xsl:variable` declarations (it stops at them), and it does not take variable
/// names from outside, so a global parameter is passed by writing its value into the expressions that use it, in a copy of the
/// stylesheet that only the engine sees. The value is the one given, else the default the parameter declares when that is a literal
/// (`'text'`, `12`, `true()`), else the empty string when it declares none; a default that is an expression is an error that says to
/// give a value (it is not evaluated), and so is a parameter the stylesheet does not declare.
fn with_parameters(xslt: &str, params: &[Param]) -> Result<String, TransformError> {
    let mut xot = Xot::new();
    // (A stylesheet that cannot be read is left for the compiler to say so; a parameter has nowhere to go in it.)
    let Ok(root) = xot.parse(xslt) else {
        return match params.first() {
            Some(first) => fail(format!("the stylesheet cannot be read, so the parameter {} has nowhere to go", first.name)),
            None => Ok(xslt.to_owned()),
        };
    };
    let Ok(top) = xot.document_element(root) else { return Ok(xslt.to_owned()) };
    let declared: Vec<xot::Node> = xot.children(top).filter(|c| is_xslt(&xot, *c, "param")).collect();
    let mut literals: Vec<(String, String)> = Vec::new();
    for node in &declared {
        let Some(name) = attribute(&xot, *node, "name") else { continue };
        let value = match params.iter().find(|p| p.name == name) {
            Some(given) => given.value.clone(),
            None => match attribute(&xot, *node, "select") {
                Some(select) => literal_default(&select).ok_or_else(|| TransformError {
                    message: format!(
                        "the parameter {name} has the default select=\"{select}\", an expression, which is not evaluated: give it a value"
                    ),
                })?,
                None => ParamValue::String(String::new()),
            },
        };
        literals.push((name, literal_of(&value)?));
    }
    if let Some(unknown) = params.iter().find(|p| !literals.iter().any(|(n, _)| *n == p.name)) {
        return fail(format!("the stylesheet declares no top-level xsl:param {}", unknown.name));
    }
    if declared.is_empty() {
        return Ok(xslt.to_owned());
    }
    for node in declared {
        xot.remove(node).map_err(|_| TransformError { message: "the stylesheet cannot be changed".to_owned() })?;
    }
    let elements: Vec<xot::Node> = xot.descendants(top).filter(|n| xot.element(*n).is_some()).collect();
    for node in elements {
        let in_xslt = xot.element(node).is_some_and(|e| xot.name_ns_str(e.name()).1 == XSLT_NAMESPACE);
        let changes: Vec<(xot::NameId, String)> = xot
            .attributes(node)
            .iter()
            .filter_map(|(name, value)| {
                let (local, namespace) = xot.name_ns_str(name);
                if !namespace.is_empty() || !value.contains('$') && !value.contains('{') {
                    return None;
                }
                let changed = if in_xslt && XPATH_ATTRIBUTES.contains(&local) {
                    substitute(value, &literals)
                } else if in_xslt && matches!(local, "match" | "mode" | "version" | "as" | "priority") {
                    return None;
                } else {
                    substitute_template(value, &literals)
                };
                (changed != *value).then_some((name, changed))
            })
            .collect();
        for (name, value) in changes {
            xot.set_attribute(node, name, value);
        }
    }
    xot.to_string(root).map_err(|_| TransformError { message: "the stylesheet cannot be written".to_owned() })
}

fn run(xml: &str, xslt: &str, params: &[Param]) -> Result<Transformed, TransformError> {
    let xslt = with_parameters(xslt, params)?;
    let xslt = xslt.as_str();
    let mut xot = Xot::new();
    let source = crate::without_doctype(xml);
    let root = xot.parse(&source).map_err(|e| TransformError { message: format!("the document cannot be read: {e}") })?;
    let namespaces = Namespaces::new(Namespaces::default_namespaces(), String::new(), FN_NAMESPACE.to_string());
    let program = xee_xslt_compiler::parse(StaticContext::from_namespaces(namespaces), xslt).map_err(|e| TransformError {
        message: format!("the stylesheet cannot be used: {} (the engine does not implement all of XSLT)", describe(&e)),
    })?;
    // (What the engine's own `evaluate` does after it has parsed: the document is the context node of the program.)
    let mut documents = xee_interpreter::xml::Documents::new();
    let handle = documents.add_root(None, root).map_err(|_| TransformError { message: "the document cannot be used".to_owned() })?;
    let context_node = documents.get_node_by_handle(handle).ok_or_else(|| TransformError { message: "the document cannot be used".to_owned() })?;
    let mut builder = program.dynamic_context_builder();
    builder.context_node(context_node);
    builder.documents(documents);
    let context = builder.build();
    let sequence =
        program.runnable(&context).many(&mut xot).map_err(|e| TransformError { message: format!("the transformation failed: {}", describe(&e)) })?;
    let mut text = String::new();
    let mut items = 0usize;
    for item in sequence.iter() {
        items += 1;
        match item {
            Item::Node(node) => text.push_str(&xot.to_string(node).unwrap_or_else(|_| xot.string_value(node))),
            Item::Atomic(atomic) => {
                if !text.is_empty() && !text.ends_with(['\n', ' ']) {
                    text.push(' ');
                }
                text.push_str(&unquote(&atomic.xpath_representation()));
            }
            Item::Function(_) => return fail("the transformation made a function, which has no text"),
        }
        if text.len() > MAX_OUTPUT_BYTES {
            return fail("the result is larger than the 64 MB that is written out");
        }
    }
    Ok(Transformed { text, items })
}

fn describe(e: &xee_interpreter::error::SpannedError) -> String {
    let code = e.error.code();
    let message = e.error.message();
    if message.is_empty() { code } else { format!("{code}: {message}") }
}

/// An XPath string representation (`"a"`) as the string: one pair of the surrounding quotes off.
fn unquote(representation: &str) -> String {
    let quote = representation.chars().next().filter(|c| matches!(c, '"' | '\''));
    match quote {
        Some(q) if representation.len() >= 2 && representation.ends_with(q) => {
            representation.get(1..representation.len() - 1).unwrap_or_default().replace(&format!("{q}{q}"), &q.to_string())
        }
        _ => representation.to_owned(),
    }
}
