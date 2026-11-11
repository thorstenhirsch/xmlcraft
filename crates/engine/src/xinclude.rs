//! XInclude 1.0: the `xi:include` elements of a document expanded into the text they name.
//!
//! An `xi:include` names a resource with `href`, found through the resolver (the open documents, the catalogs, the bundle, files and,
//! when the person allows it, the web). `parse="xml"` (the default) includes the document element of the resource; `parse="text"`
//! includes its text as character data. An include that cannot be expanded uses its `xi:fallback` content when it has some.
//!
//! This is the basic processing of the specification: `xpointer`, `accept`, `accept-language`, namespace fixup and the `xml:base`
//! of the included content are not done yet (an include that asks for an `xpointer` is a resource error, so its fallback, if any,
//! is used). The fatal errors of the specification (an include of the document it is in, directly or through others; a `parse` that
//! is neither `xml` nor `text`; an empty or missing `href` with no `xpointer`, which would include the whole document it is in, §3.1)
//! end the expansion and are not answered by a fallback. An empty `xi:fallback` includes nothing. Cycles are stopped by the stack of
//! documents being expanded, which starts with the document itself, and the resolver's budget bounds what is read.
//!
//! The result is a list of edits, one for each outermost `xi:include` (its range in the text and what replaces it, with the line
//! endings of the document), so that the editor changes only those ranges.

use std::ops::Range;
use std::sync::Arc;

use xmlcraft_resolve::{Budget, Request, Resolver, join, normalized};
use xmlcraft_syntax::{SyntaxKind, SyntaxNode, decode_references, parse};
use xmlcraft_text::encoding::{Detected, Encoding, decode, sniff};
use xmlcraft_text::eol::{Eol, LineEndings};
use xmlcraft_text::{Buffer, Edit, apply_edits};

use crate::validation::{OpenDocuments, ResolverFactory};
use crate::worker::Worker;

/// The XInclude namespace.
pub const XINCLUDE_NAMESPACE: &str = "http://www.w3.org/2001/XInclude";
/// Includes nested deeper than this are refused.
const MAX_DEPTH: usize = 32;
/// The most includes one expansion replaces.
const MAX_INCLUDES: usize = 1000;
/// The longest text an expansion makes (the same resource may be included many times).
const MAX_EXPANDED_BYTES: usize = 64 * 1024 * 1024;

/// The text with its includes expanded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expanded {
    pub text: String,
    /// How many `xi:include` elements were replaced by what they name (an include answered by its fallback is not counted).
    pub includes: usize,
    /// One edit for each outermost include, in the coordinates of the text that was expanded, ascending: `text` is the text with them.
    pub edits: Vec<Edit>,
}

/// Why an include could not be expanded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub message: String,
    /// A fatal error of the specification: a fallback does not answer it.
    pub fatal: bool,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

fn error(message: impl Into<String>) -> Error {
    Error { message: message.into(), fatal: false }
}

fn fatal(message: impl Into<String>) -> Error {
    Error { message: message.into(), fatal: true }
}

/// Expand every include in `text`, whose location is `base` (a URI or a path; relative `href`s are resolved against it, and an include
/// of it is a cycle).
pub fn expand(text: &str, base: &str, resolver: &Resolver) -> Result<Expanded, Error> {
    let mut budget = Budget::default();
    let mut stack = Vec::new();
    if !base.is_empty() {
        stack.push(normalized(base));
    }
    let mut count = 0usize;
    let eol = LineEndings::count(text).dominant();
    let edits: Vec<Edit> = replacements(text, base, resolver, &mut stack, &mut budget, 0, &mut count)?
        .into_iter()
        .map(|(range, replacement)| Edit::replace(range, with_line_ending(&replacement, eol)))
        .collect();
    Ok(Expanded { text: apply_edits(text, &edits), includes: count, edits })
}

/// `text` with every line break written as `eol`.
fn with_line_ending(text: &str, eol: Eol) -> String {
    if !text.contains(['\r', '\n']) {
        return text.to_owned();
    }
    text.replace("\r\n", "\n").replace('\r', "\n").replace('\n', eol.as_str())
}

