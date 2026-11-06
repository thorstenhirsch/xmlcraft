//! Reading HTML: a tokenizer for tags, comments and raw text, and a stack that closes elements the way HTML does.

use std::collections::HashMap;
use std::ops::Range;

use xmlcraft_syntax::{Code, Diagnostic};

use crate::data::{self, element};

/// The most elements read; the rest of the text is left as it is.
pub const MAX_ELEMENTS: usize = 200_000;
/// Elements nested deeper than this are not opened.
pub const MAX_DEPTH: usize = 512;
/// The most problems listed.
const MAX_DIAGNOSTICS: usize = 1_000;
/// The most attributes read in one tag.
const MAX_ATTRIBUTES: usize = 1_000;

/// How an element ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Close {
    /// By its end tag.
    EndTag,
    /// A void element: it has no end tag.
    Void,
    /// By the start of another element that HTML takes to end it, or by the end tag of its parent (the end tag was left out, as HTML allows).
    Implied,
    /// Never: the text ended (or its parent's end tag came) with it still open, and HTML does not let its end tag be left out.
    Open,
}

/// An attribute of a start tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute {
    /// The name, in lower case (HTML names are case-insensitive).
    pub name: String,
    pub name_span: Range<usize>,
    /// The value with its quotes taken off (character references are not decoded); none for `<input disabled>`.
    pub value: Option<String>,
    /// Where the value is, quotes included.
    pub value_span: Option<Range<usize>>,
}

/// An element.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    /// The name in lower case (as written, inside `svg` and `math`, where case counts).
    pub name: String,
    pub depth: usize,
    pub parent: Option<usize>,
    pub name_span: Range<usize>,
    /// `<` to `>` of the start tag.
    pub start_tag: Range<usize>,
    /// `</` to `>` of the end tag, when there is one.
    pub end_tag: Option<Range<usize>>,
    pub end_name_span: Option<Range<usize>>,
    /// The whole element: from its start tag to its end tag, or to where it was closed.
    pub span: Range<usize>,
    pub attributes: Vec<Attribute>,
    pub close: Close,
    /// Written inside `svg` or `math`.
    pub foreign: bool,
}

impl Element {
    /// The value of the attribute called `name` (lower case), if it has a value.
    pub fn attribute(&self, name: &str) -> Option<&Attribute> {
        self.attributes.iter().find(|a| a.name == name)
    }
}

/// A text read as HTML.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    /// In the order of their start tags (a parent comes before its children).
    pub elements: Vec<Element>,
    pub diagnostics: Vec<Diagnostic>,
    /// The `<!DOCTYPE …>`, if there is one.
    pub doctype: Option<Range<usize>>,
    /// The comments, `<!--` to `-->`.
    pub comments: Vec<Range<usize>>,
    /// The reader stopped at a cap ([`MAX_ELEMENTS`], [`MAX_DEPTH`] or the number of problems).
    pub truncated: bool,
}

struct Reader<'a> {
    text: &'a str,
    pos: usize,
    doc: Document,
    /// The indices of the open elements, outermost first.
    stack: Vec<usize>,
    /// How many of the open elements are `svg` or `math` (their content is foreign).
    foreign: usize,
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic()
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}')
}

