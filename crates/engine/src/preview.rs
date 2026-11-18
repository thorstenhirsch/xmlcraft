//! What the Preview tool window shows for a document, and the rules the browser view that shows it keeps.
//!
//! [`prepare`] turns a document into the page a browser shows: an HTML document as it is (its markup is never rewritten); an XML
//! document that names an XSLT stylesheet (or has one chosen) as the stylesheet's result, when the stylesheet writes HTML; an SVG
//! image as it is; any other XML document as a readable tree of our own (escaped, indented, collapsible with `<details>`, no script).
//! It is pure and bounded: the text and the result are capped at [`MAX_BYTES`], the tree at [`MAX_TREE_NODES`] nodes and
//! [`MAX_TREE_DEPTH`] levels, and what was cut or could not be done is a warning.
//!
//! The rest is the security policy of the browser view, as pure functions so that it is tested exactly:
//! - the page is served from memory at [`DOCUMENT_URL`] (a custom scheme; WebView2 sees it as [`DOCUMENT_URL_WEBVIEW2`]) by [`serve`],
//!   with the [`content_security_policy`] (`default-src 'none'`, inline styles, `data:` images and fonts, no forms, no `<base>`, a
//!   sandbox without scripts) and `X-Content-Type-Options: nosniff`, among the [`response_headers`];
//! - [`allows_navigation`] lets only the document itself load (the first load, a reload, an anchor in it): every link, form, refresh
//!   and redirect elsewhere is refused;
//! - per document a person may allow remote images (`img-src https:`) and inline scripts (`script-src 'unsafe-inline'`), never by
//!   default ([`Allow`]);
//! - [`fallback_page`] is the copy written for the system browser when there is no browser view: the same policy as a
//!   `<meta http-equiv>` after the doctype (an SVG image is shown through `<img>`, where it runs nothing and loads nothing).

use std::fmt;

use xmlcraft_syntax::{Severity, SyntaxKind, SyntaxNode, decode_references, parse, split_qname};

/// The most bytes of a document, and of a stylesheet's result, the preview shows.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
/// The most elements, texts, comments and processing instructions the tree of an XML document shows.
pub const MAX_TREE_NODES: usize = 20_000;
/// The deepest the tree of an XML document goes.
pub const MAX_TREE_DEPTH: usize = 200;
/// The most characters of one text the tree shows.
const MAX_TEXT_CHARS: usize = 4_000;
/// The most characters of a title.
const MAX_TITLE_CHARS: usize = 200;
/// How far into a document [`names_stylesheet`] looks for its prolog.
const PROLOG_BYTES: usize = 64 * 1024;

pub const SVG_NAMESPACE: &str = "http://www.w3.org/2000/svg";
const XSLT_NAMESPACE: &str = "http://www.w3.org/1999/XSL/Transform";

/// The scheme the browser view serves the document under.
pub const SCHEME: &str = "xmlcraft-preview";
/// Where the browser view loads the document from (WKWebView, WebKitGTK).
pub const DOCUMENT_URL: &str = "xmlcraft-preview://localhost/document";
/// The same address as WebView2 sees it: it has no custom schemes, and wry serves them as `https://<scheme>.<host>`.
pub const DOCUMENT_URL_WEBVIEW2: &str = "https://xmlcraft-preview.localhost/document";
/// The path of [`DOCUMENT_URL`].
pub const DOCUMENT_PATH: &str = "/document";

/// The document to preview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input<'a> {
    /// An HTML document: shown as it is.
    Html(&'a str),
    /// An XML document, and the text of the XSLT stylesheet to apply to it (the one it names, or one chosen), when there is one.
    Xml { text: &'a str, stylesheet: Option<&'a str> },
}

/// How the document is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    /// The HTML document as it is.
    Html,
    /// The HTML a stylesheet made of the XML document.
    Transformed,
    /// The SVG image as it is.
    Svg,
    /// The XML document as a tree.
    Tree,
}

impl Shown {
    /// The name the automation tool reports.
    pub fn name(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Transformed => "xslt",
            Self::Svg => "svg",
            Self::Tree => "tree",
        }
    }
}

/// The page for a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    /// What the browser is given: the HTML (or the SVG).
    pub body: String,
    /// Its media type, with the charset (the body is UTF-8).
    pub media_type: &'static str,
    pub shown: Shown,
    /// The page's title: the HTML's `<title>`, else the root element's name.
    pub title: Option<String>,
    /// What could not be shown as it is, or was cut.
    pub warnings: Vec<String>,
}

pub const HTML_MEDIA_TYPE: &str = "text/html; charset=utf-8";
pub const SVG_MEDIA_TYPE: &str = "image/svg+xml; charset=utf-8";

/// Why there is no page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewError {
    /// The document, or what its stylesheet made, is larger than [`MAX_BYTES`].
    TooLarge { bytes: usize, limit: usize },
}

impl fmt::Display for PreviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { bytes, limit } => {
                write!(f, "the page would be {} MB, more than the {} MB the preview shows", megabytes(*bytes), megabytes(*limit))
            }
        }
    }
}

impl std::error::Error for PreviewError {}

fn megabytes(bytes: usize) -> String {
    // (A display value: precision does not matter, and usize to f64 cannot fail.)
    format!("{:.1}", bytes as f64 / (1024.0 * 1024.0))
}

fn too_large(text: &str) -> Result<(), PreviewError> {
    if text.len() > MAX_BYTES { Err(PreviewError::TooLarge { bytes: text.len(), limit: MAX_BYTES }) } else { Ok(()) }
}

