//! The parser: tokens to a lossless syntax tree, plus the well-formedness and namespace diagnostics.
//!
//! It never fails. Every token goes into the tree exactly once and in order, so the tree's text is the input, byte for byte; broken
//! markup is recovered from (an unclosed element is closed when its parent's end tag shows up, a stray end tag is kept in place) and
//! each recovery is reported as a [`Diagnostic`].
//!
//! It uses no recursion: open elements are a stack, so a document nested a million levels deep costs memory, not the call stack. Past
//! [`Limits::max_depth`] elements stop nesting in the tree (their tags and content become siblings) but are still matched and checked.

use std::collections::{HashMap, HashSet, VecDeque};
use std::ops::Range;

use rowan::{GreenNode, GreenNodeBuilder};

use crate::chars::{is_name_start_char, is_xml_char, is_xml_space};
use crate::diagnostic::{Code, Diagnostic, Severity};
use crate::kinds::{SyntaxKind, SyntaxNode};
use crate::lexer::{Lexer, Token, TokenKind};
use crate::ns::{NsScope, XML_NAMESPACE, XMLNS_NAMESPACE, is_ncname, split_qname};

/// Caps that keep a hostile or huge document from costing more than it is worth.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Larger documents are not parsed (the tree's offsets are 32-bit, and past this size the editor uses large-file mode).
    pub max_bytes: usize,
    /// Elements nested deeper than this are not nested in the tree.
    pub max_depth: usize,
    /// Diagnostics beyond this many are dropped, with one note saying so.
    pub max_diagnostics: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self { max_bytes: 256 * 1024 * 1024, max_depth: 1024, max_diagnostics: 10_000 }
    }
}

/// The result of parsing: the tree and what is wrong with the document.
#[derive(Clone, Debug)]
pub struct Parse {
    green: GreenNode,
    diagnostics: Vec<Diagnostic>,
}

impl Parse {
    /// The root node ([`SyntaxKind::Document`]). Its text is the parsed text, unless the text was over [`Limits::max_bytes`].
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.severity() == Severity::Error)
    }
}

/// Parse `text` with the default [`Limits`].
pub fn parse(text: &str) -> Parse {
    parse_with(text, &Limits::default())
}

pub fn parse_with(text: &str, limits: &Limits) -> Parse {
    if text.len() > limits.max_bytes {
        let mut tree = Tree::new();
        tree.start(SyntaxKind::Document);
        return Parse { green: tree.into_green(), diagnostics: vec![Diagnostic::new(Code::TooLarge, 0..0, Vec::new())] };
    }
    let mut parser = Parser::new(text, limits);
    parser.run();
    parser.finish()
}

/// `GreenNodeBuilder` panics on unbalanced calls; this keeps count so that cannot happen.
struct Tree {
    inner: GreenNodeBuilder<'static>,
    depth: usize,
}

impl Tree {
    fn new() -> Self {
        Self { inner: GreenNodeBuilder::new(), depth: 0 }
    }

    fn start(&mut self, kind: SyntaxKind) {
        self.inner.start_node(rowan::SyntaxKind(kind as u16));
        self.depth += 1;
    }

    fn finish(&mut self) {
        if self.depth > 0 {
            self.inner.finish_node();
            self.depth -= 1;
        }
    }

    fn token(&mut self, kind: SyntaxKind, text: &str) {
        self.inner.token(rowan::SyntaxKind(kind as u16), text);
    }

    fn into_green(mut self) -> GreenNode {
        while self.depth > 0 {
            self.finish();
        }
        self.inner.finish()
    }
}

/// An element whose end tag has not come yet.
struct Open {
    /// The name in its start tag (or, if it has none, the `<`).
    name: Range<usize>,
    /// Whether the tree has an `Element` node open for it.
    nested: bool,
}