/// Whether a start tag of `incoming` ends the open element `open` (whose end tag HTML lets a person leave out).
fn closes(open: &str, incoming: &str) -> bool {
    const BLOCKS: &[&str] = &[
        "address",
        "article",
        "aside",
        "blockquote",
        "details",
        "dialog",
        "div",
        "dl",
        "fieldset",
        "figcaption",
        "figure",
        "footer",
        "form",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "header",
        "hgroup",
        "hr",
        "main",
        "menu",
        "nav",
        "ol",
        "p",
        "pre",
        "search",
        "section",
        "table",
        "ul",
    ];
    match open {
        "p" => {
            BLOCKS.contains(&incoming)
                || matches!(
                    incoming,
                    "li" | "dt"
                        | "dd"
                        | "tr"
                        | "td"
                        | "th"
                        | "tbody"
                        | "thead"
                        | "tfoot"
                        | "caption"
                        | "colgroup"
                        | "option"
                        | "optgroup"
                        | "rt"
                        | "rp"
                )
        }
        "li" => incoming == "li",
        "dt" | "dd" => matches!(incoming, "dt" | "dd"),
        "rt" | "rp" => matches!(incoming, "rt" | "rp"),
        "optgroup" => incoming == "optgroup",
        "option" => matches!(incoming, "option" | "optgroup"),
        "thead" => matches!(incoming, "tbody" | "tfoot"),
        "tbody" => matches!(incoming, "tbody" | "tfoot"),
        "tr" => matches!(incoming, "tr" | "tbody" | "tfoot" | "thead"),
        "td" | "th" => matches!(incoming, "td" | "th" | "tr" | "tbody" | "tfoot" | "thead"),
        "caption" => matches!(incoming, "colgroup" | "thead" | "tbody" | "tfoot" | "tr"),
        "colgroup" => matches!(incoming, "thead" | "tbody" | "tfoot" | "tr" | "caption"),
        "head" => {
            incoming == "body" || !matches!(incoming, "title" | "base" | "link" | "meta" | "style" | "script" | "noscript" | "template" | "head")
        }
        _ => false,
    }
}

impl<'a> Reader<'a> {
    fn report(&mut self, code: Code, span: Range<usize>, args: Vec<String>) {
        if self.doc.diagnostics.len() >= MAX_DIAGNOSTICS {
            self.doc.truncated = true;
            return;
        }
        self.doc.diagnostics.push(Diagnostic::new(code, span, args));
    }