/// The page a browser shows for a document.
pub fn prepare(input: Input<'_>) -> Result<Prepared, PreviewError> {
    match input {
        Input::Html(text) => {
            too_large(text)?;
            Ok(Prepared { body: text.to_owned(), media_type: HTML_MEDIA_TYPE, shown: Shown::Html, title: html_title(text), warnings: Vec::new() })
        }
        Input::Xml { text, stylesheet } => {
            too_large(text)?;
            let mut warnings = Vec::new();
            if let Some(sheet) = stylesheet {
                too_large(sheet)?;
                // (The engine implements no serialization declarations: `xsl:output` is read here and left out of what it compiles.)
                let output = output_declarations(sheet);
                let compiled = without(sheet, &output.ranges);
                match crate::guard(|| xmlcraft_query::transform(text, &compiled)) {
                    Ok(Ok(done)) => match html_of_result(output.method.as_deref(), &done.text) {
                        Some(html) => {
                            too_large(&html)?;
                            let title = html_title(&html);
                            return Ok(Prepared { body: html, media_type: HTML_MEDIA_TYPE, shown: Shown::Transformed, title, warnings });
                        }
                        None => warnings.push(
                            "The stylesheet does not write HTML (its output method is not html or xhtml): the document is shown as a tree."
                                .to_owned(),
                        ),
                    },
                    Ok(Err(e)) => warnings.push(format!("The stylesheet could not be applied ({e}): the document is shown as a tree.")),
                    Err(_) => {
                        warnings.push("The stylesheet could not be applied (the XSLT engine failed): the document is shown as a tree.".to_owned())
                    }
                }
            }
            let parsed = parse(text);
            let root = parsed.syntax();
            let document_element = root.children().find(|n| n.kind() == SyntaxKind::Element);
            if stylesheet.is_none() && document_element.as_ref().is_some_and(is_svg) {
                return Ok(Prepared {
                    body: text.to_owned(),
                    media_type: SVG_MEDIA_TYPE,
                    shown: Shown::Svg,
                    title: Some("SVG".to_owned()),
                    warnings,
                });
            }
            if parsed.diagnostics().iter().any(|d| d.severity() == Severity::Error) {
                warnings.push("The document is not well-formed XML: the tree is what the editor reads of it.".to_owned());
            }
            let title = document_element.as_ref().and_then(tag_name).map(|n| clip(&n, MAX_TITLE_CHARS));
            let mut tree = Tree::default();
            tree.document(&root);
            if tree.cut_nodes {
                warnings.push(format!("The tree shows the first {MAX_TREE_NODES} nodes."));
            }
            if tree.cut_depth {
                warnings.push(format!("The tree goes {MAX_TREE_DEPTH} levels deep; deeper elements are left out."));
            }
            let body = tree_page(title.as_deref().unwrap_or("XML"), &tree.out);
            Ok(Prepared { body, media_type: HTML_MEDIA_TYPE, shown: Shown::Tree, title, warnings })
        }
    }
}

/// Whether the prolog of an XML document (what comes before its root element) has an `xml-stylesheet` instruction for XSLT: a cheap
/// look, for the command bar.
pub fn names_stylesheet(text: &str) -> bool {
    stylesheet_href(text).is_some()
}

/// The `href` of the XSLT stylesheet the prolog of an XML document names (as `crate::xslt::stylesheet_of` reads it), from a look at the
/// first 64 KiB only.
pub fn stylesheet_href(text: &str) -> Option<String> {
    let mut end = text.len().min(PROLOG_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let head = text.get(..end).unwrap_or_default();
    // The prolog ends where the first start tag does: `<` and a name (not `<?`, `<!`).
    let prolog_end = head
        .match_indices('<')
        .find(|(i, _)| head.get(i + 1..).and_then(|r| r.chars().next()).is_some_and(xmlcraft_syntax::is_name_start_char))
        .map_or(head.len(), |(i, _)| i);
    let prolog = head.get(..prolog_end).unwrap_or_default();
    if !prolog.contains("<?xml-stylesheet") {
        return None;
    }
    crate::xslt::stylesheet_of(prolog)
}

/// What remote content and scripts a person allowed for one document. Nothing, by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Allow {
    /// Images from `https:` addresses (`img-src https:`): the network, asked for by a person.
    pub remote_images: bool,
    /// The document's inline scripts (`script-src 'unsafe-inline'`); never a script from elsewhere, never a connection.
    pub scripts: bool,
}

/// The Content-Security-Policy of the page. By default: `default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:;
/// form-action 'none'; base-uri 'none'; sandbox` (no scripts, no connections, no frames, no forms; the sandbox also blocks popups and
/// navigating the view).
pub fn content_security_policy(allow: Allow) -> String {
    let images = if allow.remote_images { "img-src data: https:" } else { "img-src data:" };
    let mut policy = format!("default-src 'none'; style-src 'unsafe-inline'; {images}; font-src data:; form-action 'none'; base-uri 'none'");
    if allow.scripts {
        policy.push_str("; script-src 'unsafe-inline'; sandbox allow-scripts");
    } else {
        policy.push_str("; sandbox");
    }
    policy
}

/// The headers of every answer of the browser view (the content type aside).
pub fn response_headers(allow: Allow) -> Vec<(&'static str, String)> {
    vec![
        ("Content-Security-Policy", content_security_policy(allow)),
        ("X-Content-Type-Options", "nosniff".to_owned()),
        ("Referrer-Policy", "no-referrer".to_owned()),
        ("Cache-Control", "no-store".to_owned()),
        ("X-DNS-Prefetch-Control", "off".to_owned()),
    ]
}