/// `text` with its includes expanded.
fn expand_into(
    text: &str,
    base: &str,
    resolver: &Resolver,
    stack: &mut Vec<String>,
    budget: &mut Budget,
    depth: usize,
    count: &mut usize,
) -> Result<String, Error> {
    let edits: Vec<Edit> =
        replacements(text, base, resolver, stack, budget, depth, count)?.into_iter().map(|(range, text)| Edit::replace(range, text)).collect();
    let out = apply_edits(text, &edits);
    if out.len() > MAX_EXPANDED_BYTES {
        return Err(fatal(format!("the expanded text would have more than {MAX_EXPANDED_BYTES} bytes")));
    }
    Ok(out)
}

/// The range of each outermost include of `text` and what replaces it.
fn replacements(
    text: &str,
    base: &str,
    resolver: &Resolver,
    stack: &mut Vec<String>,
    budget: &mut Budget,
    depth: usize,
    count: &mut usize,
) -> Result<Vec<(Range<usize>, String)>, Error> {
    if depth > MAX_DEPTH {
        return Err(error(format!("includes are nested more than {MAX_DEPTH} deep")));
    }
    let parsed = parse(text);
    let errors: Vec<_> = parsed.diagnostics().iter().filter(|d| d.severity() == xmlcraft_syntax::Severity::Error).collect();
    // A fallback is a fragment: it has no root of its own and may be several elements or text, so its parse problems are ignored.
    if depth == 0 && !errors.is_empty() {
        let first = errors.first().map(|d| d.message()).unwrap_or_default();
        return Err(error(format!("the document is not well-formed: {first}")));
    }
    let root = parsed.syntax();
    let includes: Vec<SyntaxNode> = root.descendants().filter(|n| n.kind() == SyntaxKind::Element && is_include(n) && !inside_include(n)).collect();
    let mut out = Vec::with_capacity(includes.len());
    let mut total = 0usize;
    for include in includes {
        if *count >= MAX_INCLUDES {
            return Err(fatal(format!("more than {MAX_INCLUDES} includes")));
        }
        let range = usize::from(include.text_range().start())..usize::from(include.text_range().end());
        if text.get(range.clone()).is_none() {
            continue;
        }
        let replacement = replace(&include, At { text, base, depth }, resolver, stack, budget, count)?;
        total = total.saturating_add(replacement.len());
        if total > MAX_EXPANDED_BYTES {
            return Err(fatal(format!("the expanded text would have more than {MAX_EXPANDED_BYTES} bytes")));
        }
        out.push((range, replacement));
    }
    Ok(out)
}

/// Where an include is: the text it is in, its base, and how deep the includes already are.
#[derive(Clone, Copy)]
struct At<'a> {
    text: &'a str,
    base: &'a str,
    depth: usize,
}

fn replace(
    include: &SyntaxNode,
    at: At<'_>,
    resolver: &Resolver,
    stack: &mut Vec<String>,
    budget: &mut Budget,
    count: &mut usize,
) -> Result<String, Error> {
    match expand_one(include, at.base, resolver, stack, budget, at.depth, count) {
        Ok(replacement) => {
            *count = count.saturating_add(1);
            Ok(replacement)
        }
        Err(e) if e.fatal => Err(e),
        Err(e) => match fallback(include, at.base) {
            Some((range, fallback_base)) => {
                let fragment = range.and_then(|range| at.text.get(range)).unwrap_or_default();
                expand_into(fragment, &fallback_base, resolver, stack, budget, at.depth.saturating_add(1), count)
                    .map_err(|inner| Error { message: format!("{}; and its fallback: {}", e.message, inner.message), fatal: inner.fatal })
            }
            None => Err(e),
        },
    }
}