    fn rest(&self) -> &'a str {
        self.text.get(self.pos..).unwrap_or_default()
    }

    /// Close the element at the top of the stack at `at`, in the way `close` says.
    fn pop(&mut self, close: Close, at: usize) {
        if let Some(index) = self.stack.pop()
            && let Some(e) = self.doc.elements.get_mut(index)
        {
            e.close = close;
            e.span.end = at.max(e.start_tag.end);
            if e.name == "svg" || e.name == "math" {
                self.foreign = self.foreign.saturating_sub(1);
            }
        }
    }

    fn top_name(&self) -> Option<&str> {
        self.stack.last().and_then(|i| self.doc.elements.get(*i)).map(|e| e.name.as_str())
    }

    fn run(&mut self) {
        while self.pos < self.text.len() {
            if self.doc.elements.len() >= MAX_ELEMENTS {
                self.doc.truncated = true;
                break;
            }
            let rest = self.rest();
            let Some(lt) = rest.find('<') else { break };
            self.pos += lt;
            let rest = self.rest();
            if rest.starts_with("<!--") {
                self.comment();
            } else if rest.starts_with("<!") || rest.starts_with("<?") {
                self.declaration();
            } else if rest.starts_with("</") && rest.get(2..).and_then(|r| r.chars().next()).is_some_and(is_name_start) {
                self.end_tag();
            } else if rest.get(1..).and_then(|r| r.chars().next()).is_some_and(is_name_start) {
                self.start_tag();
            } else {
                // A `<` that starts no tag is text.
                self.pos += 1;
            }
        }
        // What is still open at the end.
        let end = self.text.len();
        while let Some(top) = self.stack.last().copied() {
            let (name, span, optional) = match self.doc.elements.get(top) {
                Some(e) => (e.name.clone(), e.name_span.clone(), element(&e.name).is_some_and(|i| i.optional_end)),
                None => break,
            };
            if optional {
                self.pop(Close::Implied, end);
            } else {
                self.report(Code::HtmlUnclosedElement, span, vec![name]);
                self.pop(Close::Open, end);
            }
        }
    }

    fn comment(&mut self) {
        let start = self.pos;
        match self.rest().get(4..).and_then(|r| r.find("-->")) {
            Some(n) => {
                self.pos = start + 4 + n + 3;
                self.doc.comments.push(start..self.pos);
            }
            None => {
                self.report(Code::HtmlUnterminatedComment, start..start + 4, Vec::new());
                self.pos = self.text.len();
                self.doc.comments.push(start..self.pos);
            }
        }
    }

    /// `<!DOCTYPE …>`, `<![CDATA[…]]>` and `<?…>`: up to the next `>` (a bogus comment, but for the doctype).
    fn declaration(&mut self) {
        let start = self.pos;
        let end = self.rest().find('>').map_or(self.text.len(), |n| start + n + 1);
        let is_doctype = self.rest().get(..9).is_some_and(|p| p.eq_ignore_ascii_case("<!doctype"));
        if is_doctype && self.doc.doctype.is_none() {
            self.doc.doctype = Some(start..end);
        }
        self.pos = end;
    }

    fn end_tag(&mut self) {
        let start = self.pos;
        let after = start + 2;
        let name_len =
            self.text.get(after..).unwrap_or_default().find(|c: char| is_space(c) || c == '/' || c == '>').unwrap_or(self.text.len() - after);
        let written = self.text.get(after..after + name_len).unwrap_or_default();
        let name = if self.foreign > 0 { written.to_owned() } else { written.to_ascii_lowercase() };
        let end = self.text.get(after + name_len..).and_then(|r| r.find('>')).map_or(self.text.len(), |n| after + name_len + n + 1);
        if !self.text.get(..end).is_some_and(|t| t.ends_with('>')) {
            self.report(Code::HtmlUnfinishedTag, start..start + 2 + name_len, Vec::new());
        }
        self.pos = end;
        let name_span = after..after + name_len;
        if data::is_void(&name) && self.foreign == 0 {
            self.report(Code::HtmlVoidEndTag, name_span, vec![name]);
            return;
        }
        // The nearest open element of that name.
        let found = self.stack.iter().rposition(|i| self.doc.elements.get(*i).is_some_and(|e| e.name == name));
        let Some(depth) = found else {
            self.report(Code::HtmlStrayEndTag, start..end, vec![name]);
            return;
        };
        // What is open above it is closed: silently when HTML lets its end tag be left out, else it is a problem.
        while self.stack.len() > depth + 1 {
            let Some(top) = self.stack.last().copied() else { break };
            let (inner, span, optional) = match self.doc.elements.get(top) {
                Some(e) => (e.name.clone(), e.name_span.clone(), element(&e.name).is_some_and(|i| i.optional_end)),
                None => break,
            };
            if optional {
                self.pop(Close::Implied, start);
            } else {
                self.report(Code::HtmlUnclosedBefore, span, vec![inner, name.clone()]);
                self.pop(Close::Open, start);
            }
        }
        if let Some(index) = self.stack.last().copied()
            && let Some(e) = self.doc.elements.get_mut(index)
        {
            e.end_tag = Some(start..end);
            e.end_name_span = Some(name_span);
        }
        self.pop(Close::EndTag, end);
    }

    fn start_tag(&mut self) {
        let start = self.pos;
        let after = start + 1;
        let tail = self.text.get(after..).unwrap_or_default();
        let name_len = tail.find(|c: char| is_space(c) || c == '/' || c == '>').unwrap_or(tail.len());
        let written = tail.get(..name_len).unwrap_or_default();
        let foreign_here = self.foreign > 0 || matches!(written.to_ascii_lowercase().as_str(), "svg" | "math");
        let name = if self.foreign > 0 { written.to_owned() } else { written.to_ascii_lowercase() };
        let name_span = after..after + name_len;
        self.pos = after + name_len;
        let (attributes, self_closing, finished) = self.attributes();
        let end = self.pos;
        if !finished {
            self.report(Code::HtmlUnfinishedTag, start..(name_span.end), Vec::new());
        }
        // The start of this element ends the open ones that HTML says it ends.
        if self.foreign == 0 {
            while let Some(top) = self.top_name().map(str::to_owned) {
                if element(&top).is_some_and(|i| i.optional_end) && closes(&top, &name) {
                    self.pop(Close::Implied, start);
                } else {
                    break;
                }
            }
        }
        let info = if self.foreign > 0 { None } else { element(&name) };
        if self.foreign == 0 {
            match info {
                None if !data::is_custom_element(&name) => self.report(Code::HtmlUnknownElement, name_span.clone(), vec![name.clone()]),
                Some(i) if i.obsolete => self.report(Code::HtmlObsoleteElement, name_span.clone(), vec![name.clone()]),
                _ => {}
            }
            if name == "img" && !attributes.iter().any(|a| a.name == "alt") {
                self.report(Code::HtmlMissingAlt, name_span.clone(), Vec::new());
            }
        }
        let mut seen: HashMap<&str, ()> = HashMap::new();
        for a in &attributes {
            if seen.insert(a.name.as_str(), ()).is_some() {
                self.report(Code::HtmlDuplicateAttribute, a.name_span.clone(), vec![a.name.clone()]);
            }
        }
        let depth = self.stack.len();
        if depth >= MAX_DEPTH {
            self.report(Code::HtmlTooDeep, name_span, vec![MAX_DEPTH.to_string()]);
            self.doc.truncated = true;
            self.pos = self.text.len();
            return;
        }
        let void = info.is_some_and(|i| i.void) || (self.foreign > 0 && self_closing);
        let self_closing_ignored = self_closing && !void && self.foreign == 0 && !foreign_here;
        if self_closing_ignored {
            self.report(Code::HtmlSelfClosing, name_span.clone(), vec![name.clone()]);
        }
        let parent = self.stack.last().copied();
        let index = self.doc.elements.len();
        self.doc.elements.push(Element {
            name: name.clone(),
            depth,
            parent,
            name_span,
            start_tag: start..end,
            end_tag: None,
            end_name_span: None,
            span: start..end,
            attributes,
            close: Close::Open,
            foreign: self.foreign > 0,
        });
        if void || (foreign_here && self_closing) {
            if let Some(e) = self.doc.elements.get_mut(index) {
                e.close = if void { Close::Void } else { Close::EndTag };
            }
            return;
        }
        self.stack.push(index);
        // (`pop` takes one off again when this element ends.)
        if matches!(name.as_str(), "svg" | "math") {
            self.foreign += 1;
        }
        // Raw text: up to the end tag of the same name.
        if self.foreign == 0 && info.is_some_and(|i| i.raw_text) {
            self.raw_text(&name);
        }
    }

    /// The text of `script`, `style`, `textarea` and the like is not markup: skip to its end tag.
    fn raw_text(&mut self, name: &str) {
        let lower = self.rest().to_ascii_lowercase();
        let needle = format!("</{name}");
        let mut from = 0;
        while let Some(n) = lower.get(from..).and_then(|r| r.find(&needle)) {
            let at = from + n;
            let next = lower.get(at + needle.len()..).and_then(|r| r.chars().next());
            if next.is_none_or(|c| is_space(c) || c == '/' || c == '>') {
                self.pos += at;
                return;
            }
            from = at + needle.len();
        }
        // No end tag: the rest of the text is its content.
        self.pos = self.text.len();
    }

    /// The attributes of the start tag whose name was just read; the position ends after its `>`. Returns them, whether the tag ends in
    /// `/>`, and whether it was finished (found its `>`).
    fn attributes(&mut self) -> (Vec<Attribute>, bool, bool) {
        let mut out: Vec<Attribute> = Vec::new();
        let mut self_closing = false;
        loop {
            // White space and slashes between attributes.
            let rest = self.rest();
            let skipped = rest.find(|c: char| !is_space(c) && c != '/').unwrap_or(rest.len());
            let slash_before_end = rest.get(..skipped).is_some_and(|s| s.contains('/')) && rest.get(skipped..).is_some_and(|r| r.starts_with('>'));
            self.pos += skipped;
            let Some(c) = self.rest().chars().next() else { return (out, false, false) };
            if c == '>' {
                self.pos += 1;
                self_closing = slash_before_end;
                return (out, self_closing, true);
            }
            if out.len() >= MAX_ATTRIBUTES {
                // Skip to the end of the tag.
                let end = self.rest().find('>').map_or(self.text.len(), |n| self.pos + n + 1);
                self.pos = end;
                return (out, self_closing, true);
            }
            let name_start = self.pos;
            // (A name may start with `=`, as the standard has it.)
            let first = c.len_utf8();
            let rest = self.rest();
            let name_len =
                rest.get(first..).and_then(|r| r.find(|ch: char| is_space(ch) || matches!(ch, '/' | '>' | '='))).map_or(rest.len(), |n| first + n);
            let name = rest.get(..name_len).unwrap_or_default().to_ascii_lowercase();
            self.pos += name_len;
            let name_span = name_start..self.pos;
            // `=` and a value.
            let after_name = self.rest();
            let spaces = after_name.find(|ch: char| !is_space(ch)).unwrap_or(after_name.len());
            let (mut value, mut value_span) = (None, None);
            if after_name.get(spaces..).is_some_and(|r| r.starts_with('=')) {
                self.pos += spaces + 1;
                let rest = self.rest();
                let spaces = rest.find(|ch: char| !is_space(ch)).unwrap_or(rest.len());
                self.pos += spaces;
                let rest = self.rest();
                let vstart = self.pos;
                match rest.chars().next() {
                    Some(q @ ('"' | '\'')) => {
                        let inner = rest.get(1..).unwrap_or_default();
                        match inner.find(q) {
                            Some(n) => {
                                value = inner.get(..n).map(str::to_owned);
                                self.pos = vstart + 1 + n + 1;
                            }
                            None => {
                                // Not closed: the rest of the text.
                                value = Some(inner.to_owned());
                                self.pos = self.text.len();
                            }
                        }
                        value_span = Some(vstart..self.pos);
                    }
                    Some('>') | None => {
                        value = Some(String::new());
                        value_span = Some(vstart..vstart);
                    }
                    Some(_) => {
                        let n = rest.find(|ch: char| is_space(ch) || ch == '>').unwrap_or(rest.len());
                        value = rest.get(..n).map(str::to_owned);
                        self.pos = vstart + n;
                        value_span = Some(vstart..self.pos);
                    }
                }
            }
            out.push(Attribute { name, name_span, value, value_span });
        }
    }
}