/// An answer of the browser view's protocol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Served {
    pub status: u16,
    /// Every header, the content type included.
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

/// The answer to a request for `path` (the path of the URL, without query or fragment): the page at [`DOCUMENT_PATH`], else 404.
/// `page` is what is shown now; with none the document is an empty page.
pub fn serve(path: &str, page: Option<&Prepared>, allow: Allow) -> Served {
    let mut headers = response_headers(allow);
    if path != DOCUMENT_PATH {
        headers.push(("Content-Type", "text/plain; charset=utf-8".to_owned()));
        return Served { status: 404, headers, body: Vec::new() };
    }
    let (media_type, body) = page.map_or((HTML_MEDIA_TYPE, Vec::new()), |p| (p.media_type, p.body.as_bytes().to_vec()));
    headers.push(("Content-Type", media_type.to_owned()));
    Served { status: 200, headers, body }
}

/// Whether the browser view may load `url`: only the document itself (the first load, a reload, an anchor of it). A link elsewhere, a
/// form, a refresh, a redirect, `about:`, `file:`, `data:` and `javascript:` addresses are refused.
pub fn allows_navigation(url: &str) -> bool {
    let address = url.split_once('#').map_or(url, |(address, _)| address);
    address == DOCUMENT_URL || address == DOCUMENT_URL_WEBVIEW2
}

/// The page written for the system browser when there is no browser view (Linux, the web, a view that could not be made): the policy
/// as a `<meta http-equiv="Content-Security-Policy">` right after the doctype (a meta cannot carry the sandbox), and an SVG image as
/// an `<img>` of it (an image runs nothing and loads nothing). Only this copy is changed; the document is not.
pub fn fallback_page(page: &Prepared, allow: Allow) -> String {
    let policy = content_security_policy(allow).replace("; sandbox allow-scripts", "").replace("; sandbox", "");
    let meta = format!("<meta http-equiv=\"Content-Security-Policy\" content=\"{}\">", escape(&policy));
    if page.media_type == SVG_MEDIA_TYPE {
        let title = escape(page.title.as_deref().unwrap_or("SVG"));
        return format!(
            "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\">{meta}<title>{title}</title><style>body{{margin:0;padding:16px}}img{{max-width:100%}}</style></head>\n<body><img alt=\"{title}\" src=\"data:image/svg+xml;charset=utf-8,{}\"></body></html>\n",
            percent_encode(&page.body)
        );
    }
    let at = after_doctype(&page.body);
    let (head, rest) = page.body.split_at_checked(at).unwrap_or(("", &page.body));
    format!("{head}{meta}{rest}")
}

/// Where the doctype ends, after a byte order mark, white space and comments before it; 0 when there is none. A `<meta>` there is
/// put into the head by the HTML parser, and the page keeps its mode (a meta before the doctype would make it quirky).
fn after_doctype(text: &str) -> usize {
    let mut at = 0;
    loop {
        let rest = text.get(at..).unwrap_or_default();
        let trimmed = rest.trim_start_matches(['\u{feff}', ' ', '\t', '\n', '\r', '\u{c}']);
        let skipped = at + (rest.len() - trimmed.len());
        if trimmed.starts_with("<!--") {
            let Some(end) = trimmed.find("-->") else { return 0 };
            at = skipped + end + 3;
            continue;
        }
        if trimmed.get(..9).is_some_and(|t| t.eq_ignore_ascii_case("<!doctype")) {
            return trimmed.find('>').map_or(0, |end| skipped + end + 1);
        }
        return 0;
    }
}

/// `text` for a `data:` address: every byte that is not unreserved written as `%XX`.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_mul(3));
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `text` safe in HTML text and in quoted attribute values.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// At most `chars` characters of `text` on one line, with `…` when cut.
fn clip(text: &str, chars: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut shown: String = flat.chars().take(chars).collect();
    if flat.chars().count() > chars {
        shown.push('…');
    }
    shown
}

/// The text of an HTML document's `<title>`, if it has one that is not empty.
fn html_title(text: &str) -> Option<String> {
    let doc = xmlcraft_html::parse(text);
    let title = doc.elements.iter().find(|e| e.name == "title")?;
    let inner_end = title.end_tag.as_ref().map_or(title.span.end, |t| t.start);
    let inner = text.get(title.start_tag.end.min(inner_end)..inner_end)?;
    let shown = clip(&decode_references(inner), MAX_TITLE_CHARS);
    (!shown.is_empty()).then_some(shown)
}

/// The name of an element (as written, with its prefix).
fn tag_name(element: &SyntaxNode) -> Option<String> {
    let start = element.children().find(|n| n.kind() == SyntaxKind::StartTag)?;
    start.children_with_tokens().filter_map(|t| t.into_token()).find(|t| t.kind() == SyntaxKind::Name).map(|t| t.text().to_owned())
}

/// The attributes of an element as (name, value), the values with their references decoded.
fn attributes(element: &SyntaxNode) -> Vec<(String, String)> {
    let Some(start) = element.children().find(|n| n.kind() == SyntaxKind::StartTag) else { return Vec::new() };
    start
        .children()
        .filter(|n| n.kind() == SyntaxKind::Attribute)
        .filter_map(|attribute| {
            let name = attribute.children_with_tokens().filter_map(|t| t.into_token()).find(|t| t.kind() == SyntaxKind::Name)?.text().to_owned();
            let raw = attribute.children().find(|n| n.kind() == SyntaxKind::AttrValue).map(|v| v.text().to_string()).unwrap_or_default();
            let unquoted = raw.strip_prefix(['"', '\'']).map(|r| r.strip_suffix(['"', '\'']).unwrap_or(r)).unwrap_or(&raw);
            Some((name, decode_references(unquoted)))
        })
        .collect()
}