fn expand_one(
    include: &SyntaxNode,
    base: &str,
    resolver: &Resolver,
    stack: &mut Vec<String>,
    budget: &mut Budget,
    depth: usize,
    count: &mut usize,
) -> Result<String, Error> {
    let attrs = attributes(include);
    let has_xpointer = attrs.iter().any(|(name, _)| name == "xpointer");
    let parse_mode = attr(&attrs, "parse").unwrap_or_else(|| "xml".to_owned());
    if parse_mode != "xml" && parse_mode != "text" {
        return Err(fatal(format!("parse=\"{parse_mode}\" is neither xml nor text")));
    }
    let href = attr(&attrs, "href").unwrap_or_default();
    if href.is_empty() && !has_xpointer {
        // (XInclude 1.0 §3.1: with neither, the include would name the whole document it is in.)
        return Err(fatal("an include with an empty or no href needs an xpointer (it would include the document it is in)"));
    }
    if has_xpointer {
        return Err(error("an xpointer is not supported"));
    }
    let base = base_with_xml_base(include, base, &attrs);
    let request = Request::uri(&href, &base);
    let resolved = resolver.resolve(&request, budget).map_err(|e| error(format!("{href}: {}", e.message)))?;
    let location = normalized(&resolved.location);
    if stack.contains(&location) {
        return Err(fatal(format!("{href}: the include goes round in a circle (it includes a document that is being included)")));
    }
    if parse_mode == "text" {
        let detected = attr(&attrs, "encoding")
            .and_then(|label| Encoding::from_label(&label))
            .map_or_else(|| sniff(&resolved.bytes), |encoding| Detected { encoding, bom: false, declared: None });
        let decoded = decode(&resolved.bytes, &detected);
        return Ok(escape_text(&decoded.text));
    }
    let decoded = decode(&resolved.bytes, &sniff(&resolved.bytes));
    if decoded.lossy {
        return Err(error(format!("{href}: its bytes are not valid in its encoding")));
    }
    stack.push(location);
    let expanded = expand_into(&decoded.text, &resolved.location, resolver, stack, budget, depth.saturating_add(1), count);
    stack.pop();
    let expanded = expanded?;
    let parsed = parse(&expanded);
    if parsed.diagnostics().iter().any(|d| d.severity() == xmlcraft_syntax::Severity::Error) {
        return Err(error(format!("{href}: the included document is not well-formed")));
    }
    let root = parsed.syntax();
    let Some(element) = root.children().find(|n| n.kind() == SyntaxKind::Element) else {
        return Err(error(format!("{href}: the included document has no element")));
    };
    let range = usize::from(element.text_range().start())..usize::from(element.text_range().end());
    Ok(expanded.get(range).unwrap_or_default().to_owned())
}

/// The fallback of an include: the range of its content (`None` for an empty `<xi:fallback/>`, which includes nothing), and the base
/// its relative references are resolved against.
fn fallback(include: &SyntaxNode, base: &str) -> Option<(Option<Range<usize>>, String)> {
    let fallback = include.children().find(|n| n.kind() == SyntaxKind::Element && local_name(n).as_deref() == Some("fallback") && in_xinclude(n))?;
    let start = fallback.children().find(|n| n.kind() == SyntaxKind::StartTag);
    let end = fallback.children().find(|n| n.kind() == SyntaxKind::EndTag);
    let range = match (start, end) {
        (Some(start), Some(end)) => Some(usize::from(start.text_range().end())..usize::from(end.text_range().start())),
        _ => None,
    };
    let base = base_with_xml_base(&fallback, base, &attributes(&fallback));
    Some((range, base))
}

/// `base` with the `xml:base` of the include and of its ancestors, outermost first.
fn base_with_xml_base(include: &SyntaxNode, base: &str, own: &[(String, String)]) -> String {
    let mut chain: Vec<SyntaxNode> = include.ancestors().skip(1).filter(|n| n.kind() == SyntaxKind::Element).collect();
    chain.reverse();
    let mut base = base.to_owned();
    for element in chain {
        if let Some(value) = attr(&attributes(&element), "xml:base") {
            base = join(&base, &value);
        }
    }
    if let Some(value) = attr(own, "xml:base") {
        base = join(&base, &value);
    }
    base
}

/// The attributes of an element, decoded: name as written, value without quotes.
fn attributes(element: &SyntaxNode) -> Vec<(String, String)> {
    let Some(start) = element.children().find(|n| n.kind() == SyntaxKind::StartTag) else { return Vec::new() };
    start
        .children()
        .filter(|n| n.kind() == SyntaxKind::Attribute)
        .filter_map(|attribute| {
            let name = attribute
                .children_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| t.kind() == SyntaxKind::Name)
                .map(|t| t.text().to_owned())?;
            let value = attribute.children().find(|n| n.kind() == SyntaxKind::AttrValue)?;
            let raw = value.text().to_string();
            let quote = raw.chars().next()?;
            let inner = raw.strip_prefix(quote).and_then(|s| s.strip_suffix(quote)).unwrap_or(&raw);
            Some((name, decode_references(inner)))
        })
        .collect()
}

