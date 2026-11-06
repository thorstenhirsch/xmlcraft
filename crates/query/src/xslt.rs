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
    // (The engine is early: it can panic on input it does not expect, so a panic is an error here, not a crash.)
    match std::panic::catch_unwind(|| run(xml, xslt)) {
        Ok(done) => done,
        Err(_) => fail("the XSLT engine stopped on this input (it does not implement everything of XSLT); nothing was produced"),
    }
}

fn run(xml: &str, xslt: &str) -> Result<Transformed, TransformError> {
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