/// The local part of a name.
fn local(name: &str) -> &str {
    split_qname(name).map_or(name, |(_, local)| local)
}

/// The namespace an element's prefix (or its default namespace) is bound to on the element itself.
fn own_namespace(element: &SyntaxNode) -> Option<String> {
    let name = tag_name(element)?;
    let wanted = match split_qname(&name) {
        Some((Some(prefix), _)) => format!("xmlns:{prefix}"),
        _ => "xmlns".to_owned(),
    };
    attributes(element).into_iter().find(|(n, _)| *n == wanted).map(|(_, v)| v)
}

/// Whether the root element is an SVG image: `svg` in the SVG namespace.
fn is_svg(element: &SyntaxNode) -> bool {
    tag_name(element).is_some_and(|n| local(&n) == "svg") && own_namespace(element).as_deref() == Some(SVG_NAMESPACE)
}

/// The serialization declarations of a stylesheet: the `method` of its unnamed `xsl:output`, if it says one, and where every top-level
/// `xsl:output` is in the text.
struct OutputDeclarations {
    method: Option<String>,
    ranges: Vec<std::ops::Range<usize>>,
}

fn output_declarations(stylesheet: &str) -> OutputDeclarations {
    let mut found = OutputDeclarations { method: None, ranges: Vec::new() };
    let parsed = parse(stylesheet);
    let Some(root) = parsed.syntax().children().find(|n| n.kind() == SyntaxKind::Element) else { return found };
    let prefix_of_xslt = attributes(&root).into_iter().find_map(|(name, value)| {
        if value != XSLT_NAMESPACE {
            return None;
        }
        if name == "xmlns" { Some(String::new()) } else { name.strip_prefix("xmlns:").map(str::to_owned) }
    });
    let output_name = match prefix_of_xslt.as_deref() {
        Some("") => "output".to_owned(),
        Some(prefix) => format!("{prefix}:output"),
        None => "xsl:output".to_owned(),
    };
    for output in root.children().filter(|n| n.kind() == SyntaxKind::Element && tag_name(n).as_deref() == Some(output_name.as_str())) {
        found.ranges.push(usize::from(output.text_range().start())..usize::from(output.text_range().end()));
        let attributes = attributes(&output);
        if found.method.is_none() && !attributes.iter().any(|(name, _)| name == "name") {
            found.method = attributes.into_iter().find(|(name, _)| name == "method").map(|(_, v)| v.trim().to_owned());
        }
    }
    found
}

/// `text` without the `ranges` (in order, not overlapping).
fn without(text: &str, ranges: &[std::ops::Range<usize>]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for range in ranges {
        let start = range.start.clamp(at, text.len());
        out.push_str(text.get(at..start).unwrap_or_default());
        at = range.end.clamp(start, text.len());
    }
    out.push_str(text.get(at..).unwrap_or_default());
    out
}

/// The page a stylesheet's result makes, when the stylesheet writes HTML: its output method is `html` or `xhtml`; or it says none, or
/// `xml`, and the result's root is `html` (XHTML written as XML: the engine writes a result element without its namespace
/// declaration, so the namespace cannot be asked; `text` and other methods are not pages). The result is written as XML, so an
/// element that is not void in HTML and was written `<p/>` gets its end tag (`<p></p>`): the HTML parser would leave it open. A
/// result without a doctype gets `<!DOCTYPE html>` (what the html output method of XSLT 3.0 writes).
fn html_of_result(method: Option<&str>, result: &str) -> Option<String> {
    let parsed = parse(result);
    let document = parsed.syntax();
    let root = document.children().find(|n| n.kind() == SyntaxKind::Element);
    let root_name = root.as_ref().and_then(tag_name);
    let root_is_html = root_name.as_deref().is_some_and(|n| local(n).eq_ignore_ascii_case("html"));
    let html = match method.map(|m| local(m).to_ascii_lowercase()) {
        Some(m) if m == "html" || m == "xhtml" => true,
        None => root_is_html,
        Some(m) => m == "xml" && root_is_html,
    };
    if !html {
        return None;
    }
    let mut out = String::with_capacity(result.len() + 32);
    let has_doctype = document.children().any(|n| n.kind() == SyntaxKind::Doctype);
    if !has_doctype {
        out.push_str("<!DOCTYPE html>\n");
    }
    for element in document.descendants_with_tokens() {
        let Some(token) = element.into_token() else { continue };
        if token.kind() == SyntaxKind::SlashGt
            && let Some(name) = token.parent().and_then(|tag| tag.parent()).as_ref().and_then(tag_name)
            && !xmlcraft_html::data::is_void(&local(&name).to_ascii_lowercase())
        {
            out.push_str("></");
            out.push_str(&name);
            out.push('>');
            continue;
        }
        // (The XML declaration of a result means nothing to the HTML parser.)
        if token.parent().is_some_and(|p| p.kind() == SyntaxKind::XmlDecl) {
            continue;
        }
        out.push_str(token.text());
    }
    Some(out)
}

/// The tree of an XML document, as HTML.
#[derive(Default)]
struct Tree {
    out: String,
    nodes: usize,
    cut_nodes: bool,
    cut_depth: bool,
}