fn attr(attributes: &[(String, String)], name: &str) -> Option<String> {
    attributes.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone())
}

fn local_name(element: &SyntaxNode) -> Option<String> {
    let start = element.children().find(|n| n.kind() == SyntaxKind::StartTag)?;
    let name = start.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == SyntaxKind::Name)?;
    Some(name.text().rsplit_once(':').map_or(name.text(), |(_, local)| local).to_owned())
}

fn prefix_of(element: &SyntaxNode) -> Option<String> {
    let start = element.children().find(|n| n.kind() == SyntaxKind::StartTag)?;
    let name = start.children_with_tokens().filter_map(|e| e.into_token()).find(|t| t.kind() == SyntaxKind::Name)?;
    name.text().rsplit_once(':').map(|(prefix, _)| prefix.to_owned())
}

/// The namespace of an element: its prefix's declaration on itself or an ancestor, or the default namespace.
fn namespace_of(element: &SyntaxNode) -> Option<String> {
    let prefix = prefix_of(element);
    let key = prefix.as_ref().map_or_else(|| "xmlns".to_owned(), |p| format!("xmlns:{p}"));
    for ancestor in element.ancestors().filter(|n| n.kind() == SyntaxKind::Element) {
        if let Some(value) = attr(&attributes(&ancestor), &key) {
            return (!value.is_empty()).then_some(value);
        }
    }
    None
}

fn in_xinclude(element: &SyntaxNode) -> bool {
    namespace_of(element).as_deref() == Some(XINCLUDE_NAMESPACE)
}

fn is_include(node: &SyntaxNode) -> bool {
    local_name(node).as_deref() == Some("include") && in_xinclude(node)
}

fn inside_include(node: &SyntaxNode) -> bool {
    node.ancestors().skip(1).any(|n| n.kind() == SyntaxKind::Element && is_include(&n))
}

/// Text included with `parse="text"` is character data: the markup characters are escaped so that it stays one.
fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// Whether a text has at least one include (what the app asks before it bothers to expand).
pub fn has_include(text: &str) -> bool {
    let parsed = parse(text);
    parsed.syntax().descendants().any(|n| n.kind() == SyntaxKind::Element && is_include(&n))
}

// --- the worker ----------------------------------------------------------------------------------------------------------------

/// One request to expand the includes of a document (XML ▸ Expand XIncludes).
pub struct ExpandJob {
    /// The version of the text, so that the edits are applied only to the text they were made for.
    pub version: u64,
    pub text: String,
    /// The document's location as a URI (empty when it has no file).
    pub base: String,
    /// The documents that are open, by their paths, so that an included document being edited is read as it is.
    pub open: Vec<(String, Buffer)>,
}

pub struct ExpandOutcome {
    pub version: u64,
    pub result: Result<Expanded, Error>,
}

fn run(resolvers: &ResolverFactory, job: ExpandJob) -> ExpandOutcome {
    let resolver = resolvers(Arc::new(OpenDocuments::new(job.open)));
    let result = crate::guard(|| expand(&job.text, &job.base, &resolver)).unwrap_or_else(|panic| {
        log::error!("expanding XIncludes failed: {panic}");
        Err(fatal("expanding the XIncludes failed (a bug); nothing was changed"))
    });
    ExpandOutcome { version: job.version, result }
}

/// Expands includes on a worker thread: the resources go through the resolver, which may download them.
pub struct XIncludeExpander {
    worker: Worker<ExpandJob, ExpandOutcome>,
}

impl XIncludeExpander {
    pub fn new(resolvers: ResolverFactory) -> Self {
        Self { worker: Worker::new("xinclude", move |job| run(&resolvers, job)) }
    }

    /// An expander with no thread, for tests and the web.
    pub fn inline(resolvers: ResolverFactory) -> Self {
        Self { worker: Worker::inline(move |job| run(&resolvers, job)) }
    }

    pub fn on_done(&self, notify: impl Fn() + Send + Sync + 'static) {
        self.worker.on_done(notify);
    }

    /// Expand the includes of the text of `document`.
    pub fn submit(&self, document: u64, job: ExpandJob) {
        self.worker.submit(document, job);
    }