/// Read `text` as HTML.
pub fn parse(text: &str) -> Document {
    let mut reader = Reader { text, pos: 0, doc: Document::default(), stack: Vec::new(), foreign: 0 };
    // A byte-order mark is not part of the markup.
    if text.starts_with('\u{FEFF}') {
        reader.pos = '\u{FEFF}'.len_utf8();
    }
    reader.run();
    let mut doc = reader.doc;
    // Ids that are used twice.
    let mut ids: HashMap<String, usize> = HashMap::new();
    let mut found: Vec<(Range<usize>, String)> = Vec::new();
    for e in &doc.elements {
        if let Some(a) = e.attribute("id")
            && let Some(v) = a.value.as_deref().filter(|v| !v.is_empty())
            && ids.insert(v.to_owned(), 1).is_some()
        {
            found.push((a.value_span.clone().unwrap_or_else(|| a.name_span.clone()), v.to_owned()));
        }
    }
    for (span, id) in found {
        if doc.diagnostics.len() < MAX_DIAGNOSTICS {
            doc.diagnostics.push(Diagnostic::new(Code::HtmlDuplicateId, span, vec![id]));
        }
    }
    // A whole document names its doctype; a fragment (no `html` element) does not need to.
    if doc.doctype.is_none()
        && let Some(html) = doc.elements.iter().find(|e| e.name == "html")
    {
        let span = html.name_span.clone();
        doc.diagnostics.push(Diagnostic::new(Code::HtmlMissingDoctype, span, Vec::new()));
    }
    doc.diagnostics.sort_by_key(|d| (d.span.start, d.span.end));
    doc
}