impl Tree {
    fn document(&mut self, root: &SyntaxNode) {
        for child in root.children() {
            match child.kind() {
                SyntaxKind::Element => self.element(&child, 0),
                SyntaxKind::Comment => self.comment(&child),
                SyntaxKind::ProcessingInstruction => self.instruction(&child),
                _ => {}
            }
        }
        if self.cut_nodes {
            self.out.push_str("<div class=\"cut\">…</div>");
        }
    }

    /// Count a node; false when the tree is full.
    fn room(&mut self) -> bool {
        if self.nodes >= MAX_TREE_NODES {
            self.cut_nodes = true;
            return false;
        }
        self.nodes += 1;
        true
    }

    fn comment(&mut self, node: &SyntaxNode) {
        if self.room() {
            self.out.push_str(&format!("<div class=\"m\">{}</div>", escape(&clip(&node.text().to_string(), MAX_TEXT_CHARS))));
        }
    }

    fn instruction(&mut self, node: &SyntaxNode) {
        if self.room() {
            self.out.push_str(&format!("<div class=\"m\">{}</div>", escape(&clip(&node.text().to_string(), MAX_TEXT_CHARS))));
        }
    }

    fn text(&mut self, text: &str) {
        let shown = clip(text, MAX_TEXT_CHARS);
        if !shown.is_empty() && self.room() {
            self.out.push_str(&format!("<div class=\"x\">{}</div>", escape(&shown)));
        }
    }

    fn start_tag(name: &str, element: &SyntaxNode, empty: bool) -> String {
        let mut tag = format!("<span class=\"p\">&lt;</span><span class=\"n\">{}</span>", escape(name));
        for (attribute, value) in attributes(element) {
            tag.push_str(&format!(
                " <span class=\"an\">{}</span><span class=\"p\">=</span><span class=\"av\">\"{}\"</span>",
                escape(&attribute),
                escape(&clip(&value, MAX_TEXT_CHARS))
            ));
        }
        tag.push_str(if empty { "<span class=\"p\">/&gt;</span>" } else { "<span class=\"p\">&gt;</span>" });
        tag
    }

    fn element(&mut self, node: &SyntaxNode, depth: usize) {
        if depth >= MAX_TREE_DEPTH {
            self.cut_depth = true;
            return;
        }
        if !self.room() {
            return;
        }
        let name = tag_name(node).unwrap_or_default();
        // What it holds: its text in one piece when it is only text, else its children one by one.
        let mut texts = String::new();
        let mut children = false;
        for child in node.children_with_tokens() {
            match child.kind() {
                SyntaxKind::Element | SyntaxKind::Comment | SyntaxKind::ProcessingInstruction => children = true,
                SyntaxKind::Text | SyntaxKind::CharRef | SyntaxKind::EntityRef | SyntaxKind::CData => texts.push_str(&content_text(&child)),
                _ => {}
            }
        }
        let end = format!("<span class=\"p\">&lt;/</span><span class=\"n\">{}</span><span class=\"p\">&gt;</span>", escape(&name));
        if !children {
            let shown = clip(&texts, MAX_TEXT_CHARS);
            if shown.is_empty() {
                self.out.push_str(&format!("<div class=\"l\">{}</div>", Self::start_tag(&name, node, true)));
            } else {
                self.out.push_str(&format!(
                    "<div class=\"l\">{}<span class=\"t\">{}</span>{end}</div>",
                    Self::start_tag(&name, node, false),
                    escape(&shown)
                ));
            }
            return;
        }
        self.out.push_str(&format!("<details open><summary>{}</summary><div class=\"k\">", Self::start_tag(&name, node, false)));
        let mut pending = String::new();
        for child in node.children_with_tokens() {
            match child.kind() {
                SyntaxKind::Text | SyntaxKind::CharRef | SyntaxKind::EntityRef | SyntaxKind::CData => pending.push_str(&content_text(&child)),
                SyntaxKind::Element | SyntaxKind::Comment | SyntaxKind::ProcessingInstruction => {
                    self.text(&std::mem::take(&mut pending));
                    let Some(child) = child.into_node() else { continue };
                    match child.kind() {
                        SyntaxKind::Element => self.element(&child, depth + 1),
                        SyntaxKind::Comment => self.comment(&child),
                        _ => self.instruction(&child),
                    }
                }
                _ => {}
            }
            if self.cut_nodes {
                break;
            }
        }
        self.text(&pending);
        self.out.push_str(&format!("</div><div class=\"e\">{end}</div></details>"));
    }
}

/// The text a piece of content stands for: references decoded, a CDATA section without its markers.
fn content_text(piece: &xmlcraft_syntax::SyntaxElement) -> String {
    match piece {
        rowan::NodeOrToken::Token(token) => match token.kind() {
            SyntaxKind::Text => token.text().to_owned(),
            _ => decode_references(token.text()),
        },
        rowan::NodeOrToken::Node(node) => node
            .children_with_tokens()
            .filter_map(|t| t.into_token())
            .filter(|t| t.kind() == SyntaxKind::CdataText)
            .map(|t| t.text().to_owned())
            .collect(),
    }
}