    /// What finished, by document.
    pub fn poll(&self) -> Vec<(u64, ExpandOutcome)> {
        self.worker.poll()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use xmlcraft_resolve::{Environment, FileAccess, FixedClock, NoBuffers, NoFiles, Policy};

    struct Buffers(HashMap<String, String>);
    impl xmlcraft_resolve::Buffers for Buffers {
        fn text(&self, location: &str) -> Option<Arc<str>> {
            self.0.get(&normalized(location)).map(|s| Arc::from(s.as_str()))
        }
    }

    struct Files(HashMap<String, Vec<u8>>);
    impl FileAccess for Files {
        fn read(&self, path: &str) -> Result<Vec<u8>, String> {
            self.0.get(&normalized(path)).cloned().ok_or_else(|| format!("{path}: no such file"))
        }
    }

    fn resolver(buffers: &[(&str, &str)], files: &[(&str, &[u8])]) -> Resolver {
        let env = Environment {
            buffers: Arc::new(Buffers(buffers.iter().map(|(k, v)| (normalized(k), (*v).to_owned())).collect())),
            files: Arc::new(Files(files.iter().map(|(k, v)| (normalized(k), (*v).to_vec())).collect())),
            fetcher: None,
            cache: None,
            clock: Arc::new(FixedClock(0)),
        };
        Resolver::new(env, Policy::default(), &[])
    }

    #[test]
    fn an_include_is_replaced_by_the_document_element_it_names() {
        let text = "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"><xi:include href=\"b.xml\"/></a>";
        let r = resolver(&[("u:/b.xml", "<?xml version=\"1.0\"?><b>in</b>")], &[]);
        let got = expand(text, "u:/a.xml", &r).expect("expands");
        assert_eq!(got.text, "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"><b>in</b></a>");
        assert_eq!(got.includes, 1);
    }

    #[test]
    fn each_outermost_include_is_one_edit_of_its_own_range_written_with_the_documents_line_endings() {
        let text = "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\">\r\n  <xi:include href=\"b.xml\"/>\r\n  <keep/>\r\n  <xi:include href=\"c.txt\" parse=\"text\"/>\r\n</a>";
        let r = resolver(&[("u:/b.xml", "<b>\n  <x/>\n</b>"), ("u:/c.txt", "one\ntwo")], &[]);
        let got = expand(text, "u:/a.xml", &r).expect("expands");
        let ranges: Vec<&str> = got.edits.iter().map(|e| &text[e.range.clone()]).collect();
        assert_eq!(ranges, ["<xi:include href=\"b.xml\"/>", "<xi:include href=\"c.txt\" parse=\"text\"/>"], "only the includes are replaced");
        assert_eq!(got.edits[0].text, "<b>\r\n  <x/>\r\n</b>");
        assert_eq!(got.edits[1].text, "one\r\ntwo");
        assert_eq!(got.text, "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\">\r\n  <b>\r\n  <x/>\r\n</b>\r\n  <keep/>\r\n  one\r\ntwo\r\n</a>");
    }

    #[test]
    fn the_document_itself_is_on_the_cycle_stack_and_an_empty_href_without_an_xpointer_is_fatal() {
        // The file a.xml on disk has no include; the text being expanded is a.xml as it is being edited, and includes a.xml: a loop.
        let text = "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"><xi:include href=\"a.xml\"/></a>";
        let r = resolver(&[], &[("u:/a.xml", b"<a/>")]);
        let e = expand(text, "u:/a.xml", &r).expect_err("a loop");
        assert!(e.fatal && e.message.contains("circle"), "{e}");
        let empty = "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"><xi:include href=\"\"><xi:fallback>no</xi:fallback></xi:include></a>";
        let e = expand(empty, "u:/a.xml", &r).expect_err("fatal");
        assert!(e.fatal && e.message.contains("xpointer"), "a fallback does not answer a fatal error: {e}");
        let bad_parse = "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"><xi:include href=\"a.xml\" parse=\"html\"/></a>";
        assert!(expand(bad_parse, "u:/a.xml", &r).is_err_and(|e| e.fatal));
    }

    #[test]
    fn an_empty_fallback_includes_nothing() {
        let text = "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"><xi:include href=\"none.xml\"><xi:fallback/></xi:include></a>";
        let r = resolver(&[], &[]);
        let got = expand(text, "u:/a.xml", &r).expect("expands");
        assert_eq!(got.text, "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"></a>");
        assert_eq!((got.includes, got.edits.len()), (0, 1));
    }

    #[test]
    fn the_expander_answers_with_the_version_it_was_given() {
        let factory: ResolverFactory = Arc::new(|_| resolver(&[("u:/b.xml", "<b/>")], &[]));
        let expander = XIncludeExpander::inline(factory);
        let text = "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"><xi:include href=\"b.xml\"/></a>".to_owned();
        expander.submit(3, ExpandJob { version: 9, text, base: "u:/a.xml".to_owned(), open: Vec::new() });
        let got = expander.poll();
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].0, got[0].1.version), (3, 9));
        assert!(got[0].1.result.as_ref().is_ok_and(|e| e.includes == 1));
    }