/// An attribute of a start tag, as far as the checks need it.
struct Attr {
    name: Range<usize>,
    /// The value between the quotes.
    value: Option<Range<usize>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TagEnd {
    Closed,
    SelfClosed,
    Unterminated,
}

struct Parser<'a> {
    src: &'a str,
    lexer: Lexer<'a>,
    ahead: VecDeque<Token>,
    tree: Tree,
    diags: Vec<Diagnostic>,
    limits: &'a Limits,
    open: Vec<Open>,
    /// How many open elements have each name, so an end tag that matches none is rejected without searching the stack.
    open_names: HashMap<&'a str, usize>,
    ns: NsScope,
    seen_root: bool,
    seen_doctype: bool,
    reported_deep: bool,
    overflowed: bool,
    /// Tokens consumed so far, to notice a step that made no progress.
    consumed: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str, limits: &'a Limits) -> Self {
        Self {
            src,
            lexer: Lexer::new(src),
            ahead: VecDeque::new(),
            tree: Tree::new(),
            diags: Vec::new(),
            limits,
            open: Vec::new(),
            open_names: HashMap::new(),
            ns: NsScope::new(),
            seen_root: false,
            seen_doctype: false,
            reported_deep: false,
            overflowed: false,
            consumed: 0,
        }
    }

    // --- the token stream ----------------------------------------------------------------------------------------------------

    fn fill(&mut self, n: usize) {
        while self.ahead.len() <= n {
            match self.lexer.next() {
                Some(token) => self.ahead.push_back(token),
                None => break,
            }
        }
    }

    fn peek(&mut self) -> Option<Token> {
        self.peek_nth(0)
    }

    fn peek_nth(&mut self, n: usize) -> Option<Token> {
        self.fill(n);
        self.ahead.get(n).copied()
    }

    fn kind(&mut self) -> Option<TokenKind> {
        self.peek().map(|t| t.kind)
    }

    /// Take the next token into the tree.
    fn bump(&mut self) -> Option<Token> {
        self.fill(0);
        let token = self.ahead.pop_front()?;
        self.tree.token(token.kind.into(), self.src.get(token.range()).unwrap_or_default());
        self.consumed += 1;
        Some(token)
    }

    fn skip_whitespace(&mut self) {
        while self.kind() == Some(TokenKind::Whitespace) {
            self.bump();
        }
    }

    /// Whether the first thing after the whitespace that starts at the next token is `=`. It reads the text rather than looking ahead
    /// in the tokens, so any amount of whitespace costs neither a limit nor memory.
    fn next_is_eq(&mut self) -> bool {
        let Some(token) = self.peek() else { return false };
        self.src.get(token.start..).is_some_and(|rest| rest.trim_start_matches(is_xml_space).starts_with('='))
    }

    fn slice(&self, range: &Range<usize>) -> &'a str {
        self.src.get(range.clone()).unwrap_or_default()
    }

    // --- diagnostics ---------------------------------------------------------------------------------------------------------

    fn diag(&mut self, code: Code, span: Range<usize>, args: Vec<String>) {
        if self.diags.len() < self.limits.max_diagnostics {
            self.diags.push(Diagnostic::new(code, span, args));
        } else if !self.overflowed {
            self.overflowed = true;
            self.diags.push(Diagnostic::new(Code::TooManyDiagnostics, span, Vec::new()));
        }
    }

    /// Text outside the root element is one problem however many tokens it spans.
    fn outside_root(&mut self, span: Range<usize>) {
        if let Some(last) = self.diags.last_mut()
            && last.code == Code::ContentOutsideRoot
            && last.span.end == span.start
        {
            last.span.end = span.end;
            return;
        }
        self.diag(Code::ContentOutsideRoot, span, Vec::new());
    }

    /// Report the first character in `range` that XML does not allow.
    fn check_chars(&mut self, range: Range<usize>) {
        let found = self.slice(&range).char_indices().find(|(_, c)| !is_xml_char(*c));
        if let Some((i, c)) = found {
            let at = range.start + i;
            self.diag(Code::InvalidChar, at..at + c.len_utf8(), vec![format!("{:04X}", u32::from(c))]);
        }
    }

    // --- the document --------------------------------------------------------------------------------------------------------

    fn run(&mut self) {
        self.tree.start(SyntaxKind::Document);
        while let Some(token) = self.peek() {
            let before = self.consumed;
            match token.kind {
                TokenKind::TagOpen => self.start_tag(),
                TokenKind::EndTagOpen => self.end_tag(),
                TokenKind::Text => self.text(token),
                TokenKind::CharRef | TokenKind::EntityRef => self.content_reference(token),
                TokenKind::BareAmp => {
                    self.diag(Code::BareAmpersand, token.range(), Vec::new());
                    self.bump();
                }
                TokenKind::BareLt => {
                    self.diag(Code::BareLessThan, token.range(), Vec::new());
                    self.bump();
                }
                TokenKind::CommentStart => self.comment(),
                TokenKind::CdataStart => self.cdata(),
                TokenKind::PiStart => self.pi(false),
                TokenKind::DoctypeStart => self.doctype(),
                _ => {
                    let text = self.slice(&token.range()).to_owned();
                    self.diag(Code::UnexpectedCharacter, token.range(), vec![text]);
                    self.bump();
                }
            }
            if self.consumed == before {
                // Every handler takes at least its first token; this keeps a mistake from becoming a hang.
                self.bump();
            }
        }
        while !self.open.is_empty() {
            self.close_unclosed();
        }
        if !self.seen_root {
            self.diag(Code::NoRootElement, 0..0, Vec::new());
        }
    }

    fn finish(self) -> Parse {
        Parse { green: self.tree.into_green(), diagnostics: self.diags }
    }

    // --- character data ------------------------------------------------------------------------------------------------------

    fn text(&mut self, token: Token) {
        let text = self.slice(&token.range());
        if self.open.is_empty() {
            // A byte-order mark that survived decoding is allowed in front of the whitespace before the root.
            let text = if token.start == 0 { text.strip_prefix('\u{FEFF}').unwrap_or(text) } else { text };
            if !text.chars().all(is_xml_space) {
                self.outside_root(token.range());
            }
        }
        self.check_chars(token.range());
        if let Some(i) = text.find("]]>") {
            self.diag(Code::CdataEndInContent, token.start + i..token.start + i + 3, Vec::new());
        }
        self.bump();
    }

    fn content_reference(&mut self, token: Token) {
        if self.open.is_empty() {
            self.outside_root(token.range());
        }
        self.check_reference(token);
        self.bump();
    }

    /// Check a character or entity reference (without taking it).
    fn check_reference(&mut self, token: Token) {
        let text = self.slice(&token.range());
        match token.kind {
            TokenKind::CharRef => {
                if char_ref_value(text).is_none() {
                    self.diag(Code::InvalidCharRef, token.range(), vec![text.to_owned()]);
                }
            }
            TokenKind::EntityRef => {
                let name = text.trim_start_matches('&').trim_end_matches(';');
                // With a DOCTYPE the entity may be declared there or in a DTD this checker has not read yet; that is the DTD's job.
                if !self.seen_doctype && !matches!(name, "lt" | "gt" | "amp" | "quot" | "apos") {
                    self.diag(Code::UndeclaredEntity, token.range(), vec![name.to_owned()]);
                }
            }
            _ => {}
        }
    }

    // --- elements ------------------------------------------------------------------------------------------------------------

    fn start_tag(&mut self) {
        let Some(lt) = self.peek() else { return };
        if self.open.is_empty() {
            if self.seen_root {
                self.diag(Code::MultipleRoots, lt.range(), Vec::new());
            }
            self.seen_root = true;
        }
        let nested = self.open.len() < self.limits.max_depth;
        if !nested && !self.reported_deep {
            self.reported_deep = true;
            self.diag(Code::NestingTooDeep, lt.range(), vec![self.limits.max_depth.to_string()]);
        }
        if nested {
            self.tree.start(SyntaxKind::Element);
        }
        self.tree.start(SyntaxKind::StartTag);
        self.bump();
        let name = match self.peek() {
            Some(t) if t.kind == TokenKind::Name => {
                self.bump();
                Some(t.range())
            }
            _ => None,
        };
        let mut attrs: Vec<Attr> = Vec::new();
        let mut end = TagEnd::Unterminated;
        while let Some(t) = self.peek() {
            match t.kind {
                TokenKind::Whitespace => {
                    self.bump();
                }
                TokenKind::Name => self.attribute(&mut attrs),
                TokenKind::Gt => {
                    self.bump();
                    end = TagEnd::Closed;
                    break;
                }
                TokenKind::SlashGt => {
                    self.bump();
                    end = TagEnd::SelfClosed;
                    break;
                }
                TokenKind::Quote => {
                    self.diag(Code::MissingAttrName, t.range(), Vec::new());
                    self.quoted_value();
                }
                TokenKind::Eq | TokenKind::Error => {
                    let text = self.slice(&t.range()).to_owned();
                    self.diag(Code::UnexpectedCharacter, t.range(), vec![text]);
                    self.bump();
                }
                _ => break, // a tag left open: content begins
            }
        }
        if end == TagEnd::Unterminated {
            self.diag(Code::UnterminatedTag, name.clone().unwrap_or_else(|| lt.range()), Vec::new());
        }
        self.tree.finish();
        self.names_and_namespaces(name.as_ref(), &attrs);
        if end == TagEnd::SelfClosed {
            self.ns.pop_scope();
            if nested {
                self.tree.finish();
            }
        } else {
            let name = name.unwrap_or_else(|| lt.range());
            *self.open_names.entry(self.slice(&name)).or_insert(0) += 1;
            self.open.push(Open { name, nested });
        }
    }

    fn attribute(&mut self, attrs: &mut Vec<Attr>) {
        self.tree.start(SyntaxKind::Attribute);
        let Some(name) = self.bump() else {
            self.tree.finish();
            return;
        };
        let name_text = self.slice(&name.range());
        let mut value = None;
        if self.next_is_eq() {
            self.skip_whitespace();
            self.bump(); // =
            self.skip_whitespace();
            match self.peek() {
                Some(t) if t.kind == TokenKind::Quote => value = self.quoted_value(),
                Some(t) if t.kind == TokenKind::Name => {
                    self.diag(Code::UnquotedAttrValue, t.range(), Vec::new());
                    self.tree.start(SyntaxKind::AttrValue);
                    self.bump();
                    self.tree.finish();
                    value = Some(t.range());
                }
                _ => self.diag(Code::MissingAttrValue, name.range(), vec![name_text.to_owned()]),
            }
        } else {
            self.diag(Code::AttrWithoutValue, name.range(), vec![name_text.to_owned()]);
        }
        self.tree.finish();
        attrs.push(Attr { name: name.range(), value });
    }

    /// A quoted attribute value, quotes included, as an `AttrValue` node. Returns the range between the quotes.
    fn quoted_value(&mut self) -> Option<Range<usize>> {
        self.tree.start(SyntaxKind::AttrValue);
        let Some(open) = self.bump() else {
            self.tree.finish();
            return None;
        };
        let quote = self.slice(&open.range()).to_owned();
        let (start, mut end, mut closed) = (open.end, open.end, false);
        while let Some(t) = self.peek() {
            match t.kind {
                TokenKind::AttrText => {
                    self.check_chars(t.range());
                    self.bump();
                    end = t.end;
                }
                TokenKind::CharRef | TokenKind::EntityRef => {
                    self.check_reference(t);
                    self.bump();
                    end = t.end;
                }
                TokenKind::BareAmp => {
                    self.diag(Code::BareAmpersand, t.range(), Vec::new());
                    self.bump();
                    end = t.end;
                }
                TokenKind::Quote => {
                    end = t.start;
                    self.bump();
                    closed = true;
                    break;
                }
                _ => break,
            }
        }
        if !closed {
            self.diag(Code::UnterminatedAttrValue, open.range(), vec![quote]);
        }
        self.tree.finish();
        Some(start..end)
    }

    fn end_tag(&mut self) {
        let Some(lt) = self.peek() else { return };
        let name_token = self.peek_nth(1).filter(|t| t.kind == TokenKind::Name);
        let name = name_token.map(|t| self.slice(&t.range()));
        // The end tag closes the nearest open element with its name; if none has it, it is taken as the end tag of the innermost
        // one (a typo), which reads better than leaving everything open.
        let top = self.open.len().checked_sub(1);
        let target = match name {
            // The search only runs when the name is open somewhere, and what it passes over is then closed: the cost is shared out.
            Some(n) if self.open_names.contains_key(n) => self.open.iter().rposition(|o| self.slice(&o.name) == n).or(top),
            Some(_) => top,
            None => top,
        };
        let name_span = name_token.map_or_else(|| lt.range(), |t| lt.start..t.end);
        match (name, target) {
            (None, _) => self.diag(Code::MissingEndTagName, lt.range(), Vec::new()),
            (Some(n), None) => self.diag(Code::UnmatchedEndTag, name_span, vec![n.to_owned()]),
            (Some(n), Some(j)) => {
                let expected = self.open.get(j).map(|o| self.slice(&o.name)).unwrap_or_default();
                if expected != n {
                    self.diag(Code::MismatchedEndTag, name_span, vec![expected.to_owned(), n.to_owned()]);
                }
            }
        }
        if let Some(j) = target {
            while self.open.len() > j + 1 {
                self.close_unclosed();
            }
        }
        self.tree.start(SyntaxKind::EndTag);
        self.bump(); // </
        if name_token.is_some() {
            self.bump();
        }
        let mut closed = false;
        let mut reported_junk = false;
        while let Some(t) = self.peek() {
            match t.kind {
                TokenKind::Whitespace => {
                    self.bump();
                }
                TokenKind::Gt | TokenKind::SlashGt => {
                    if t.kind == TokenKind::SlashGt {
                        self.diag(Code::UnexpectedCharacter, t.range(), vec!["/".to_owned()]);
                    }
                    self.bump();
                    closed = true;
                    break;
                }
                TokenKind::Name | TokenKind::Eq | TokenKind::Error | TokenKind::Quote => {
                    if !reported_junk {
                        reported_junk = true;
                        let text = self.slice(&t.range()).to_owned();
                        self.diag(Code::UnexpectedCharacter, t.range(), vec![text]);
                    }
                    if t.kind == TokenKind::Quote {
                        self.quoted_value();
                    } else {
                        self.bump();
                    }
                }
                _ => break,
            }
        }
        if !closed {
            self.diag(Code::UnterminatedEndTag, lt.range(), Vec::new());
        }
        self.tree.finish();
        if target.is_some() {
            self.pop_open();
        }
    }

    /// Close the innermost open element without an end tag of its own.
    fn close_unclosed(&mut self) {
        if let Some(open) = self.open.last() {
            let name = self.slice(&open.name).to_owned();
            let span = open.name.clone();
            self.diag(Code::UnclosedElement, span, vec![name]);
        }
        self.pop_open();
    }

    fn pop_open(&mut self) {
        if let Some(open) = self.open.pop() {
            let name = self.slice(&open.name);
            if let Some(count) = self.open_names.get_mut(name) {
                *count -= 1; // at least 1: every open element was counted when it opened
                if *count == 0 {
                    self.open_names.remove(name);
                }
            }
            if open.nested {
                self.tree.finish();
            }
            self.ns.pop_scope();
        }
    }

    // --- names and namespaces ------------------------------------------------------------------------------------------------

    /// Check the names of a start tag and open its namespace scope (which closes with the element).
    fn names_and_namespaces(&mut self, element: Option<&Range<usize>>, attrs: &[Attr]) {
        self.ns.push_scope();
        self.find_duplicate_attributes(attrs);
        // Declarations first: they apply to the element's own name and attributes.
        for attr in attrs {
            let name = self.slice(&attr.name);
            if name == "xmlns" {
                self.declare(None, attr);
            } else if let Some(prefix) = name.strip_prefix("xmlns:") {
                self.declare(Some(prefix), attr);
            }
        }
        if let Some(range) = element
            && let Some((Some(prefix), _)) = self.qname(range)
        {
            self.check_prefix(prefix, range);
        }
        let mut expanded: HashMap<(String, &'a str), &'a str> = HashMap::new();
        for attr in attrs {
            let name = self.slice(&attr.name);
            if name == "xmlns" || name.starts_with("xmlns:") {
                continue;
            }
            if let Some((Some(prefix), local)) = self.qname(&attr.name)
                && let Some(uri) = self.check_prefix(prefix, &attr.name)
                && let Some(first) = expanded.insert((uri, local), name)
            {
                self.diag(Code::DuplicateExpandedAttribute, attr.name.clone(), vec![first.to_owned(), name.to_owned()]);
            }
        }
    }

    fn find_duplicate_attributes(&mut self, attrs: &[Attr]) {
        let mut seen: HashSet<&str> = HashSet::new();
        for attr in attrs {
            let name = self.slice(&attr.name);
            if !seen.insert(name) {
                self.diag(Code::DuplicateAttribute, attr.name.clone(), vec![name.to_owned()]);
            }
        }
    }

    /// The prefix and local part of the name at `range`, reporting a name that is not a valid (qualified) name.
    fn qname(&mut self, range: &Range<usize>) -> Option<(Option<&'a str>, &'a str)> {
        let name = self.slice(range);
        if !name.chars().next().is_some_and(is_name_start_char) {
            self.diag(Code::InvalidName, range.clone(), vec![name.to_owned()]);
            return None;
        }
        let split = split_qname(name);
        if split.is_none() {
            self.diag(Code::InvalidQName, range.clone(), vec![name.to_owned()]);
        }
        split
    }

    /// Look the prefix of the name at `range` up; report it if it is not declared.
    fn check_prefix(&mut self, prefix: &str, range: &Range<usize>) -> Option<String> {
        let uri = self.ns.lookup(Some(prefix)).map(str::to_owned);
        if uri.is_none() {
            self.diag(Code::UnboundPrefix, range.start..range.start + prefix.len(), vec![prefix.to_owned()]);
        }
        uri
    }

    // --- comments, CDATA and processing instructions --------------------------------------------------------------------------

    fn comment(&mut self) {
        let Some(start) = self.peek() else { return };
        self.tree.start(SyntaxKind::Comment);
        self.bump();
        let mut closed = false;
        let mut last_text_ends_with_hyphen = false;
        while let Some(t) = self.peek() {
            match t.kind {
                TokenKind::CommentText => {
                    let text = self.slice(&t.range());
                    if let Some(i) = text.find("--") {
                        self.diag(Code::DoubleHyphenInComment, t.start + i..t.start + i + 2, Vec::new());
                    }
                    last_text_ends_with_hyphen = text.ends_with('-');
                    self.check_chars(t.range());
                    self.bump();
                }
                TokenKind::CommentEnd => {
                    if last_text_ends_with_hyphen {
                        // `--->`: the last hyphen of the text and the first of the end.
                        self.diag(Code::DoubleHyphenInComment, t.start.saturating_sub(1)..t.start + 1, Vec::new());
                    }
                    self.bump();
                    closed = true;
                    break;
                }
                _ => break,
            }
        }
        if !closed {
            self.diag(Code::UnterminatedComment, start.range(), Vec::new());
        }
        self.tree.finish();
    }

    fn cdata(&mut self) {
        let Some(start) = self.peek() else { return };
        if self.open.is_empty() {
            self.outside_root(start.range());
        }
        self.tree.start(SyntaxKind::CData);
        self.bump();
        let mut closed = false;
        while let Some(t) = self.peek() {
            match t.kind {
                TokenKind::CdataText => {
                    self.check_chars(t.range());
                    self.bump();
                }
                TokenKind::CdataEnd => {
                    self.bump();
                    closed = true;
                    break;
                }
                _ => break,
            }
        }
        if !closed {
            self.diag(Code::UnterminatedCdata, start.range(), Vec::new());
        }
        self.tree.finish();
    }

    /// A processing instruction, or the XML declaration. In the DTD's internal subset (`in_subset`) there is no declaration.
    fn pi(&mut self, in_subset: bool) {
        let Some(start) = self.peek() else { return };
        let target = self.peek_nth(1).filter(|t| t.kind == TokenKind::PiTarget);
        if !in_subset && target.is_some_and(|t| self.slice(&t.range()) == "xml") {
            self.xml_decl(start);
            return;
        }
        self.tree.start(SyntaxKind::ProcessingInstruction);
        self.bump();
        match target {
            Some(t) => {
                let name = self.slice(&t.range());
                if name.eq_ignore_ascii_case("xml") {
                    self.diag(Code::ReservedPiTarget, t.range(), vec![name.to_owned()]);
                }
                self.bump();
            }
            None => self.diag(Code::MissingPiTarget, start.range(), Vec::new()),
        }
        let mut closed = false;
        while let Some(t) = self.peek() {
            match t.kind {
                TokenKind::PiText => {
                    self.check_chars(t.range());
                    self.bump();
                }
                TokenKind::PiEnd => {
                    self.bump();
                    closed = true;
                    break;
                }
                _ => break,
            }
        }
        if !closed {
            self.diag(Code::UnterminatedPi, start.range(), Vec::new());
        }
        self.tree.finish();
    }

    /// `<?xml version="1.0" encoding="UTF-8"?>`; `start` is its `<?`.
    fn xml_decl(&mut self, start: Token) {
        let misplaced = !(start.start == 0 || (start.start == '\u{FEFF}'.len_utf8() && self.src.starts_with('\u{FEFF}')));
        self.tree.start(if misplaced { SyntaxKind::ProcessingInstruction } else { SyntaxKind::XmlDecl });
        self.bump(); // <?
        let target = self.bump().map_or(start.range(), |t| t.range()); // xml
        if misplaced {
            self.diag(Code::MisplacedXmlDecl, start.start..target.end, Vec::new());
        }
        let mut pseudo: Vec<(Range<usize>, Option<Range<usize>>)> = Vec::new();
        let mut closed = false;
        while let Some(t) = self.peek() {
            match t.kind {
                TokenKind::Whitespace => {
                    self.bump();
                }
                TokenKind::PiEnd => {
                    self.bump();
                    closed = true;
                    break;
                }
                TokenKind::Name => {
                    self.bump();
                    let name_text = self.slice(&t.range()).to_owned();
                    let mut value = None;
                    if self.next_is_eq() {
                        self.skip_whitespace();
                        self.bump();
                        self.skip_whitespace();
                        if self.kind() == Some(TokenKind::Quote) {
                            value = self.literal();
                        } else {
                            self.diag(Code::MissingAttrValue, t.range(), vec![name_text]);
                        }
                    } else {
                        self.diag(Code::AttrWithoutValue, t.range(), vec![name_text]);
                    }
                    pseudo.push((t.range(), value));
                }
                TokenKind::Quote => {
                    self.diag(Code::UnexpectedCharacter, t.range(), vec!["\"".to_owned()]);
                    self.literal();
                }
                TokenKind::Eq | TokenKind::LiteralText | TokenKind::Error => {
                    let text = self.slice(&t.range()).to_owned();
                    self.diag(Code::UnexpectedCharacter, t.range(), vec![text]);
                    self.bump();
                }
                _ => break, // a declaration left open: content begins
            }
        }
        if !closed {
            self.diag(Code::UnterminatedXmlDecl, start.range(), Vec::new());
        }
        self.tree.finish();
        if !misplaced {
            self.check_xml_decl(&target, &pseudo);
        }
    }

    /// The version, encoding and standalone pseudo-attributes, in that order, with legal values.
    fn check_xml_decl(&mut self, target: &Range<usize>, pseudo: &[(Range<usize>, Option<Range<usize>>)]) {
        const ORDER: [&str; 3] = ["version", "encoding", "standalone"];
        let mut last: Option<usize> = None;
        let mut seen = [false; 3];
        for (name_range, value_range) in pseudo {
            let name = self.slice(name_range);
            let value = value_range.as_ref().map(|r| (self.slice(r), r.clone()));
            let Some(index) = ORDER.iter().position(|n| *n == name) else {
                self.diag(Code::UnknownPseudoAttr, name_range.clone(), vec![name.to_owned()]);
                continue;
            };
            if seen[index] {
                self.diag(Code::DuplicatePseudoAttr, name_range.clone(), vec![name.to_owned()]);
                continue;
            }
            seen[index] = true;
            if let Some(prev) = last.filter(|prev| *prev > index) {
                self.diag(Code::PseudoAttrOrder, name_range.clone(), vec![name.to_owned(), ORDER.get(prev).copied().unwrap_or_default().to_owned()]);
            }
            last = Some(last.map_or(index, |prev| prev.max(index)));
            let Some((text, span)) = value else { continue };
            match index {
                0 => {
                    let minor = text.strip_prefix("1.").filter(|m| !m.is_empty() && m.bytes().all(|b| b.is_ascii_digit()));
                    if text != "1.0" {
                        if minor.is_some() {
                            self.diag(Code::UnsupportedVersion, span, vec![text.to_owned()]);
                        } else {
                            self.diag(Code::BadVersion, span, vec![text.to_owned()]);
                        }
                    }
                }
                1 => {
                    let mut chars = text.chars();
                    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
                        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
                    if !valid {
                        self.diag(Code::BadEncodingName, span, vec![text.to_owned()]);
                    }
                }
                _ => {
                    if !matches!(text, "yes" | "no") {
                        self.diag(Code::BadStandalone, span, vec![text.to_owned()]);
                    }
                }
            }
        }
        if !seen[0] {
            self.diag(Code::MissingVersion, target.clone(), Vec::new());
        }
    }

    /// A quoted literal of the DOCTYPE, a declaration or the XML declaration: quotes and text are tokens of the current node. Returns
    /// the range between the quotes.
    fn literal(&mut self) -> Option<Range<usize>> {
        let open = self.bump()?;
        let quote = self.slice(&open.range()).to_owned();
        let (start, mut end, mut closed) = (open.end, open.end, false);
        while let Some(t) = self.peek() {
            match t.kind {
                TokenKind::LiteralText => {
                    self.check_chars(t.range());
                    self.bump();
                    end = t.end;
                }
                TokenKind::Quote => {
                    end = t.start;
                    self.bump();
                    closed = true;
                    break;
                }
                _ => break,
            }
        }
        if !closed {
            self.diag(Code::UnterminatedLiteral, open.range(), vec![quote]);
        }
        Some(start..end)
    }

    // --- DOCTYPE -------------------------------------------------------------------------------------------------------------

    fn doctype(&mut self) {
        let Some(start) = self.peek() else { return };
        if !self.open.is_empty() || self.seen_root {
            self.diag(Code::MisplacedDoctype, start.range(), Vec::new());
        } else if self.seen_doctype {
            self.diag(Code::MultipleDoctype, start.range(), Vec::new());
        }
        self.seen_doctype = true;
        self.tree.start(SyntaxKind::Doctype);
        self.bump();
        let mut words: Vec<Range<usize>> = Vec::new();
        let mut literals = 0usize;
        let mut closed = false;
        while let Some(t) = self.peek() {
            match t.kind {
                TokenKind::Whitespace => {
                    self.bump();
                }
                TokenKind::Name => {
                    words.push(t.range());
                    self.bump();
                }
                TokenKind::Quote => {
                    self.literal();
                    literals += 1;
                }
                TokenKind::LBracket => self.subset(),
                TokenKind::Gt => {
                    self.bump();
                    closed = true;
                    break;
                }
                TokenKind::Error => {
                    let text = self.slice(&t.range()).to_owned();
                    self.diag(Code::UnexpectedCharacter, t.range(), vec![text]);
                    self.bump();
                }
                _ => break,
            }
        }
        if !closed {
            self.diag(Code::UnterminatedDoctype, start.range(), Vec::new());
        }
        self.tree.finish();
        self.check_doctype_head(&start, &words, literals);
    }

    fn check_doctype_head(&mut self, start: &Token, words: &[Range<usize>], literals: usize) {
        if words.is_empty() {
            self.diag(Code::MissingDoctypeName, start.range(), Vec::new());
            return;
        }
        let keyword = words.get(1).map(|r| (self.slice(r), r.clone()));
        let problem = match keyword {
            None if literals != 0 => Some(("a quoted string needs SYSTEM or PUBLIC before it".to_owned(), start.range())),
            Some(("SYSTEM", span)) if literals != 1 => Some(("SYSTEM must be followed by one quoted string".to_owned(), span)),
            Some(("PUBLIC", span)) if literals != 2 => Some(("PUBLIC must be followed by two quoted strings".to_owned(), span)),
            Some((other, span)) if other != "SYSTEM" && other != "PUBLIC" => Some((format!("unexpected '{other}', expected SYSTEM or PUBLIC"), span)),
            _ => None,
        };
        if let Some((message, span)) = problem {
            self.diag(Code::BadDoctype, span, vec![message]);
        } else if let Some(extra) = words.get(2) {
            self.diag(Code::BadDoctype, extra.clone(), vec!["unexpected extra name".to_owned()]);
        }
    }

    /// The internal subset: `[` declarations `]`.
    fn subset(&mut self) {
        self.tree.start(SyntaxKind::DoctypeSubset);
        self.bump();
        while let Some(t) = self.peek() {
            match t.kind {
                TokenKind::Whitespace | TokenKind::PercentRef => {
                    self.bump();
                }
                TokenKind::DeclStart => self.markup_decl(),
                TokenKind::CommentStart => self.comment(),
                TokenKind::PiStart => self.pi(true),
                TokenKind::RBracket => {
                    self.bump();
                    break;
                }
                _ => {
                    let text = self.slice(&t.range()).to_owned();
                    self.diag(Code::UnexpectedCharacter, t.range(), vec![text]);
                    self.bump();
                }
            }
        }
        self.tree.finish();
    }

    /// `<!ELEMENT ...>` and its kin; the DTD parser reads the inside, here only the extent matters.
    fn markup_decl(&mut self) {
        let Some(start) = self.peek() else { return };
        self.tree.start(SyntaxKind::MarkupDecl);
        self.bump();
        let mut closed = false;
        while let Some(t) = self.peek() {
            match t.kind {
                TokenKind::Gt => {
                    self.bump();
                    closed = true;
                    break;
                }
                TokenKind::Whitespace | TokenKind::Name | TokenKind::Punct | TokenKind::PercentRef => {
                    self.bump();
                }
                TokenKind::Quote => {
                    self.literal();
                }
                TokenKind::Error => {
                    let text = self.slice(&t.range()).to_owned();
                    self.diag(Code::UnexpectedCharacter, t.range(), vec![text]);
                    self.bump();
                }
                _ => break,
            }
        }
        if !closed {
            self.diag(Code::UnterminatedDecl, start.range(), Vec::new());
        }
        self.tree.finish();
    }

    fn declare(&mut self, prefix: Option<&str>, attr: &Attr) {
        let span = attr.name.clone();
        if prefix.is_some_and(|p| !is_ncname(p)) {
            let name = self.slice(&span).to_owned();
            self.diag(Code::InvalidQName, span, vec![name]);
            return;
        }
        let Some(value) = attr.value.clone() else { return };
        let uri = decode_references(self.slice(&value));
        match prefix {
            Some("xmlns") => self.diag(Code::ReservedPrefix, span, vec!["xmlns".to_owned()]),
            Some("xml") => {
                if uri != XML_NAMESPACE {
                    self.diag(Code::ReservedPrefix, span, vec!["xml".to_owned()]);
                }
            }
            Some(p) if uri.is_empty() => self.diag(Code::EmptyNamespaceName, span, vec![p.to_owned()]),
            _ if uri == XML_NAMESPACE || uri == XMLNS_NAMESPACE => {
                self.diag(Code::ReservedNamespace, span, vec![uri, prefix.unwrap_or_default().to_owned()]);
            }
            _ => {
                self.ns.declare(prefix, &uri);
            }
        }
    }
}

/// The character a reference like `&#65;` or `&#x41;` stands for, if it is one XML allows.
fn char_ref_value(reference: &str) -> Option<char> {
    let digits = reference.strip_prefix("&#")?.strip_suffix(';')?;
    let code = match digits.strip_prefix('x') {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse::<u32>().ok()?,
    };
    char::from_u32(code).filter(|c| is_xml_char(*c))
}

/// `raw` with the predefined entities and valid character references replaced; anything else is left as written.
fn decode_references(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find('&') {
        out.push_str(rest.get(..i).unwrap_or_default());
        let tail = rest.get(i..).unwrap_or_default();
        let end = tail.find(';').map_or(tail.len(), |e| e + 1);
        let reference = tail.get(..end).unwrap_or_default();
        let decoded = match reference {
            "&lt;" => Some('<'),
            "&gt;" => Some('>'),
            "&amp;" => Some('&'),
            "&quot;" => Some('"'),
            "&apos;" => Some('\''),
            _ => char_ref_value(reference),
        };
        match decoded {
            Some(c) => out.push(c),
            None => out.push_str(reference),
        }
        rest = tail.get(end..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}