/// The page of a tree: our own markup and styles, no script.
fn tree_page(title: &str, tree: &str) -> String {
    const STYLE: &str = "\
:root{color-scheme:light dark;--fg:#1f2328;--muted:#6e7781;--name:#0550ae;--attr:#8250df;--value:#0a3069;--line:#d0d7de}\
@media (prefers-color-scheme:dark){:root{--fg:#e6edf3;--muted:#8d96a0;--name:#79c0ff;--attr:#d2a8ff;--value:#a5d6ff;--line:#30363d}}\
body{margin:0;padding:12px 16px;font:13px/1.5 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;color:var(--fg)}\
details>summary{cursor:default;list-style-position:outside;margin-left:1em}\
.k{margin-left:1.2em;padding-left:.6em;border-left:1px solid var(--line)}\
.l,.x,.m,.e,.cut{margin-left:1em}.n{color:var(--name)}.an{color:var(--attr)}.av{color:var(--value)}.p,.m,.cut{color:var(--muted)}\
.t,.x{white-space:pre-wrap;word-break:break-word}";
    format!(
        "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>{}</title><style>{STYLE}</style></head>\n<body>{tree}</body></html>\n",
        escape(title)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const XSL_HTML: &str = r#"<xsl:stylesheet version="3.0" xmlns:xsl="http://www.w3.org/1999/XSL/Transform"><xsl:output method="html"/><xsl:template match="/"><html><head><title>Books</title></head><body><ul><xsl:for-each select="//book"><li><xsl:value-of select="."/></li></xsl:for-each></ul><div/></body></html></xsl:template></xsl:stylesheet>"#;
    const BOOKS: &str = "<?xml-stylesheet type=\"text/xsl\" href=\"b.xsl\"?>\n<books><book>One</book><book>Two &amp; three</book></books>";

    fn tree(text: &str) -> Prepared {
        prepare(Input::Xml { text, stylesheet: None }).expect("prepares")
    }

    #[test]
    fn an_html_document_is_shown_as_it_is_with_its_title() {
        let page = "<!doctype html><html><head><title> A  &amp; B </title></head><body><p>x<script>alert(1)</script></body></html>";
        let p = prepare(Input::Html(page)).expect("prepares");
        assert_eq!((p.body.as_str(), p.media_type, p.shown), (page, HTML_MEDIA_TYPE, Shown::Html), "the markup is not rewritten");
        assert_eq!(p.title.as_deref(), Some("A & B"));
        assert!(p.warnings.is_empty());
        assert_eq!(prepare(Input::Html("")).expect("empty").title, None);
    }

    #[test]
    fn a_document_or_a_result_over_the_cap_is_refused() {
        let big = "x".repeat(MAX_BYTES + 1);
        assert_eq!(prepare(Input::Html(&big)), Err(PreviewError::TooLarge { bytes: MAX_BYTES + 1, limit: MAX_BYTES }));
        assert!(prepare(Input::Xml { text: &big, stylesheet: None }).is_err());
        assert!(prepare(Input::Xml { text: "<a/>", stylesheet: Some(&big) }).is_err());
        assert!(prepare(Input::Html(&"x".repeat(MAX_BYTES))).is_ok(), "the cap itself is allowed");
        assert!(PreviewError::TooLarge { bytes: MAX_BYTES + 1, limit: MAX_BYTES }.to_string().contains("8.0 MB"));
    }

    #[test]
    fn a_stylesheet_that_writes_html_gives_its_result_with_non_void_elements_closed() {
        let p = prepare(Input::Xml { text: BOOKS, stylesheet: Some(XSL_HTML) }).expect("prepares");
        assert_eq!((p.shown, p.media_type), (Shown::Transformed, HTML_MEDIA_TYPE), "{p:?}");
        assert!(p.body.starts_with("<!DOCTYPE html>\n<html>"), "{}", p.body);
        assert!(p.body.contains("<li>One</li><li>Two &amp; three</li>"), "{}", p.body);
        assert!(p.body.contains("<div></div>") && !p.body.contains("<div/>"), "{}", p.body);
        assert_eq!(p.title.as_deref(), Some("Books"));
    }

    #[test]
    fn void_elements_stay_empty_and_only_tags_are_touched() {
        let sheet = "<xsl:stylesheet version=\"3.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\"><xsl:template match=\"/\"><html><body><br/><p title=\"a/>b\"/><img src=\"x\"/></body></html></xsl:template></xsl:stylesheet>";
        let p = prepare(Input::Xml { text: "<r/>", stylesheet: Some(sheet) }).expect("prepares");
        assert_eq!(p.shown, Shown::Transformed, "no output method and an html root: HTML");
        assert!(p.body.contains("<br/>") && p.body.contains("<img src=\"x\"/>"), "{}", p.body);
        assert!(p.body.contains("<p title=\"a/&gt;b\"></p>") || p.body.contains("<p title=\"a/>b\"></p>"), "{}", p.body);
    }

    #[test]
    fn the_output_method_decides_whether_a_result_is_html() {
        let with = |method: &str, body: &str| {
            format!(
                "<xsl:stylesheet version=\"3.0\" xmlns:xsl=\"http://www.w3.org/1999/XSL/Transform\"><xsl:output method=\"{method}\"/><xsl:template match=\"/\">{body}</xsl:template></xsl:stylesheet>"
            )
        };
        let shown = |sheet: &str| prepare(Input::Xml { text: "<r/>", stylesheet: Some(sheet) }).expect("prepares").shown;
        assert_eq!(shown(&with("xhtml", "<page/>")), Shown::Transformed);
        assert_eq!(shown(&with("xml", "<html xmlns=\"http://www.w3.org/1999/xhtml\"/>")), Shown::Transformed, "XHTML written as XML is a page");
        assert_eq!(shown(&with("xml", "<page/>")), Shown::Tree, "other XML is not");
        assert_eq!(shown(&with("text", "<html/>")), Shown::Tree);
        let p = prepare(Input::Xml { text: "<r/>", stylesheet: Some(&with("xml", "<out/>")) }).expect("prepares");
        assert!(p.warnings.iter().any(|w| w.contains("does not write HTML")), "{:?}", p.warnings);
        // A named output is not the one the result is written with; another prefix for XSLT is still XSLT.
        let named = "<x:stylesheet version=\"3.0\" xmlns:x=\"http://www.w3.org/1999/XSL/Transform\"><x:output name=\"o\" method=\"xml\"/><x:output method=\"html\"/><x:template match=\"/\"><page/></x:template></x:stylesheet>";
        let declared = output_declarations(named);
        assert_eq!((declared.method.as_deref(), declared.ranges.len()), (Some("html"), 2));
        assert_eq!(
            without(named, &declared.ranges),
            "<x:stylesheet version=\"3.0\" xmlns:x=\"http://www.w3.org/1999/XSL/Transform\"><x:template match=\"/\"><page/></x:template></x:stylesheet>"
        );
        assert_eq!(shown(named), Shown::Transformed, "the engine runs the stylesheet without its declarations");
        assert_eq!(output_declarations("<nope").method, None);
        assert_eq!(without("abc", &[1..2, 5..9]), "ac", "ranges past the end take nothing");
    }

    #[test]
    fn a_stylesheet_that_fails_leaves_the_tree_and_says_why() {
        let p = prepare(Input::Xml { text: BOOKS, stylesheet: Some("<not-xslt/>") }).expect("prepares");
        assert_eq!(p.shown, Shown::Tree);
        assert!(p.warnings.first().is_some_and(|w| w.contains("could not be applied")), "{:?}", p.warnings);
    }

    #[test]
    fn plain_xml_is_an_escaped_collapsible_tree_without_script() {
        let p = tree(
            "<?xml version=\"1.0\"?>\n<!-- c -->\n<lib x=\"&lt;1&quot;\"><book id=\"1\">A &amp; <![CDATA[<b>]]></book><empty/><mixed>t<i>u</i>v</mixed></lib>",
        );
        assert_eq!((p.shown, p.title.as_deref()), (Shown::Tree, Some("lib")));
        assert!(p.warnings.is_empty(), "{:?}", p.warnings);
        let b = &p.body;
        assert!(b.starts_with("<!DOCTYPE html>") && b.contains("<title>lib</title>"));
        assert!(!b.contains("<script") && !b.contains("<b>"), "nothing of the document is markup: {b}");
        assert!(b.contains("<span class=\"av\">\"&lt;1&quot;\"</span>"), "attribute values are decoded and escaped: {b}");
        assert!(b.contains("<span class=\"t\">A &amp; &lt;b&gt;</span>"), "text and CDATA as text: {b}");
        assert!(b.contains("<span class=\"n\">empty</span><span class=\"p\">/&gt;</span>"));
        assert!(b.contains("<details open><summary>") && b.contains("<div class=\"x\">t</div>") && b.contains("<div class=\"x\">v</div>"));
        assert!(b.contains("<div class=\"m\">&lt;!-- c --&gt;</div>"));
    }

    #[test]
    fn broken_xml_still_gives_a_tree_and_a_warning() {
        let p = tree("<a><b>text</a>");
        assert_eq!(p.shown, Shown::Tree);
        assert!(p.warnings.iter().any(|w| w.contains("not well-formed")), "{:?}", p.warnings);
        assert!(p.body.contains("<span class=\"n\">b</span>"));
        let p = tree("");
        assert_eq!((p.shown, p.title), (Shown::Tree, None));
    }

    #[test]
    fn the_tree_is_bounded_in_nodes_and_depth() {
        let wide = format!("<r>{}</r>", "<i/>".repeat(MAX_TREE_NODES + 50));
        let p = tree(&wide);
        assert!(p.body.matches("<div class=\"l\">").count() < MAX_TREE_NODES, "cut");
        assert!(p.warnings.iter().any(|w| w.contains("first 20000 nodes")), "{:?}", p.warnings);
        let deep = format!("{}{}", "<d>".repeat(MAX_TREE_DEPTH + 20), "</d>".repeat(MAX_TREE_DEPTH + 20));
        let p = tree(&deep);
        assert!(p.warnings.iter().any(|w| w.contains("levels deep")), "{:?}", p.warnings);
        assert_eq!(p.body.matches("<details open>").count(), MAX_TREE_DEPTH, "one level per element shown");
    }

    #[test]
    fn an_svg_image_is_shown_as_svg_and_svg_in_another_namespace_is_not() {
        let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"><rect width=\"5\" height=\"5\"/></svg>";
        let p = tree(svg);
        assert_eq!((p.shown, p.media_type, p.body.as_str()), (Shown::Svg, SVG_MEDIA_TYPE, svg));
        assert_eq!(tree("<s:svg xmlns:s=\"http://www.w3.org/2000/svg\"/>").shown, Shown::Svg);
        assert_eq!(tree("<svg/>").shown, Shown::Tree);
        assert_eq!(tree("<svg xmlns=\"urn:x\"/>").shown, Shown::Tree);
    }

    #[test]
    fn a_stylesheet_is_named_only_in_the_prolog() {
        assert!(names_stylesheet(BOOKS));
        assert_eq!(stylesheet_href(BOOKS).as_deref(), Some("b.xsl"));
        assert!(names_stylesheet("<?xml version=\"1.0\"?><!-- x --><?xml-stylesheet href='a.xsl'?><r/>"));
        assert!(!names_stylesheet("<?xml-stylesheet type=\"text/css\" href=\"a.css\"?><r/>"), "CSS is not XSLT");
        assert!(!names_stylesheet("<r><?xml-stylesheet href=\"a.xsl\"?></r>"), "after the root it names nothing");
        assert!(!names_stylesheet("") && !names_stylesheet("<r/>"));
        let late = format!("{}<?xml-stylesheet href=\"a.xsl\"?><r/>", " ".repeat(PROLOG_BYTES));
        assert!(!names_stylesheet(&late), "past the look");
        assert!(!names_stylesheet(&format!("{}é<?xml-stylesheet href=\"a.xsl\"?>", " ".repeat(PROLOG_BYTES - 1))), "cut at a character");
    }

    #[test]
    fn the_policy_is_exactly_the_documented_one() {
        assert_eq!(
            content_security_policy(Allow::default()),
            "default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; form-action 'none'; base-uri 'none'; sandbox"
        );
        assert_eq!(
            content_security_policy(Allow { remote_images: true, scripts: false }),
            "default-src 'none'; style-src 'unsafe-inline'; img-src data: https:; font-src data:; form-action 'none'; base-uri 'none'; sandbox"
        );
        assert_eq!(
            content_security_policy(Allow { remote_images: false, scripts: true }),
            "default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; form-action 'none'; base-uri 'none'; script-src 'unsafe-inline'; sandbox allow-scripts"
        );
        let headers = response_headers(Allow::default());
        let names: Vec<&str> = headers.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["Content-Security-Policy", "X-Content-Type-Options", "Referrer-Policy", "Cache-Control", "X-DNS-Prefetch-Control"]);
        assert_eq!(headers[1].1, "nosniff");
    }

    #[test]
    fn only_the_document_is_served_and_with_the_policy() {
        let page = prepare(Input::Html("<p>hi")).expect("prepares");
        let served = serve(DOCUMENT_PATH, Some(&page), Allow::default());
        assert_eq!((served.status, served.body.as_slice()), (200, b"<p>hi".as_slice()));
        assert!(served.headers.contains(&("Content-Type", HTML_MEDIA_TYPE.to_owned())));
        assert!(served.headers.contains(&("Content-Security-Policy", content_security_policy(Allow::default()))));
        for path in ["/", "/document/", "/x.png", "/../etc/passwd", "", "/DOCUMENT"] {
            let other = serve(path, Some(&page), Allow::default());
            assert_eq!((other.status, other.body.len()), (404, 0), "{path}");
            assert!(other.headers.iter().any(|(n, _)| *n == "Content-Security-Policy"), "a refusal carries the policy too");
        }
        assert_eq!(serve(DOCUMENT_PATH, None, Allow::default()).status, 200, "nothing yet: an empty page");
    }

    #[test]
    fn only_the_document_and_its_anchors_may_be_navigated_to() {
        for url in
            [DOCUMENT_URL, "xmlcraft-preview://localhost/document#part-2", DOCUMENT_URL_WEBVIEW2, "https://xmlcraft-preview.localhost/document#x"]
        {
            assert!(allows_navigation(url), "{url}");
        }
        for url in [
            "https://example.com/",
            "http://xmlcraft-preview.localhost/document",
            "xmlcraft-preview://localhost/other.html",
            "xmlcraft-preview://localhost/document?x=1",
            "xmlcraft-preview://localhost/document/",
            "xmlcraft-preview://evil/document",
            "file:///etc/passwd",
            "about:blank",
            "data:text/html,<p>",
            "javascript:alert(1)",
            "",
        ] {
            assert!(!allows_navigation(url), "{url}");
        }
    }

    #[test]
    fn the_system_browser_copy_carries_the_policy_after_the_doctype() {
        let page =
            |body: &str| Prepared { body: body.to_owned(), media_type: HTML_MEDIA_TYPE, shown: Shown::Html, title: None, warnings: Vec::new() };
        let meta = "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src &#39;none&#39;; style-src &#39;unsafe-inline&#39;; img-src data:; font-src data:; form-action &#39;none&#39;; base-uri &#39;none&#39;\">";
        assert_eq!(fallback_page(&page("<!DOCTYPE html><p>x"), Allow::default()), format!("<!DOCTYPE html>{meta}<p>x"));
        assert_eq!(
            fallback_page(&page("\u{feff} <!-- a --><!doctype html>\n<p>"), Allow::default()),
            format!("\u{feff} <!-- a --><!doctype html>{meta}\n<p>")
        );
        assert_eq!(fallback_page(&page("<p>no doctype"), Allow::default()), format!("{meta}<p>no doctype"));
        assert_eq!(fallback_page(&page("<!-- open"), Allow::default()), format!("{meta}<!-- open"));
        assert!(fallback_page(&page(""), Allow { remote_images: false, scripts: true }).contains("script-src &#39;unsafe-inline&#39;\">"));
        let svg = Prepared {
            body: "<svg xmlns=\"http://www.w3.org/2000/svg\"><script>x</script></svg>".into(),
            media_type: SVG_MEDIA_TYPE,
            shown: Shown::Svg,
            title: None,
            warnings: Vec::new(),
        };
        let shown = fallback_page(&svg, Allow::default());
        assert!(shown.contains("<img alt=\"SVG\" src=\"data:image/svg+xml;charset=utf-8,%3Csvg%20xmlns%3D%22http%3A%2F%2F"), "{shown}");
        assert!(!shown.contains("<script>"), "the image's markup is only in the address");
    }
}