    #[test]
    fn a_text_include_is_escaped_so_that_the_result_is_still_xml() {
        let text = "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"><xi:include href=\"b.txt\" parse=\"text\"/></a>";
        let r = resolver(&[("u:/b.txt", "a < b & c")], &[]);
        assert_eq!(expand(text, "u:/a.xml", &r).expect("expands").text, "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\">a &lt; b &amp; c</a>");
    }

    #[test]
    fn a_missing_resource_uses_the_fallback() {
        let text = "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"><xi:include href=\"none.xml\"><xi:fallback><b>fallback</b></xi:fallback></xi:include></a>";
        let r = resolver(&[], &[]);
        let got = expand(text, "u:/a.xml", &r).expect("expands");
        assert_eq!(got.text, "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"><b>fallback</b></a>");
        assert_eq!(got.includes, 0, "the failed include was not replaced; its fallback was");
    }

    #[test]
    fn nested_includes_are_expanded_and_a_cycle_stops() {
        let a = "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"><xi:include href=\"b.xml\"/></a>";
        let b = "<b xmlns:xi=\"http://www.w3.org/2001/XInclude\"><xi:include href=\"c.xml\"/></b>";
        let c = "<c/>";
        let r = resolver(&[("u:/a.xml", a), ("u:/b.xml", b), ("u:/c.xml", c)], &[]);
        assert_eq!(
            expand(a, "u:/a.xml", &r).expect("expands").text,
            "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"><b xmlns:xi=\"http://www.w3.org/2001/XInclude\"><c/></b></a>"
        );
        let cycle = resolver(&[("u:/a.xml", "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"><xi:include href=\"a.xml\"/></a>")], &[]);
        assert!(expand("<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"><xi:include href=\"a.xml\"/></a>", "u:/a.xml", &cycle).is_err());
    }

    #[test]
    fn a_file_next_to_the_document_is_read_and_text_is_decoded() {
        let text = "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"><xi:include href=\"b.xml\"/></a>";
        let r = resolver(&[], &[("u:/b.xml", b"<b/>")]);
        assert_eq!(expand(text, "u:/a.xml", &r).expect("expands").text, "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"><b/></a>");
    }

    #[test]
    fn an_include_in_another_namespace_is_left_alone() {
        let text = "<a xmlns:x=\"urn:not-xinclude\"><x:include href=\"b.xml\"/></a>";
        let r = resolver(&[], &[]);
        assert_eq!(expand(text, "u:/a.xml", &r).expect("expands").text, text);
    }

    #[test]
    fn an_xpointer_is_refused_and_its_fallback_used() {
        let text = "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"><xi:include href=\"b.xml\" xpointer=\"/b\"><xi:fallback>no</xi:fallback></xi:include></a>";
        let r = resolver(&[("u:/b.xml", "<b/>")], &[]);
        assert_eq!(expand(text, "u:/a.xml", &r).expect("expands").text, "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\">no</a>");
    }

    #[test]
    fn the_exported_helpers_never_panic_on_any_text() {
        // Not a property test, but the same promise: any text gives an answer, or an error, without a panic.
        for text in ["", "<", "<a", "<xi:include", "<a xmlns:xi=\"http://www.w3.org/2001/XInclude\"><xi:include/></a>"] {
            let r = resolver(&[], &[("u:/b", b"x")]);
            let _ = expand(text, "u:/a", &r);
            let _ = has_include(text);
        }
        let _ = NoBuffers;
        let _ = NoFiles;
    }
}
